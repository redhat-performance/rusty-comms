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
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{info, warn};


/// Host coordinator for cross-environment IPC benchmarking
/// 
/// Manages direct IPC communication with containerized clients.
/// This coordinator is responsible for:
/// - Establishing connections to containerized servers
/// - Coordinating benchmark timing and synchronization
/// - Collecting and aggregating performance metrics
pub struct HostCoordinator {
    config: BenchmarkConfiguration,
}

/// Client process coordinator for container environments
///
/// Handles client-side coordination in containerized environments.
/// Operates in passive mode, waiting for host-initiated connections
/// and responding to benchmark requests.
pub struct ClientProcess {
    config: BenchmarkConfiguration,
    connection_endpoint: Option<PathBuf>,
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

        // Wait for container endpoint by mechanism
        match self.config.mechanisms.first().copied().unwrap_or(crate::cli::IpcMechanism::UnixDomainSocket) {
            crate::cli::IpcMechanism::UnixDomainSocket => {
                let ipc_path = self
                    .config
                    .ipc_path
                    .as_ref()
                    .ok_or_else(|| anyhow!("ipc_path must be specified in host mode for UDS"))?;
                info!("Waiting for client socket at {:?}", ipc_path);
                let timeout_duration = Duration::from_secs(self.config.connection_timeout);
                let start_time = Instant::now();
                while start_time.elapsed() < timeout_duration {
                    if std::path::Path::new(&ipc_path).exists() {
                        info!("Client socket detected at {:?}", ipc_path);
                        // Give the server a moment to be ready for connections
                        // This avoids the race condition where socket exists but server isn't listening yet
                        sleep(Duration::from_millis(Self::SOCKET_READY_DELAY_MS)).await;
                        break;
                    }
                    sleep(Duration::from_millis(200)).await;
                }
                if !std::path::Path::new(&ipc_path).exists() {
                    return Err(anyhow!("Timeout waiting for client socket at {:?}", ipc_path));
                }
                let result = self.execute_benchmark().await?;
                return Ok(vec![result]);
            }
            crate::cli::IpcMechanism::SharedMemory => {
                // For SHM, container creates the segment; just wait briefly before driving the benchmark
                let timeout_duration = Duration::from_secs(self.config.connection_timeout);
                info!("Waiting for shared memory segment: {:?}", self.config.shm_name);
                sleep(timeout_duration.min(Duration::from_secs(1))).await; // minimal wait; connection retries are in transport
                let result = self.execute_benchmark().await?;
                return Ok(vec![result]);
            }
            crate::cli::IpcMechanism::PosixMessageQueue => {
                // For PMQ, container creates the queue; rely on client retry logic
                let timeout_duration = Duration::from_secs(self.config.connection_timeout);
                info!("Waiting briefly before driving PMQ benchmark (queue will be created by container)");
                sleep(timeout_duration.min(Duration::from_secs(1))).await;
                let result = self.execute_benchmark().await?;
                return Ok(vec![result]);
            }
            other => {
                return Err(anyhow!("Cross-environment not implemented for {:?}", other));
            }
        }

