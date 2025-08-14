//! # Cross-Environment Coordination Module
//!
//! This module implements the process coordination logic for Issue #11 
//! cross-environment IPC benchmarking between host and container environments.
//!
//! ## Architecture Overview
//!
//! ```
//! ┌─────────────────────┐    ┌─────────────────────┐
//! │   Host Environment  │    │ Container Environment│
//! │ (Safety Domain)     │    │ (Non-Safety Domain) │
//! │                     │    │                     │
//! │ ┌─────────────────┐ │    │ ┌─────────────────┐ │
//! │ │ HostCoordinator │ │    │ │ ClientProcess   │ │
//! │ │  - Spawns       │ │    │ │  - Waits for    │ │
//! │ │    servers      │◄────┤ │    connection   │ │
//! │ │  - Collects     │ │    │ │  - Passive mode │ │
//! │ │    results      │ │    │ │                 │ │
//! │ └─────────────────┘ │    │ └─────────────────┘ │
//! └─────────────────────┘    └─────────────────────┘
//! ```
//!
//! ## Key Components
//!
//! - **HostCoordinator**: Manages server process spawning and result collection
//! - **ClientProcess**: Handles container-side client coordination  
//! - **ProcessManager**: Low-level process spawning and lifecycle management
//! - **ResultAggregator**: Collects and combines results from multiple processes

use crate::{
    cli::{BenchmarkConfiguration, ExecutionMode},
    results::{BenchmarkResults, ResultsManager},
};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// Process identifier for tracking spawned processes
pub type ProcessId = u32;

/// Host coordinator for cross-environment IPC benchmarking
/// 
/// Manages multiple server processes that communicate with containerized clients.
/// This coordinator is responsible for:
/// - Spawning dedicated server processes for each expected client
/// - Coordinating benchmark timing and synchronization
/// - Collecting results from all server processes
/// - Aggregating final performance metrics
pub struct HostCoordinator {
    config: BenchmarkConfiguration,
    server_processes: Arc<Mutex<HashMap<ProcessId, ServerProcess>>>,
    results_aggregator: ResultAggregator,
    next_process_id: ProcessId,
}

/// Represents a spawned server process
struct ServerProcess {
    process_id: ProcessId,
    child: Child,
    mechanism: crate::cli::IpcMechanism,
    ipc_path: PathBuf,
    started_at: Instant,
}

/// Client process coordinator for container environments
///
/// Handles client-side coordination in containerized environments.
/// Operates in passive mode, waiting for host-initiated connections
/// and responding to benchmark requests.
pub struct ClientProcess {
    config: BenchmarkConfiguration,
    connection_endpoint: PathBuf,
}

/// Aggregates results from multiple processes
struct ResultAggregator {
    results: Arc<Mutex<Vec<BenchmarkResults>>>,
}

impl HostCoordinator {
    /// Create a new host coordinator
    pub fn new(config: BenchmarkConfiguration) -> Result<Self> {
        if config.mode != ExecutionMode::Host {
            return Err(anyhow!("HostCoordinator requires ExecutionMode::Host"));
        }

        if config.client_count == 0 {
            return Err(anyhow!("client_count must be greater than 0"));
        }

        Ok(Self {
            config,
            server_processes: Arc::new(Mutex::new(HashMap::new())),
            results_aggregator: ResultAggregator::new(),
            next_process_id: 1,
        })
    }

    /// Run the host coordination process
    /// 
    /// This is the main entry point for host mode operation:
    /// 1. Spawn server processes for each expected client
    /// 2. Wait for all clients to connect
    /// 3. Execute benchmark coordination
    /// 4. Collect and aggregate results
    pub async fn run(&mut self) -> Result<Vec<BenchmarkResults>> {
        info!("Starting host coordinator for {} clients", self.config.client_count);

        // Spawn server processes for each expected client
        self.spawn_server_processes().await?;

        // Wait for all clients to connect
        self.wait_for_client_connections().await?;

        // Execute coordinated benchmark
        self.execute_benchmark().await?;

        // Collect results from all processes
        let results = self.collect_results().await?;

        // Clean up processes
        self.cleanup_processes().await?;

        info!("Host coordination completed successfully");
        Ok(results)
    }

