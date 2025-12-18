//! Host-Container Benchmark Runner
//!
//! This module provides the infrastructure for running IPC benchmarks between
//! a host (driver) and a Podman container (responder). The host drives the
//! tests while the container acts as the IPC server/receiver.
//!
//! # Architecture
//!
//! ```text
//! +------------------+                    +-------------------+
//! |   Host (Driver)  |  <-- IPC -->       | Container (Server)|
//! |   - Sends msgs   |                    | - Receives msgs   |
//! |   - Measures lat |                    | - Echoes back (RT)|
//! |   - Collects res |                    |                   |
//! +------------------+                    +-------------------+
//! ```
//!
//! # IPC Mechanism Configuration
//!
//! Each mechanism requires specific container setup:
//!
//! - **UDS**: Shared socket directory at `/tmp/rusty-comms`
//! - **SHM**: `--ipc=host` for shared `/dev/shm` access
//! - **PMQ**: Mounted `/dev/mqueue` and `--privileged`
//! - **TCP**: `--network=host` for direct port access
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::host_container::HostBenchmarkRunner;
//! use ipc_benchmark::cli::{Args, IpcMechanism};
//! use ipc_benchmark::benchmark::BenchmarkConfig;
//!
//! # fn example() -> anyhow::Result<()> {
//! let args = Args::default();
//! let config = BenchmarkConfig::from_args(&args)?;
//! let runner = HostBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args);
//!
//! // Run blocking benchmark with container
//! let results = runner.run_blocking()?;
//! println!("Latency: {:?}", results.one_way_latency);
//! # Ok(())
//! # }
//! ```

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

#[cfg(target_os = "linux")]
use libc;

use crate::benchmark::BenchmarkConfig;
use crate::cli::{Args, IpcMechanism};
use crate::container::{ContainerConfig, ContainerManager, UDS_SOCKET_DIR};
#[cfg(unix)]
use crate::ipc::get_monotonic_time_ns;
use crate::ipc::{
    BlockingTransport, BlockingTransportFactory, Message, MessageType, TransportConfig,
};
use crate::metrics::{LatencyType, MetricsCollector, PerformanceMetrics};
use crate::results::BenchmarkResults;
use crate::results_blocking::BlockingResultsManager;

/// Timeout for waiting for container to become ready.
const CONTAINER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Ready marker that container prints to indicate it's listening.
const CONTAINER_READY_MARKER: &str = "SERVER_READY";

/// Host-container benchmark runner.
///
/// Manages container lifecycle and drives benchmarks from the host,
/// with the container acting as the IPC server/responder.
pub struct HostBenchmarkRunner {
    /// Benchmark configuration.
    config: BenchmarkConfig,

    /// IPC mechanism to test.
    mechanism: IpcMechanism,

    /// Command-line arguments.
    args: Args,

    /// Container manager for Podman operations.
    container_manager: ContainerManager,
}

impl HostBenchmarkRunner {
    /// Create a new host benchmark runner.
    ///
    /// # Arguments
    ///
    /// * `config` - Benchmark configuration
    /// * `mechanism` - IPC mechanism to test
    /// * `args` - Command-line arguments
    ///
    /// # Returns
    ///
    /// New runner instance
    pub fn new(config: BenchmarkConfig, mechanism: IpcMechanism, args: Args) -> Self {
        let container_manager =
            ContainerManager::new(&args.container_prefix, &args.container_image);

        Self {
            config,
            mechanism,
            args,
            container_manager,
        }
    }

