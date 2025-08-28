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
    results::BenchmarkResults,
};
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, info, warn};

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

        // For UDS cross-environment MVP: wait for the client-side server socket to appear
        let ipc_path = self
            .config
            .ipc_path
            .as_ref()
            .ok_or_else(|| anyhow!("ipc_path must be specified in host mode"))?
            .clone();

        info!("Waiting for client socket at {:?}", ipc_path);
        let timeout_duration = Duration::from_secs(self.config.connection_timeout);
        let start_time = Instant::now();
        while start_time.elapsed() < timeout_duration {
            if std::path::Path::new(&ipc_path).exists() {
                info!("Client socket detected at {:?}", ipc_path);
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }

        if !std::path::Path::new(&ipc_path).exists() {
            return Err(anyhow!(
                "Timeout waiting for client socket at {:?}",
                ipc_path
            ));
        }

        // Execute a client-side benchmark against the client-provided UDS server
        self.execute_benchmark().await?;

        info!("Host coordination completed successfully");
        Ok(Vec::new())
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

        // For single client scenarios, use the simple expected socket name
        if self.config.client_count == 1 && client_id == 0 {
            let filename = "ipc_benchmark.sock";
            Ok(base_path.join(filename))
        } else {
            // For multi-client scenarios, use unique names
            let filename = format!("rusty-comms-{}-client{}-{:?}.sock", 
                                   std::process::id(), client_id, mechanism);
            Ok(base_path.join(filename))
        }
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
        
        // Configure the command to run a UDS server that creates a socket at the specified path
        // Use the proper CLI short value (e.g., "uds") for the mechanism
        cmd.arg("-m").arg(mechanism.cli_value())
           .arg("--message-size").arg(self.config.message_size.to_string())
           .arg("--concurrency").arg("1") // Force single-threaded for cross-env
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
        info!("Executing coordinated benchmark (UDS client)");

        // Only UDS is supported in this minimal cross-env path
        use crate::ipc::{IpcTransport, TransportConfig};
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;

        let socket_path = self
            .config
            .ipc_path
            .as_ref()
            .ok_or_else(|| anyhow!("ipc_path must be specified in host mode"))?
            .to_string_lossy()
            .to_string();

        let transport_config = TransportConfig {
            socket_path,
            buffer_size: self.config.buffer_size,
            host: self.config.host.clone(),
            port: self.config.port,
            shared_memory_name: "ipc_benchmark_shm".to_string(),
            max_connections: 1,
            message_queue_depth: 10,
            message_queue_name: "ipc_benchmark_pmq".to_string(),
        };

        let mut client = UnixDomainSocketTransport::new();
        client.start_client(&transport_config).await?;
        info!("Connected to client UDS server");

        // Prepare payload and metrics collection
        let payload = vec![0u8; self.config.message_size];
        let mut latencies_ns: Vec<u128> = Vec::with_capacity(self.config.msg_count.unwrap_or(10_000));
        let bench_start = Instant::now();
        let mut messages_sent: u64 = 0;

        // Initialize streaming writers once (avoid creating runtime inside runtime)
        let mut stream_manager: Option<crate::results::ResultsManager> = {
            // Only create if either streaming path is set
            if self.config.streaming_output_json.is_some() || self.config.streaming_output_csv.is_some() {
                let mut rm = crate::results::ResultsManager::new(None, None)?;
                if let Some(ref json_path) = self.config.streaming_output_json {
                    rm.enable_per_message_streaming(json_path)?;
                }
                if let Some(ref csv_path) = self.config.streaming_output_csv {
                    rm.enable_csv_streaming(csv_path)?;
                }
                Some(rm)
            } else {
                None
            }
        };

        // Default to one-way sends; if round_trip set, measure round-trip latency instead
        if let Some(duration) = self.config.duration {
            let start = Instant::now();
            let mut i: u64 = 0;
            while start.elapsed() < duration {
                let send_start = Instant::now();
                let msg = crate::ipc::Message::new(i, payload.clone(), if self.config.round_trip { crate::ipc::MessageType::Request } else { crate::ipc::MessageType::OneWay });
                let _ = client.send(&msg).await; // ignore individual send errors
                if self.config.round_trip {
                    let _ = client.receive().await; // best-effort response
                }
                // Stream per-message latency if configured
                if let Some(rm) = &mut stream_manager {
                    let record = crate::results::MessageLatencyRecord::new(
                        i,
                        crate::cli::IpcMechanism::UnixDomainSocket,
                        self.config.message_size,
                        if self.config.round_trip { crate::metrics::LatencyType::RoundTrip } else { crate::metrics::LatencyType::OneWay },
                        send_start.elapsed(),
                    );
                    rm.write_streaming_record_direct(&record).await?;
                }
                let elapsed = send_start.elapsed().as_nanos();
                latencies_ns.push(elapsed);
                i += 1;
                messages_sent = i;
            }
        } else {
            let count = self.config.msg_count.unwrap_or(1000);
            for i in 0..count {
                let send_start = Instant::now();
                let msg = crate::ipc::Message::new(i as u64, payload.clone(), if self.config.round_trip { crate::ipc::MessageType::Request } else { crate::ipc::MessageType::OneWay });
                let _ = client.send(&msg).await;
                if self.config.round_trip {
                    let _ = client.receive().await;
                }
                if let Some(rm) = &mut stream_manager {
                    let record = crate::results::MessageLatencyRecord::new(
                        i as u64,
                        crate::cli::IpcMechanism::UnixDomainSocket,
                        self.config.message_size,
                        if self.config.round_trip { crate::metrics::LatencyType::RoundTrip } else { crate::metrics::LatencyType::OneWay },
                        send_start.elapsed(),
                    );
                    rm.write_streaming_record_direct(&record).await?;
                }
                let elapsed = send_start.elapsed().as_nanos();
                latencies_ns.push(elapsed);
                messages_sent = i as u64 + 1;
            }
        }

        client.close().await?;
        info!("Coordinated benchmark complete");

        // Finalize streaming files
        if let Some(rm) = &mut stream_manager {
            rm.flush_pending_records().await?;
            rm.finalize().await?;
        }

        // Compute simple metrics
        let duration_ns_total = bench_start.elapsed().as_nanos();
        let total_bytes = (messages_sent as usize) * self.config.message_size;
        let msgs_per_sec = if duration_ns_total > 0 { (messages_sent as f64) / (duration_ns_total as f64 / 1e9) } else { 0.0 };
        let bytes_per_sec = msgs_per_sec * (self.config.message_size as f64);

        latencies_ns.sort_unstable();
        let len = latencies_ns.len();
        let p = |q: f64| -> Option<u128> {
            if len == 0 { return None; }
            let idx = ((q / 100.0) * (len as f64 - 1.0)).round() as usize;
            latencies_ns.get(idx).copied()
        };
        let mean_ns = if len > 0 { (latencies_ns.iter().sum::<u128>() as f64 / len as f64) as u128 } else { 0 };
        let min_ns = latencies_ns.first().copied().unwrap_or(0);
        let max_ns = latencies_ns.last().copied().unwrap_or(0);

        // Write output artifact for host side with metrics
        let out_path = if let Some(path) = &self.config.output_file {
            path.clone()
        } else {
            let out_dir = std::env::var("IPC_BENCHMARK_OUTPUT_DIR").unwrap_or_else(|_| "./output".to_string());
            std::path::Path::new(&out_dir).join("host_client_results.json")
        };
        let result = serde_json::json!({
            "mechanism": "UnixDomainSocket",
            "message_size": self.config.message_size,
            "round_trip": self.config.round_trip,
            "mode": "host",
            "totals": {
                "messages_sent": messages_sent,
                "bytes_transferred": total_bytes,
                "duration_ns": duration_ns_total,
            },
            "throughput": {
                "messages_per_second": msgs_per_sec,
                "bytes_per_second": bytes_per_sec,
            },
            "latency_ns": {
                "min": min_ns,
                "max": max_ns,
                "mean": mean_ns,
                "p50": p(50.0).unwrap_or(0),
                "p95": p(95.0).unwrap_or(0),
                "p99": p(99.0).unwrap_or(0),
            }
        });
        if let Some(parent) = out_path.parent() { let _ = std::fs::create_dir_all(parent); }
        let _ = std::fs::write(&out_path, serde_json::to_string_pretty(&result)?);
        Ok(())
    }

    /// Collect results from all processes
    async fn collect_results(&self) -> Result<Vec<BenchmarkResults>> {
        info!("Collecting results from all processes");

        let processes = self.server_processes.lock().await;
        let results = Vec::new();

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
        info!("Client mode: starting UDS server at {:?}", self.connection_endpoint);
        use crate::ipc::{IpcTransport, TransportConfig};
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;

        let mut transport = UnixDomainSocketTransport::new();
        let config = TransportConfig {
            socket_path: self.connection_endpoint.to_string_lossy().to_string(),
            buffer_size: 8192,
            host: "127.0.0.1".to_string(),
            port: 8080,
            shared_memory_name: "ipc_benchmark_shm".to_string(),
            max_connections: 16,
            message_queue_depth: 10,
            message_queue_name: "ipc_benchmark_pmq".to_string(),
        };

        // Start UDS server that the host will connect to
        transport.start_server(&config).await?;
        info!("UDS server listening at {:?}", self.connection_endpoint);

        // Serve until idle for a grace period after last activity
        let idle_timeout = Duration::from_secs(2);
        let mut last_activity = Instant::now();
        let mut received: usize = 0;

        loop {
            // If no activity for idle_timeout, assume host is done and shut down
            if last_activity.elapsed() >= idle_timeout {
                break;
            }

            match tokio::time::timeout(Duration::from_millis(250), transport.receive()).await {
                Ok(Ok(request)) => {
                    received += 1;
                    last_activity = Instant::now();
                    if self.config.round_trip {
                        let response = crate::ipc::Message::new(
                            request.id,
                            request.payload,
                            crate::ipc::MessageType::Response,
                        );
                        let _ = transport.send(&response).await;
                    }
                }
                Ok(Err(_)) => {
                    // Transport error; continue allowing idle timeout to trigger
                }
                Err(_) => {
                    // Timeout waiting; loop to check idle timer
                }
            }
        }

        // Close server (will unlink the socket because client owns it)
        let _ = transport.close().await;
        info!("Client UDS server exiting after {} messages", received);
        Ok(())
    }

    /// Wait for host connection
    async fn wait_for_host_connection(&self) -> Result<()> {
        info!("Waiting for host to initiate connection");

        // Create a UDS server that listens for connections
        use crate::ipc::{IpcTransport, TransportConfig};
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
        
        let mut transport = UnixDomainSocketTransport::new();
        let config = TransportConfig {
            socket_path: self.connection_endpoint.to_string_lossy().to_string(),
            buffer_size: 8192,
            host: "127.0.0.1".to_string(),
            port: 8080,
            shared_memory_name: "ipc_benchmark_shm".to_string(),
            max_connections: 16,
            message_queue_depth: 10,
            message_queue_name: "ipc_benchmark_pmq".to_string(),
        };

        // Start the UDS server
        transport.start_server(&config).await?;
        info!("UDS server started, waiting for connections on {:?}", self.connection_endpoint);

        // Keep the server running - in a real implementation this would handle benchmark requests
        // For now, just keep it alive to allow client connections
        loop {
            sleep(Duration::from_secs(1)).await;
        }
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