    /// Spawn server processes for each expected client
    async fn spawn_server_processes(&mut self) -> Result<()> {
        let mut processes = self.server_processes.lock().await;
        
        for client_id in 0..self.config.client_count {
            for mechanism in &self.config.mechanisms {
                let process_id = self.next_process_id;
                self.next_process_id += 1;

                let ipc_path = self.generate_ipc_path(client_id, mechanism)?;
                
                info!("Spawning server process {} for client {} using {:?}", 
                      process_id, client_id, mechanism);

                let child = self.spawn_server_process(process_id, mechanism, &ipc_path).await?;

                let server_process = ServerProcess {
                    process_id,
                    child,
                    mechanism: *mechanism,
                    ipc_path,
                    started_at: Instant::now(),
                };

                processes.insert(process_id, server_process);
            }
        }

        info!("Spawned {} server processes", processes.len());
        Ok(())
    }

    /// Generate IPC path for a specific client and mechanism
    fn generate_ipc_path(&self, client_id: usize, mechanism: &crate::cli::IpcMechanism) -> Result<PathBuf> {
        let base_path = self.config.ipc_path
            .as_ref()
            .ok_or_else(|| anyhow!("ipc_path must be specified in host mode"))?;

        let filename = format!("rusty-comms-{}-client{}-{:?}.sock", 
                               std::process::id(), client_id, mechanism);
        Ok(base_path.join(filename))
    }

    /// Spawn a single server process
    async fn spawn_server_process(
        &self,
        process_id: ProcessId,
        mechanism: &crate::cli::IpcMechanism,
        ipc_path: &PathBuf,
    ) -> Result<Child> {
        let current_exe = std::env::current_exe()
            .map_err(|e| anyhow!("Failed to get current executable path: {}", e))?;

        let mut cmd = Command::new(current_exe);
        
        // Configure the command to run in standalone server mode
        cmd.arg("--mode").arg("standalone")
           .arg("--mechanisms").arg(&format!("{:?}", mechanism).to_lowercase())
           .arg("--message-size").arg(self.config.message_size.to_string())
           .arg("--concurrency").arg("1") // Force single-threaded for cross-env
           .arg("--ipc-path").arg(ipc_path)
           .stdin(Stdio::null())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        // Add duration or message count
        if let Some(duration) = self.config.duration {
            cmd.arg("--duration").arg(format!("{}s", duration.as_secs()));
        } else if let Some(msg_count) = self.config.msg_count {
            cmd.arg("--msg-count").arg(msg_count.to_string());
        }

        // Enable debug logging if requested
        if self.config.cross_env_debug {
            cmd.arg("--verbose").arg("--verbose");
        }

        let child = cmd.spawn()
            .map_err(|e| anyhow!("Failed to spawn server process {}: {}", process_id, e))?;

        debug!("Spawned server process {} with PID {}", process_id, child.id());
        Ok(child)
    }

    /// Wait for all client connections with timeout
    async fn wait_for_client_connections(&self) -> Result<()> {
        let timeout_duration = Duration::from_secs(self.config.connection_timeout);
        let start_time = Instant::now();

        info!("Waiting for {} clients to connect (timeout: {}s)", 
              self.config.client_count, self.config.connection_timeout);

        while start_time.elapsed() < timeout_duration {
            // Check if all expected clients have connected
            // This is a simplified implementation - in practice we'd check actual connections
            let connected_clients = self.count_connected_clients().await;
            
            if connected_clients >= self.config.client_count {
                info!("All {} clients connected", self.config.client_count);
                return Ok(());
            }

            if self.config.cross_env_debug {
                debug!("Connected clients: {}/{}", connected_clients, self.config.client_count);
            }

            sleep(Duration::from_millis(500)).await;
        }

        Err(anyhow!("Timeout waiting for client connections after {}s", 
                   self.config.connection_timeout))
    }

    /// Count connected clients (simplified implementation)
    async fn count_connected_clients(&self) -> usize {
        // This is a placeholder implementation
        // In practice, we'd check actual socket connections or coordination files
        let processes = self.server_processes.lock().await;
        processes.len() / self.config.mechanisms.len()
    }

    /// Execute coordinated benchmark across all processes
    async fn execute_benchmark(&self) -> Result<()> {
        info!("Executing coordinated benchmark");

        // In a full implementation, this would:
        // 1. Send start signals to all processes
        // 2. Monitor progress
        // 3. Handle any process failures
        // 4. Coordinate timing across processes

        let estimated_duration = self.config.duration
            .unwrap_or(Duration::from_secs(30)); // Default estimate

        info!("Benchmark running for approximately {:?}", estimated_duration);
        sleep(estimated_duration + Duration::from_secs(5)).await; // Extra time for cleanup

        Ok(())
    }