        info!("Host coordination completed successfully");
        Ok(Vec::new())
    }


    /// Execute coordinated benchmark across all processes
    async fn execute_benchmark(&self) -> Result<BenchmarkResults> {
        use crate::ipc::{IpcTransport, TransportConfig};

        // Select transport based on mechanism
        let mechanism = self.config.mechanisms.first().copied().unwrap_or(crate::cli::IpcMechanism::UnixDomainSocket);
        info!("Executing coordinated benchmark ({:?} client)", mechanism);
        let mut client: Box<dyn IpcTransport> = match mechanism {
            crate::cli::IpcMechanism::UnixDomainSocket => {
                use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
                let socket_path = self
                    .config
                    .ipc_path
                    .as_ref()
                    .ok_or_else(|| anyhow!("ipc_path must be specified in host mode for UDS"))?
                    .to_string_lossy()
                    .to_string();
                let cfg = TransportConfig { 
                    socket_path, 
                    buffer_size: self.config.buffer_size, 
                    host: self.config.host.clone(), 
                    port: self.config.port, 
                    shared_memory_name: "ipc_benchmark_shm".to_string(), 
                    max_connections: 1, 
                    message_queue_depth: 10, 
                    message_queue_name: "ipc_benchmark_pmq".to_string() 
                };
                let mut t = UnixDomainSocketTransport::new();
                
                // Retry connection logic to handle race conditions
                let mut retry_count = 0;
                let max_retries = 5;
                loop {
                    match t.start_client(&cfg).await {
                        Ok(()) => {
                            info!("Connected to client UDS server");
                            break;
                        }
                        Err(e) => {
                            retry_count += 1;
                            if retry_count >= max_retries {
                                return Err(anyhow!("Failed to connect to UDS server after {} retries: {}", max_retries, e));
                            }
                            warn!("Failed to connect to UDS server (attempt {}/{}): {}", retry_count, max_retries, e);
                            sleep(Duration::from_millis(Self::CONNECTION_RETRY_DELAY_MS)).await;
                        }
                    }
                }
                Box::new(t)
            }
            crate::cli::IpcMechanism::SharedMemory => {
                use crate::ipc::shared_memory::SharedMemoryTransport;
                let shm_name = self.config.shm_name.as_deref().unwrap_or("ipc_benchmark_shm_crossenv");
                let cfg = TransportConfig { 
                    socket_path: String::new(), 
                    buffer_size: self.config.buffer_size, 
                    host: self.config.host.clone(), 
                    port: self.config.port, 
                    shared_memory_name: shm_name.to_string(), 
                    max_connections: 1, 
                    message_queue_depth: 10, 
                    message_queue_name: "ipc_benchmark_pmq".to_string() 
                };
                let mut t = SharedMemoryTransport::new();
                t.start_client(&cfg).await?;
                info!("Connected to client SHM server");
                Box::new(t)
            }
            crate::cli::IpcMechanism::PosixMessageQueue => {
                use crate::ipc::posix_message_queue::PosixMessageQueueTransport;
                let cfg = TransportConfig {
                    socket_path: String::new(),
                    buffer_size: self.config.buffer_size,
                    host: self.config.host.clone(),
                    port: self.config.port,
                    shared_memory_name: String::new(),
                    max_connections: 1,
                    message_queue_depth: 10,
                    // Use a fixed cross-environment PMQ name so host and container agree
                    message_queue_name: "ipc_benchmark_pmq_crossenv".to_string(),
                };
                let mut t = PosixMessageQueueTransport::new();
                t.start_client(&cfg).await?;
                info!("Connected to client PMQ server");
                Box::new(t)
            }
            other => {
                return Err(anyhow!("Cross-environment not implemented for {:?}", other));
            }
        };

        // Prepare payload and metrics collection
        let payload = vec![0u8; self.config.message_size];
        let mut latencies_ns: Vec<u128> = Vec::with_capacity(self.config.msg_count.unwrap_or(10_000));
        let bench_start = Instant::now();
        let messages_sent: u64;

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

        // Default to one-way sends for SHM cross-env to avoid blocking; respect round_trip for UDS/PMQ
        let is_shm = matches!(mechanism, crate::cli::IpcMechanism::SharedMemory);
        let do_round_trip = if is_shm { false } else { self.config.round_trip };

        // Execute benchmark loop based on duration or message count
        messages_sent = if let Some(duration) = self.config.duration {
            self.run_benchmark_duration(
                client.as_mut(),
                &payload,
                duration,
                mechanism,
                is_shm,
                do_round_trip,
                &mut latencies_ns,
                &mut stream_manager,
            ).await?
        } else {
            let count = self.config.msg_count.unwrap_or(1000);
            self.run_benchmark_count(
                client.as_mut(),
                &payload,
                count,
                mechanism,
                is_shm,
                do_round_trip,
                &mut latencies_ns,
                &mut stream_manager,
            ).await?
        };

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

        // Calculate throughput in MB/s
        let average_megabytes_per_second = bytes_per_sec / 1_000_000.0;
        let peak_bytes_per_sec = latencies_ns
            .iter()
            .filter(|&&ns| ns > 0)
            .map(|&ns| (self.config.message_size as f64) * 1e9 / (ns as f64))
            .fold(0.0, f64::max);
        let peak_megabytes_per_second = peak_bytes_per_sec / 1_000_000.0;

        // Write output artifact for host side with metrics
        let out_path = if let Some(path) = &self.config.output_file {
            path.clone()
        } else {
            let out_dir = std::env::var("IPC_BENCHMARK_OUTPUT_DIR").unwrap_or_else(|_| "./output".to_string());
            std::path::Path::new(&out_dir).join("host_client_results.json")
        };
        let result = serde_json::json!({
            "mechanism": match mechanism {
                crate::cli::IpcMechanism::UnixDomainSocket => "UnixDomainSocket",
                crate::cli::IpcMechanism::SharedMemory => "SharedMemory",
                crate::cli::IpcMechanism::TcpSocket => "TCP Socket",
                crate::cli::IpcMechanism::PosixMessageQueue => "POSIX Message Queue",
                crate::cli::IpcMechanism::All => "All Mechanisms",
            },
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
                "average_megabytes_per_second": average_megabytes_per_second,
                "peak_megabytes_per_second": peak_megabytes_per_second,
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
        // Construct a minimal BenchmarkResults to return
        let mut br = crate::results::BenchmarkResults::new(
            mechanism,
            self.config.message_size,
            1,
            self.config.msg_count,
            self.config.duration,
        );
        // Populate throughput-only metrics
        let throughput = crate::metrics::ThroughputMetrics {
            messages_per_second: msgs_per_sec,
            bytes_per_second: bytes_per_sec,
            total_messages: messages_sent as usize,
            total_bytes: total_bytes,
            duration_ns: duration_ns_total as u64,
        };
        let perf = crate::metrics::PerformanceMetrics {
            latency: None,
            throughput,
            timestamp: chrono::Utc::now(),
        };
        br.add_one_way_results(perf.clone());
        Ok(br)
    }


    // Performance optimization constants
    const SHM_SEND_TIMEOUT_MS: u64 = 50;
    const SHM_RECEIVE_TIMEOUT_MS: u64 = 100;
    const SHM_RETRY_DELAY_MS: u64 = 1;
    const CONNECTION_RETRY_DELAY_MS: u64 = 200;
    const SOCKET_READY_DELAY_MS: u64 = 100;

    /// Execute benchmark for a specific duration
    async fn run_benchmark_duration(
        &self,
        client: &mut dyn crate::ipc::IpcTransport,
        payload: &[u8],
        duration: Duration,
        mechanism: crate::cli::IpcMechanism,
        is_shm: bool,
        do_round_trip: bool,
        latencies_ns: &mut Vec<u128>,
        stream_manager: &mut Option<crate::results::ResultsManager>,
    ) -> Result<u64> {
        let start = Instant::now();
        let mut i: u64 = 0;
        
        while start.elapsed() < duration {
            i += 1;
            self.execute_single_message(
                client, payload, i, mechanism, is_shm, do_round_trip, 
                latencies_ns, stream_manager
            ).await?;
        }
        
        Ok(i)
    }

    /// Execute benchmark for a specific message count
    async fn run_benchmark_count(
        &self,
        client: &mut dyn crate::ipc::IpcTransport,
        payload: &[u8],
        count: usize,
        mechanism: crate::cli::IpcMechanism,
        is_shm: bool,
        do_round_trip: bool,
        latencies_ns: &mut Vec<u128>,
        stream_manager: &mut Option<crate::results::ResultsManager>,
    ) -> Result<u64> {
        for i in 0..count {
            self.execute_single_message(
                client, payload, i as u64, mechanism, is_shm, do_round_trip,
                latencies_ns, stream_manager
            ).await?;
        }
        
        Ok(count as u64)
    }

    /// Execute a single message send/receive cycle
    async fn execute_single_message(
        &self,
        client: &mut dyn crate::ipc::IpcTransport,
        payload: &[u8],
        message_id: u64,
        mechanism: crate::cli::IpcMechanism,
        is_shm: bool,
        do_round_trip: bool,
        latencies_ns: &mut Vec<u128>,
        stream_manager: &mut Option<crate::results::ResultsManager>,
    ) -> Result<()> {
        let send_start = Instant::now();
        
        // Create message without cloning payload unnecessarily
        let message_type = if do_round_trip { 
            crate::ipc::MessageType::Request 
        } else { 
            crate::ipc::MessageType::OneWay 
        };
        let msg = crate::ipc::Message::new(message_id, payload.to_vec(), message_type);

        // Send message with mechanism-specific handling
        if is_shm {
            // Avoid blocking forever on SHM backpressure
            match tokio::time::timeout(Duration::from_millis(Self::SHM_SEND_TIMEOUT_MS), client.send(&msg)).await {
                Ok(Ok(())) => {},
                _ => { tokio::time::sleep(Duration::from_millis(Self::SHM_RETRY_DELAY_MS)).await; }
            }
        } else {
            let _ = client.send(&msg).await; // ignore individual send errors
        }

        // Handle round-trip response
        if do_round_trip {
            if is_shm {
                let _ = tokio::time::timeout(Duration::from_millis(Self::SHM_RECEIVE_TIMEOUT_MS), client.receive()).await;
            } else {
                let _ = client.receive().await;
            }
        }

        // Record latency
        let elapsed = send_start.elapsed();
        latencies_ns.push(elapsed.as_nanos());

        // Stream per-message latency if configured (with correct mechanism)
        if let Some(rm) = stream_manager {
            let latency_type = if do_round_trip { 
                crate::metrics::LatencyType::RoundTrip 
            } else { 
                crate::metrics::LatencyType::OneWay 
            };
            let record = crate::results::MessageLatencyRecord::new(
                message_id,
                mechanism, // Use actual mechanism instead of hardcoded UDS
                self.config.message_size,
                latency_type,
                elapsed,
            );
            rm.write_streaming_record_direct(&record).await?;
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

        let connection_endpoint = config.ipc_path.clone();

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
        use crate::ipc::{IpcTransport, TransportConfig};
        let mechanism = self.config.mechanisms.first().copied().unwrap_or(crate::cli::IpcMechanism::UnixDomainSocket);

        match mechanism {
            crate::cli::IpcMechanism::UnixDomainSocket => {
                let endpoint = self
                    .connection_endpoint
                    .as_ref()
                    .ok_or_else(|| anyhow!("ipc_path must be specified in client mode for UDS"))?;
                info!("Client mode (UDS): starting server at {:?}", endpoint);
                use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
                let mut transport = UnixDomainSocketTransport::new();
                let cfg = TransportConfig {
                    socket_path: endpoint.to_string_lossy().to_string(),
                    buffer_size: self.config.buffer_size,
                    host: "127.0.0.1".to_string(),
                    port: 8080,
                    shared_memory_name: "ipc_benchmark_shm".to_string(),
                    max_connections: 16,
                    message_queue_depth: 10,
                    message_queue_name: "ipc_benchmark_pmq".to_string(),
                };
                transport.start_server(&cfg).await?;
                info!("UDS server listening at {:?}", endpoint);
                
                // Serve indefinitely to support multiple host runs
                loop {
                    match tokio::time::timeout(Duration::from_millis(50), transport.receive()).await {
                        Ok(Ok(request)) => {
                            // For UDS, send responses for round-trip requests
                            // Only avoid responses for SHM to prevent ring buffer backpressure
                            if self.config.round_trip && matches!(request.message_type, crate::ipc::MessageType::Request) {
                                let response = crate::ipc::Message::new(
                                    request.id,
                                    request.payload,
                                    crate::ipc::MessageType::Response,
                                );
                                let _ = transport.send(&response).await;
                            }
                        }
                        Ok(Err(_)) => {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        Err(_) => {
                            // Timeout; keep waiting
                        }
                    }
                }
            }
            crate::cli::IpcMechanism::SharedMemory => {
                info!("Client mode (SHM): starting shared memory server");
                use crate::ipc::shared_memory::SharedMemoryTransport;
                let shm_name = self
                    .config
                    .shm_name
                    .clone()
                    .unwrap_or_else(|| "ipc_benchmark_shm_crossenv".to_string());
                let mut transport = SharedMemoryTransport::new();
                let cfg = TransportConfig {
                    socket_path: String::new(),
                    buffer_size: self.config.buffer_size,
                    host: self.config.host.clone(),
                    port: self.config.port,
                    shared_memory_name: shm_name,
                    max_connections: 1,
                    message_queue_depth: 10,
                    message_queue_name: "ipc_benchmark_pmq".to_string(),
                };
                transport.start_server(&cfg).await?;
                info!("SHM server ready (segment created)");

                // Serve indefinitely to support multiple host runs; echo only for request-type messages
                loop {
                    match transport.receive().await {
                        Ok(request) => {
                            if matches!(request.message_type, crate::ipc::MessageType::Request | crate::ipc::MessageType::Ping) {
                                let response = crate::ipc::Message::new(
                                    request.id,
                                    request.payload,
                                    crate::ipc::MessageType::Response,
                                );
                                let _ = transport.send(&response).await;
                            }
                        }
                        Err(_) => {
                            // Brief backoff on receive error to avoid busy loop
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }
            }
            crate::cli::IpcMechanism::PosixMessageQueue => {
                info!("Client mode (PMQ): starting POSIX message queue server");
                use crate::ipc::posix_message_queue::PosixMessageQueueTransport;
                let mut transport = PosixMessageQueueTransport::new();
                let cfg = TransportConfig {
                    socket_path: String::new(),
                    buffer_size: self.config.buffer_size,
                    host: self.config.host.clone(),
                    port: self.config.port,
                    shared_memory_name: String::new(),
                    max_connections: 1,
                    message_queue_depth: 10,
                    // Fixed name to match host side
                    message_queue_name: "ipc_benchmark_pmq_crossenv".to_string(),
                };
                transport.start_server(&cfg).await?;
                info!("PMQ server ready (queue created): /{}", cfg.message_queue_name);
                // Serve indefinitely to support multiple host runs; respond if round_trip enabled
                loop {
                    match tokio::time::timeout(Duration::from_millis(50), transport.receive()).await {
                        Ok(Ok(request)) => {
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
                            // If transport errors, small backoff and continue
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        Err(_) => {
                            // Timeout: no-op, continue waiting
                        }
                    }
                }
            }
            other => Err(anyhow!("Cross-environment client not implemented for {:?}", other)),
        }
    }

    /// Wait for host connection
    async fn wait_for_host_connection(&self) -> Result<()> {
        info!("Waiting for host to initiate connection");

        // Create a UDS server that listens for connections
        use crate::ipc::{IpcTransport, TransportConfig};
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
        
        let mut transport = UnixDomainSocketTransport::new();
        let endpoint = self
            .connection_endpoint
            .as_ref()
            .ok_or_else(|| anyhow!("ipc_path must be specified in client mode for UDS"))?;
        let config = TransportConfig {
            socket_path: endpoint.to_string_lossy().to_string(),
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