    /// Create transport configuration for the current mechanism.
    ///
    /// Generates unique identifiers for sockets, shared memory, etc.
    pub fn create_transport_config(&self) -> TransportConfig {
        use uuid::Uuid;

        let unique_id = Uuid::new_v4().to_string()[..8].to_string();

        let mut config = TransportConfig::default();

        // Set buffer size
        config.buffer_size = self.config.buffer_size.unwrap_or_else(|| {
            // SHM needs larger buffer for warmup iterations
            if self.mechanism == IpcMechanism::SharedMemory {
                65536 // 64KB for SHM
            } else {
                // For other mechanisms, use 2x message size or minimum 4KB
                std::cmp::max(self.config.message_size * 2, 4096)
            }
        });

        // Set mechanism-specific configuration
        match self.mechanism {
            #[cfg(unix)]
            IpcMechanism::UnixDomainSocket => {
                // Use shared socket directory accessible to both host and container
                config.socket_path = format!("{}/ipc-bench-{}.sock", UDS_SOCKET_DIR, unique_id);
            }
            IpcMechanism::SharedMemory => {
                config.shared_memory_name = format!("/ipc-bench-shm-{}", unique_id);
            }
            #[cfg(target_os = "linux")]
            IpcMechanism::PosixMessageQueue => {
                // PMQ names must start with /
                config.message_queue_name = format!("/ipc-bench-pmq-{}", unique_id);
            }
            IpcMechanism::TcpSocket => {
                config.host = "127.0.0.1".to_string();
                // Use a high port to avoid conflicts
                config.port = 40000 + (rand::random::<u16>() % 20000);
            }
            IpcMechanism::All => {
                // Should not happen - All is expanded before reaching here
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }

        config
    }

    /// Run benchmark in blocking mode with container.
    ///
    /// Spawns container as IPC server, connects from host, and runs tests.
    ///
    /// # Returns
    ///
    /// * `Ok(BenchmarkResults)` - Benchmark completed with results
    /// * `Err` - Container or benchmark failed
    pub fn run_blocking(
        &self,
        results_manager: Option<&mut BlockingResultsManager>,
    ) -> Result<BenchmarkResults> {
        let total_start = Instant::now();

        info!("Starting host-container benchmark for {}", self.mechanism);

        // Validate prerequisites
        self.validate_prerequisites()?;

        // Create transport configuration
        let transport_config = self.create_transport_config();

        // For UDS, ensure socket directory exists
        #[cfg(unix)]
        if self.mechanism == IpcMechanism::UnixDomainSocket {
            ContainerManager::ensure_socket_dir()?;
        }

        // Initialize results structure
        let mut results = BenchmarkResults::new(
            self.mechanism,
            self.config.message_size,
            transport_config.buffer_size,
            self.config.concurrency,
            self.config.msg_count,
            self.config.duration,
            self.config.warmup_iterations,
        );

        // Run warmup if configured
        if self.config.warmup_iterations > 0 {
            info!(
                "Running warmup with {} iterations (host-container)",
                self.config.warmup_iterations
            );
            self.run_warmup_blocking(&transport_config)?;
        }

        // Run one-way latency test if enabled
        if self.config.one_way {
            info!("Running one-way latency test (host-container)");
            let one_way_results =
                self.run_one_way_test_blocking(&transport_config, results_manager)?;
            results.add_one_way_results(one_way_results);
        }

        // Run round-trip latency test if enabled
        if self.config.round_trip {
            if self.mechanism == IpcMechanism::SharedMemory {
                warn!(
                    "Shared memory in blocking mode does not support bidirectional \
                     communication. Skipping round-trip test."
                );
            } else {
                info!("Running round-trip latency test (host-container)");
                let round_trip_results = self.run_round_trip_test_blocking(&transport_config)?;
                results.add_round_trip_results(round_trip_results);
            }
        }

        results.test_duration = total_start.elapsed();

        info!(
            "Host-container benchmark completed for {} mechanism",
            self.mechanism
        );
        Ok(results)
    }

    /// Validate prerequisites for host-container mode.
    fn validate_prerequisites(&self) -> Result<()> {
        // Check Podman is available
        if !ContainerManager::is_podman_available()? {
            bail!(
                "Podman is not available. Install Podman and try again.\n\
                 See PODMAN_SETUP.md for installation instructions."
            );
        }

        // Check container image exists
        if !self.container_manager.image_exists()? {
            bail!(
                "Container image '{}' not found.\n\
                 Build it with: podman build -t {} .\n\
                 See PODMAN_SETUP.md for instructions.",
                self.args.container_image,
                self.args.container_image
            );
        }

        Ok(())
    }

    /// Spawn container running as IPC server.
    ///
    /// Returns the container process and a reader for its stdout
    /// (used to wait for ready signal).
    fn spawn_container_server(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(Child, BufReader<ChildStdout>)> {
        let mut container_config = ContainerConfig::for_mechanism(&self.mechanism)?;
        container_config.name = self.container_manager.container_name(&self.mechanism);

        // Build command arguments for the container
        let mut cmd_args = vec![
            "--run-mode".to_string(),
            "client".to_string(),
            "--blocking".to_string(),
            "-m".to_string(),
            self.mechanism_cli_name(),
            "--message-size".to_string(),
            self.config.message_size.to_string(),
        ];

        // Add mechanism-specific transport configuration
        match self.mechanism {
            #[cfg(unix)]
            IpcMechanism::UnixDomainSocket => {
                cmd_args.push("--socket-path".to_string());
                cmd_args.push(transport_config.socket_path.clone());
            }
            IpcMechanism::SharedMemory => {
                cmd_args.push("--shared-memory-name".to_string());
                cmd_args.push(transport_config.shared_memory_name.clone());
                cmd_args.push("--buffer-size".to_string());
                cmd_args.push(transport_config.buffer_size.to_string());
                if self.args.shm_direct {
                    cmd_args.push("--shm-direct".to_string());
                }
            }
            #[cfg(target_os = "linux")]
            IpcMechanism::PosixMessageQueue => {
                cmd_args.push("--message-queue-name".to_string());
                cmd_args.push(transport_config.message_queue_name.clone());
            }
            IpcMechanism::TcpSocket => {
                cmd_args.push("--host".to_string());
                cmd_args.push(transport_config.host.clone());
                cmd_args.push("--port".to_string());
                cmd_args.push(transport_config.port.to_string());
            }
            _ => {}
        }

        // Add server affinity if specified
        if let Some(affinity) = self.config.server_affinity {
            cmd_args.push("--server-affinity".to_string());
            cmd_args.push(affinity.to_string());
        }

        // Build podman run command
        let mut cmd = Command::new("podman");
        cmd.args(["run", "--rm", "-i"]);

        // Add volume mounts
        for mount in &container_config.volume_mounts {
            cmd.args(["-v", mount]);
        }

        // Add extra args (e.g., --ipc=host)
        for arg in &container_config.extra_args {
            cmd.arg(arg);
        }

        // For SHM, tell container to open existing segment (created by host)
        if self.mechanism == IpcMechanism::SharedMemory {
            cmd.args(["-e", "IPC_SHM_OPEN_EXISTING=1"]);
        }

        // Add image
        cmd.arg(&self.args.container_image);

        // Add our command arguments
        cmd.args(&cmd_args);

        debug!(
            "Spawning container: podman {:?}",
            cmd.get_args().collect::<Vec<_>>()
        );

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::inherit());

        let mut child = cmd.spawn().context("Failed to spawn container process")?;

        let stdout = child
            .stdout
            .take()
            .context("Failed to capture container stdout")?;

        let reader = BufReader::new(stdout);

        Ok((child, reader))
    }

    /// Wait for container to signal it's ready.
    fn wait_for_container_ready(&self, reader: &mut BufReader<ChildStdout>) -> Result<()> {
        let start = Instant::now();
        let mut line = String::new();

        loop {
            if start.elapsed() > CONTAINER_READY_TIMEOUT {
                bail!(
                    "Container did not become ready within {:?}",
                    CONTAINER_READY_TIMEOUT
                );
            }

            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // EOF - container exited
                    bail!("Container exited before signaling ready");
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    debug!("Container output: {}", trimmed);
                    if trimmed.contains(CONTAINER_READY_MARKER) {
                        info!("Container signaled ready");
                        return Ok(());
                    }
                }
                Err(e) => {
                    bail!("Error reading from container: {}", e);
                }
            }
        }
    }

    /// Get CLI-compatible mechanism name.
    fn mechanism_cli_name(&self) -> String {
        use clap::ValueEnum;
        self.mechanism
            .to_possible_value()
            .map(|v| v.get_name().to_string())
            .unwrap_or_else(|| format!("{:?}", self.mechanism).to_lowercase())
    }

    /// Pre-create SHM segment from host for container access.
    /// Returns the Shmem handle that must be kept alive during the test.
    fn precreate_shm_segment(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<Option<shared_memory::Shmem>> {
        if self.mechanism != IpcMechanism::SharedMemory {
            return Ok(None);
        }

        // Calculate total size (must match what the transport expects)
        use crate::ipc::shared_memory_blocking::SharedMemoryRingBuffer;
        let buffer_size = transport_config.buffer_size;
        let total_size = SharedMemoryRingBuffer::HEADER_SIZE + buffer_size;

        // Clean up any existing segment
        #[cfg(target_os = "linux")]
        {
            use std::ffi::CString;
            let shm_name = if transport_config.shared_memory_name.starts_with('/') {
                transport_config.shared_memory_name.clone()
            } else {
                format!("/{}", transport_config.shared_memory_name)
            };
            if let Ok(c_name) = CString::new(shm_name.as_bytes()) {
                unsafe {
                    libc::shm_unlink(c_name.as_ptr());
                }
            }
        }

        // Create the segment
        let shmem = shared_memory::ShmemConf::new()
            .size(total_size)
            .os_id(&transport_config.shared_memory_name)
            .create()
            .with_context(|| {
                format!(
                    "Failed to pre-create SHM segment: {}",
                    transport_config.shared_memory_name
                )
            })?;

        // Initialize the ring buffer structure
        let ptr = shmem.as_ptr() as *mut SharedMemoryRingBuffer;
        unsafe {
            std::ptr::write(ptr, SharedMemoryRingBuffer::new(buffer_size));
        }
        debug!("Initialized SHM ring buffer");

        // Set permissions to 777 so container can access
        #[cfg(unix)]
        {
            let shm_file_name = transport_config
                .shared_memory_name
                .strip_prefix('/')
                .unwrap_or(&transport_config.shared_memory_name);
            let shm_path = format!("/dev/shm/{}", shm_file_name);
            if let Ok(c_path) = std::ffi::CString::new(shm_path.as_bytes()) {
                unsafe {
                    libc::chmod(c_path.as_ptr(), 0o777);
                }
            }
        }

        debug!(
            "Pre-created SHM segment for container: {}",
            transport_config.shared_memory_name
        );
        Ok(Some(shmem))
    }

    /// Run warmup iterations with container server.
    fn run_warmup_blocking(&self, transport_config: &TransportConfig) -> Result<()> {
        // For SHM, pre-create the segment from host so container can access it
        let _shm_handle = self.precreate_shm_segment(transport_config)?;

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;

        // Wait for container to be ready
        self.wait_for_container_ready(&mut reader)?;

        // Create client transport and connect
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;
        client_transport.start_client_blocking(transport_config)?;

        // Send warmup messages
        let payload = vec![0u8; self.config.message_size];
        for i in 0..self.config.warmup_iterations {
            let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
            client_transport
                .send_blocking(&message)
                .context("Failed to send warmup message")?;

            if let Some(delay) = self.config.send_delay {
                std::thread::sleep(delay);
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        debug!("Warmup completed (host-container)");
        Ok(())
    }

    /// Run one-way latency test with container server.
    fn run_one_way_test_blocking(
        &self,
        transport_config: &TransportConfig,
        _results_manager: Option<&mut BlockingResultsManager>,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;

        // For SHM, pre-create the segment from host so container can access it
        let _shm_handle = self.precreate_shm_segment(transport_config)?;

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // Create client transport and connect
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;
        client_transport.start_client_blocking(transport_config)?;

        // Set client CPU affinity if specified
        if let Some(core) = self.config.client_affinity {
            self.set_affinity(core)?;
        }

        // Send messages and collect latencies
        let payload = vec![0u8; self.config.message_size];
        let msg_count = self.config.msg_count.unwrap_or(1000);
        let mut skip_first = !self.config.include_first_message;

        for i in 0..msg_count {
            #[cfg(unix)]
            let send_time = get_monotonic_time_ns();
            #[cfg(not(unix))]
            let send_time = std::time::Instant::now();

            let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);

            // Set timestamp in message for server-side latency measurement
            #[cfg(unix)]
            let message = {
                let mut msg = message;
                msg.timestamp = send_time;
                msg
            };

            client_transport
                .send_blocking(&message)
                .with_context(|| format!("Failed to send message {}", i))?;

            // For one-way, we estimate latency as time to send (not accurate for true IPC)
            // The server records actual latency based on receive time - send timestamp
            #[cfg(unix)]
            let latency_ns = get_monotonic_time_ns() - send_time;
            #[cfg(not(unix))]
            let latency_ns = send_time.elapsed().as_nanos() as u64;

            if skip_first && i == 0 {
                skip_first = false;
                continue;
            }

            metrics_collector.record_message(
                self.config.message_size,
                Some(Duration::from_nanos(latency_ns)),
            )?;

            if let Some(delay) = self.config.send_delay {
                std::thread::sleep(delay);
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        Ok(metrics_collector.get_metrics())
    }

    /// Run round-trip latency test with container server.
    fn run_round_trip_test_blocking(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // Create client transport and connect
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;
        client_transport.start_client_blocking(transport_config)?;

        // Set client CPU affinity if specified
        if let Some(core) = self.config.client_affinity {
            self.set_affinity(core)?;
        }

        // Send messages and measure round-trip latency
        let payload = vec![0u8; self.config.message_size];
        let msg_count = self.config.msg_count.unwrap_or(1000);
        let mut skip_first = !self.config.include_first_message;

        for i in 0..msg_count {
            #[cfg(unix)]
            let send_time = get_monotonic_time_ns();
            #[cfg(not(unix))]
            let send_time = std::time::Instant::now();

            let message = Message::new(i as u64, payload.clone(), MessageType::Request);
            client_transport
                .send_blocking(&message)
                .with_context(|| format!("Failed to send request {}", i))?;

            // Wait for response
            let _response = client_transport
                .receive_blocking()
                .with_context(|| format!("Failed to receive response for request {}", i))?;

            #[cfg(unix)]
            let latency_ns = get_monotonic_time_ns() - send_time;
            #[cfg(not(unix))]
            let latency_ns = send_time.elapsed().as_nanos() as u64;

            if skip_first && i == 0 {
                skip_first = false;
                continue;
            }

            metrics_collector.record_message(
                self.config.message_size,
                Some(Duration::from_nanos(latency_ns)),
            )?;

            if let Some(delay) = self.config.send_delay {
                std::thread::sleep(delay);
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        Ok(metrics_collector.get_metrics())
    }

    /// Send shutdown message to server via the provided transport.
    fn send_shutdown_via<T: BlockingTransport + ?Sized>(&self, transport: &mut T) -> Result<()> {
        // Send shutdown for queue-based transports
        match self.mechanism {
            #[cfg(target_os = "linux")]
            IpcMechanism::PosixMessageQueue => {
                debug!("Sending shutdown message to container (PMQ)");
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                std::thread::sleep(Duration::from_millis(50));
            }
            IpcMechanism::SharedMemory => {
                debug!("Sending shutdown message to container (SHM)");
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                // Socket-based transports close on disconnect
            }
        }
        Ok(())
    }

    /// Set CPU affinity for current thread.
    fn set_affinity(&self, core_id: usize) -> Result<()> {
        let core_ids = core_affinity::get_core_ids().context("Failed to get core IDs")?;

        let core = core_ids
            .get(core_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid core ID: {}", core_id))?;

        if core_affinity::set_for_current(*core) {
            info!("Set client affinity to core {}", core_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to set affinity for core {}",
                core_id
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_runner_creation() {
        let args = Args::default();
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = HostBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        assert_eq!(runner.mechanism, IpcMechanism::TcpSocket);
    }

    #[test]
    fn test_transport_config_tcp() {
        let args = Args::default();
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = HostBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        let transport_config = runner.create_transport_config();
        assert_eq!(transport_config.host, "127.0.0.1");
        assert!(transport_config.port >= 40000);
        assert!(transport_config.port < 60000);
    }

    #[test]
    #[cfg(unix)]
    fn test_transport_config_uds() {
        let args = Args::default();
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = HostBenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket, args);

        let transport_config = runner.create_transport_config();
        assert!(transport_config.socket_path.starts_with(UDS_SOCKET_DIR));
        assert!(transport_config.socket_path.ends_with(".sock"));
    }

    #[test]
    fn test_mechanism_cli_name() {
        let args = Args::default();
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let runner =
            HostBenchmarkRunner::new(config.clone(), IpcMechanism::TcpSocket, args.clone());
        assert_eq!(runner.mechanism_cli_name(), "tcp");

        let runner =
            HostBenchmarkRunner::new(config.clone(), IpcMechanism::SharedMemory, args.clone());
        assert_eq!(runner.mechanism_cli_name(), "shm");

        #[cfg(unix)]
        {
            let runner = HostBenchmarkRunner::new(
                config.clone(),
                IpcMechanism::UnixDomainSocket,
                args.clone(),
            );
            assert_eq!(runner.mechanism_cli_name(), "uds");
        }
    }
}