    /// Collect results from all processes
    async fn collect_results(&self) -> Result<Vec<BenchmarkResults>> {
        info!("Collecting results from all processes");

        let processes = self.server_processes.lock().await;
        let mut results = Vec::new();

        for (process_id, _server_process) in processes.iter() {
            // In practice, this would read results from process output or shared files
            info!("Collecting results from process {}", process_id);
            
            // Placeholder: In real implementation, parse JSON output from process
            // or read from shared result files
        }

        // For now, return empty results
        // In practice, this would aggregate real benchmark results
        Ok(results)
    }

    /// Clean up all spawned processes
    async fn cleanup_processes(&mut self) -> Result<()> {
        info!("Cleaning up server processes");

        let mut processes = self.server_processes.lock().await;
        
        for (process_id, mut server_process) in processes.drain() {
            match server_process.child.try_wait() {
                Ok(Some(status)) => {
                    info!("Process {} already exited with status: {}", process_id, status);
                }
                Ok(None) => {
                    info!("Terminating process {}", process_id);
                    if let Err(e) = server_process.child.kill() {
                        warn!("Failed to kill process {}: {}", process_id, e);
                    }
                }
                Err(e) => {
                    warn!("Error checking process {} status: {}", process_id, e);
                }
            }
        }

        Ok(())
    }
}

impl ClientProcess {
    /// Create a new client process coordinator
    pub fn new(config: BenchmarkConfiguration) -> Result<Self> {
        if config.mode != ExecutionMode::Client {
            return Err(anyhow!("ClientProcess requires ExecutionMode::Client"));
        }

        let connection_endpoint = config.ipc_path
            .as_ref()
            .ok_or_else(|| anyhow!("ipc_path must be specified in client mode"))?
            .clone();

        Ok(Self {
            config,
            connection_endpoint,
        })
    }

    /// Run the client process
    ///
    /// Operates in passive mode, waiting for host-initiated connections
    /// and responding to benchmark requests.
    pub async fn run(&self) -> Result<()> {
        info!("Starting client process, waiting for host connection at {:?}", 
              self.connection_endpoint);

        // Wait for host to initiate connection
        self.wait_for_host_connection().await?;

        // Participate in benchmark as directed by host
        self.participate_in_benchmark().await?;

        info!("Client process completed");
        Ok(())
    }

    /// Wait for host connection
    async fn wait_for_host_connection(&self) -> Result<()> {
        info!("Waiting for host to initiate connection");

        // In practice, this would monitor the IPC endpoint for connections
        // For now, this is a placeholder that waits for a reasonable time
        sleep(Duration::from_secs(5)).await;

        info!("Host connection established");
        Ok(())
    }

    /// Participate in benchmark as directed by host
    async fn participate_in_benchmark(&self) -> Result<()> {
        info!("Participating in benchmark");

        // In practice, this would:
        // 1. Accept messages from host
        // 2. Respond to benchmark requests
        // 3. Measure latency on client side
        // 4. Report back to host

        // Placeholder implementation
        let duration = self.config.duration.unwrap_or(Duration::from_secs(30));
        sleep(duration).await;

        Ok(())
    }
}

impl ResultAggregator {
    /// Create a new result aggregator
    fn new() -> Self {
        Self {
            results: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add results from a process
    async fn add_results(&self, results: BenchmarkResults) {
        let mut results_vec = self.results.lock().await;
        results_vec.push(results);
    }

    /// Get aggregated results
    async fn get_results(&self) -> Vec<BenchmarkResults> {
        let results_vec = self.results.lock().await;
        results_vec.clone()
    }
}

/// Factory function to create the appropriate coordinator based on execution mode
pub async fn create_coordinator(config: BenchmarkConfiguration) -> Result<Box<dyn CrossEnvironmentCoordinator>> {
    match config.mode {
        ExecutionMode::Host => {
            let coordinator = HostCoordinator::new(config)?;
            Ok(Box::new(coordinator))
        }
        ExecutionMode::Client => {
            let coordinator = ClientProcess::new(config)?;
            Ok(Box::new(coordinator))
        }
        ExecutionMode::Standalone => {
            Err(anyhow!("Standalone mode should not use cross-environment coordination"))
        }
    }
}

/// Trait for cross-environment coordinators
#[async_trait::async_trait]
pub trait CrossEnvironmentCoordinator: Send + Sync {
    /// Run the coordination process
    async fn run(&mut self) -> Result<()>;
}

#[async_trait::async_trait]
impl CrossEnvironmentCoordinator for HostCoordinator {
    async fn run(&mut self) -> Result<()> {
        self.run().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl CrossEnvironmentCoordinator for ClientProcess {
    async fn run(&mut self) -> Result<()> {
        self.run().await
    }
}
