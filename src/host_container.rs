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
    TransportFactory,
};
use crate::metrics::{LatencyType, MetricsCollector, PerformanceMetrics};
use crate::results::{BenchmarkResults, ResultsManager};
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
        // For SHM, buffer must be large enough to hold at least one message with overhead.
        // Message overhead is approximately 40 bytes (serialization + ring buffer prefix).
        const MESSAGE_OVERHEAD: usize = 64; // Conservative estimate
        config.buffer_size = self.config.buffer_size.unwrap_or_else(|| {
            if self.mechanism == IpcMechanism::SharedMemory {
                // SHM buffer must hold at least one message plus some headroom.
                // Use max of 64KB default or 2x message size + overhead.
                let min_for_message = (self.config.message_size + MESSAGE_OVERHEAD) * 2;
                std::cmp::max(65536, min_for_message)
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
        mut results_manager: Option<&mut BlockingResultsManager>,
    ) -> Result<BenchmarkResults> {
        use crate::results::MessageLatencyRecord;
        use std::collections::HashMap;

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

        // Check if we should run combined test (both one-way and round-trip enabled)
        // Combined test measures both latencies for the SAME message
        let can_do_combined = self.config.one_way
            && self.config.round_trip
            && self.mechanism != IpcMechanism::SharedMemory; // SHM doesn't support bidirectional

        if can_do_combined {
            // Run combined test - both latencies measured for same message
            info!("Running combined one-way and round-trip test (host-container)");
            let (one_way_results, round_trip_results) =
                self.run_combined_test_blocking(&transport_config, results_manager.as_deref_mut())?;
            results.add_one_way_results(one_way_results);
            results.add_round_trip_results(round_trip_results);
        } else {
            // Run tests separately (original behavior for single test type or SHM)
            let mut one_way_records: HashMap<u64, MessageLatencyRecord> = HashMap::new();
            let mut round_trip_records: HashMap<u64, MessageLatencyRecord> = HashMap::new();

            // Run one-way latency test if enabled
            if self.config.one_way {
                info!("Running one-way latency test (host-container)");
                let (one_way_results, records) = self.run_one_way_test_blocking(&transport_config)?;
                results.add_one_way_results(one_way_results);
                for record in records {
                    one_way_records.insert(record.message_id, record);
                }
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
                    let (round_trip_results, records) =
                        self.run_round_trip_test_blocking(&transport_config)?;
                    results.add_round_trip_results(round_trip_results);
                    for record in records {
                        round_trip_records.insert(record.message_id, record);
                    }
                }
            }

            // Stream records if streaming is enabled (for single test mode)
            if let Some(ref mut rm) = results_manager {
                for (_, record) in one_way_records {
                    rm.stream_latency_record(&record)?;
                }
                for (_, record) in round_trip_records {
                    rm.stream_latency_record(&record)?;
                }
            }
        }

        results.test_duration = total_start.elapsed();

        info!(
            "Host-container benchmark completed for {} mechanism",
            self.mechanism
        );
        Ok(results)
    }

    /// Run benchmark in async mode with container.
    ///
    /// Spawns container as IPC server, connects from host using async transport,
    /// and runs tests with duration and streaming support.
    ///
    /// # Returns
    ///
    /// * `Ok(BenchmarkResults)` - Benchmark completed with results
    /// * `Err` - Container or benchmark failed
    pub async fn run(
        &self,
        mut results_manager: Option<&mut ResultsManager>,
    ) -> Result<BenchmarkResults> {
        use crate::results::MessageLatencyRecord;
        use std::collections::HashMap;

        let total_start = Instant::now();

        info!(
            "Starting async host-container benchmark for {}",
            self.mechanism
        );

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
                "Running warmup with {} iterations (async host-container)",
                self.config.warmup_iterations
            );
            self.run_warmup_async(&transport_config).await?;
        }

        // Check if we should run combined test (both one-way and round-trip enabled)
        // Combined test measures both latencies for the SAME message
        let can_do_combined = self.config.one_way
            && self.config.round_trip
            && self.mechanism != IpcMechanism::SharedMemory; // SHM doesn't support bidirectional

        if can_do_combined {
            // Run combined test - both latencies measured for same message
            info!("Running combined one-way and round-trip test (async host-container)");
            let (one_way_results, round_trip_results) =
                self.run_combined_test_async(&transport_config, results_manager.as_deref_mut()).await?;
            results.add_one_way_results(one_way_results);
            results.add_round_trip_results(round_trip_results);
        } else {
            // Run tests separately (original behavior for single test type or SHM)
            let mut one_way_records: HashMap<u64, MessageLatencyRecord> = HashMap::new();
            let mut round_trip_records: HashMap<u64, MessageLatencyRecord> = HashMap::new();

            // Run one-way latency test if enabled
            if self.config.one_way {
                info!("Running one-way latency test (async host-container)");
                let (one_way_results, records) = self.run_one_way_test_async(&transport_config).await?;
                results.add_one_way_results(one_way_results);
                for record in records {
                    one_way_records.insert(record.message_id, record);
                }
            }

            // Run round-trip latency test if enabled
            if self.config.round_trip {
                if self.mechanism == IpcMechanism::SharedMemory {
                    warn!(
                        "Shared memory does not support bidirectional communication. \
                         Skipping round-trip test."
                    );
                } else {
                    info!("Running round-trip latency test (async host-container)");
                    let (round_trip_results, records) =
                        self.run_round_trip_test_async(&transport_config).await?;
                    results.add_round_trip_results(round_trip_results);
                    for record in records {
                        round_trip_records.insert(record.message_id, record);
                    }
                }
            }

            // Stream records if streaming is enabled (for single test mode)
            if let Some(ref mut rm) = results_manager {
                for (_, record) in one_way_records {
                    rm.stream_latency_record(&record).await?;
                }
                for (_, record) in round_trip_records {
                    rm.stream_latency_record(&record).await?;
                }
            }
        }

        results.test_duration = total_start.elapsed();

        info!(
            "Async host-container benchmark completed for {} mechanism",
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
    ///
    /// Handles both regular SHM (ring buffer) and shm-direct modes.
    fn precreate_shm_segment(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<Option<shared_memory::Shmem>> {
        if self.mechanism != IpcMechanism::SharedMemory {
            return Ok(None);
        }

        // For shm-direct, use the dedicated precreate function
        if self.args.shm_direct {
            use crate::ipc::BlockingSharedMemoryDirect;
            let shmem = BlockingSharedMemoryDirect::precreate_segment(
                &transport_config.shared_memory_name,
            )?;
            debug!(
                "Pre-created SHM-direct segment for container: {}",
                transport_config.shared_memory_name
            );
            return Ok(Some(shmem));
        }

        // Regular SHM (ring buffer) handling
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
    ///
    /// Returns metrics and streaming records for later merging with round-trip.
    fn run_one_way_test_blocking(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(
        PerformanceMetrics,
        Vec<crate::results::MessageLatencyRecord>,
    )> {
        use crate::results::MessageLatencyRecord;

        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

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
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i, payload.clone(), MessageType::OneWay);

                #[cfg(unix)]
                let message = {
                    let mut msg = message;
                    msg.timestamp = send_time;
                    msg
                };

                match client_transport.send_blocking(&message) {
                    Ok(_) => {}
                    Err(_) => break,
                }

                #[cfg(unix)]
                let latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let latency_ns = send_time.elapsed().as_nanos() as u64;

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                let latency = Duration::from_nanos(latency_ns);
                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::OneWay,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);

                #[cfg(unix)]
                let message = {
                    let mut msg = message;
                    msg.timestamp = send_time;
                    msg
                };

                client_transport
                    .send_blocking(&message)
                    .with_context(|| format!("Failed to send message {}", i))?;

                #[cfg(unix)]
                let latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let latency_ns = send_time.elapsed().as_nanos() as u64;

                if skip_first && i == 0 {
                    skip_first = false;
                    continue;
                }

                let latency = Duration::from_nanos(latency_ns);
                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::OneWay,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        Ok((metrics_collector.get_metrics(), streaming_records))
    }

    /// Run round-trip latency test with container server.
    ///
    /// Returns metrics and streaming records for later merging with one-way.
    fn run_round_trip_test_blocking(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(
        PerformanceMetrics,
        Vec<crate::results::MessageLatencyRecord>,
    )> {
        use crate::results::MessageLatencyRecord;

        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

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
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i, payload.clone(), MessageType::Request);
                match client_transport.send_blocking(&message) {
                    Ok(_) => {}
                    Err(_) => break,
                }

                match client_transport.receive_blocking() {
                    Ok(_) => {}
                    Err(_) => break,
                }

                #[cfg(unix)]
                let latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let latency_ns = send_time.elapsed().as_nanos() as u64;

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                let latency = Duration::from_nanos(latency_ns);
                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i as u64, payload.clone(), MessageType::Request);
                client_transport
                    .send_blocking(&message)
                    .with_context(|| format!("Failed to send request {}", i))?;

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

                let latency = Duration::from_nanos(latency_ns);
                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        Ok((metrics_collector.get_metrics(), streaming_records))
    }

    /// Run combined one-way and round-trip test with container server.
    ///
    /// This method measures BOTH one-way and round-trip latencies for the SAME message,
    /// ensuring that streaming output has correct paired latencies. For each message:
    /// 1. Record send start time
    /// 2. Send message → one-way latency = time to send
    /// 3. Receive response → round-trip latency = total time
    ///
    /// Returns tuple of (one_way_metrics, round_trip_metrics) and streams records directly.
    fn run_combined_test_blocking(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<(PerformanceMetrics, PerformanceMetrics)> {
        use crate::results::MessageLatencyRecord;

        let mut one_way_metrics = MetricsCollector::new(
            Some(LatencyType::OneWay),
            self.config.percentiles.clone(),
        )?;
        let mut round_trip_metrics = MetricsCollector::new(
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

        // Send messages and measure BOTH latencies for each message
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Collect records to stream after test completes
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i, payload.clone(), MessageType::Request);
                match client_transport.send_blocking(&message) {
                    Ok(_) => {}
                    Err(_) => break,
                }

                // Capture one-way latency (time to send)
                #[cfg(unix)]
                let one_way_latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let one_way_latency_ns = send_time.elapsed().as_nanos() as u64;

                match client_transport.receive_blocking() {
                    Ok(_) => {}
                    Err(_) => break,
                }

                // Capture round-trip latency (total time)
                #[cfg(unix)]
                let round_trip_latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let round_trip_latency_ns = send_time.elapsed().as_nanos() as u64;

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                let one_way_latency = Duration::from_nanos(one_way_latency_ns);
                let round_trip_latency = Duration::from_nanos(round_trip_latency_ns);

                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;

                // Create combined record with both latencies
                streaming_records.push(MessageLatencyRecord::new_combined(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    one_way_latency,
                    round_trip_latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                #[cfg(unix)]
                let send_time = get_monotonic_time_ns();
                #[cfg(not(unix))]
                let send_time = std::time::Instant::now();

                let message = Message::new(i as u64, payload.clone(), MessageType::Request);
                client_transport
                    .send_blocking(&message)
                    .with_context(|| format!("Failed to send request {}", i))?;

                // Capture one-way latency (time to send)
                #[cfg(unix)]
                let one_way_latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let one_way_latency_ns = send_time.elapsed().as_nanos() as u64;

                let _response = client_transport
                    .receive_blocking()
                    .with_context(|| format!("Failed to receive response for request {}", i))?;

                // Capture round-trip latency (total time)
                #[cfg(unix)]
                let round_trip_latency_ns = get_monotonic_time_ns() - send_time;
                #[cfg(not(unix))]
                let round_trip_latency_ns = send_time.elapsed().as_nanos() as u64;

                if skip_first && i == 0 {
                    skip_first = false;
                    continue;
                }

                let one_way_latency = Duration::from_nanos(one_way_latency_ns);
                let round_trip_latency = Duration::from_nanos(round_trip_latency_ns);

                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;

                // Create combined record with both latencies
                streaming_records.push(MessageLatencyRecord::new_combined(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    one_way_latency,
                    round_trip_latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        // Send shutdown message
        self.send_shutdown_via(client_transport.as_mut())?;

        // Cleanup
        client_transport.close_blocking()?;
        let _ = container.wait();

        // Stream records if manager provided
        if let Some(rm) = results_manager {
            for record in &streaming_records {
                rm.stream_latency_record(record)?;
            }
        }

        Ok((one_way_metrics.get_metrics(), round_trip_metrics.get_metrics()))
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

    // ==================== ASYNC METHODS ====================
    //
    // Note: In host-container async mode, the container always runs with --blocking,
    // so it uses blocking transports. For PMQ and SHM, the blocking and async transports
    // have incompatible designs (PMQ: two-queue vs single-queue, SHM: precreate pattern).
    // Therefore, for these mechanisms, we use blocking transports on the host side too.
    // UDS and TCP async transports are compatible with their blocking counterparts.

    /// Check if we need to use blocking transport for host-container compatibility.
    fn needs_blocking_transport(&self) -> bool {
        matches!(
            self.mechanism,
            IpcMechanism::SharedMemory | IpcMechanism::PosixMessageQueue
        )
    }

    /// Run warmup with container server (async version).
    async fn run_warmup_async(&self, transport_config: &TransportConfig) -> Result<()> {
        debug!("Running async warmup (host-container)");

        // For SHM, pre-create the segment from host so container can access it
        let _shm_handle = self.precreate_shm_segment(transport_config)?;

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // For PMQ and SHM, use blocking transport for host-container compatibility
        if self.needs_blocking_transport() {
            let mut client_transport =
                BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;
            client_transport.start_client_blocking(transport_config)?;

            let payload = vec![0u8; self.config.message_size];
            for i in 0..self.config.warmup_iterations {
                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
                if client_transport.send_blocking(&message).is_err() {
                    break;
                }
            }

            // Send shutdown message
            self.send_shutdown_via(client_transport.as_mut())?;
            client_transport.close_blocking()?;
        } else {
            // Create async client transport and connect
            let mut client_transport = TransportFactory::create(&self.mechanism)?;
            client_transport.start_client(transport_config).await?;

            // Send warmup messages
            let payload = vec![0u8; self.config.message_size];
            let warmup_count = self.config.warmup_iterations;

            for i in 0..warmup_count {
                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
                if client_transport.send(&message).await.is_err() {
                    break;
                }
            }

            // Cleanup
            client_transport.close().await?;
        }

        let _ = container.wait();
        debug!("Async warmup completed (host-container)");
        Ok(())
    }

    /// Run one-way latency test with container server (async version).
    ///
    /// Returns metrics and streaming records for later merging with round-trip.
    async fn run_one_way_test_async(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(
        PerformanceMetrics,
        Vec<crate::results::MessageLatencyRecord>,
    )> {
        use crate::results::MessageLatencyRecord;

        // For PMQ and SHM, use blocking transport for compatibility
        if self.needs_blocking_transport() {
            return self.run_one_way_test_blocking(transport_config);
        }

        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // Create async client transport and connect
        let mut client_transport = TransportFactory::create(&self.mechanism)?;
        client_transport.start_client(transport_config).await?;

        // Set client CPU affinity if specified
        if let Some(core) = self.config.client_affinity {
            self.set_affinity(core)?;
        }

        // Send messages and collect latencies
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_time = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::OneWay);

                match client_transport.send(&message).await {
                    Ok(_) => {}
                    Err(_) => break,
                }

                let latency = send_time.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::OneWay,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                let send_time = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);

                client_transport.send(&message).await?;

                let latency = send_time.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    continue;
                }

                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::OneWay,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Cleanup
        client_transport.close().await?;
        let _ = container.wait();

        Ok((metrics_collector.get_metrics(), streaming_records))
    }

    /// Run round-trip latency test with container server (async version).
    ///
    /// Returns metrics and streaming records for later merging with one-way.
    async fn run_round_trip_test_async(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(
        PerformanceMetrics,
        Vec<crate::results::MessageLatencyRecord>,
    )> {
        use crate::results::MessageLatencyRecord;

        // For PMQ and SHM, use blocking transport for compatibility
        // Note: SHM doesn't support round-trip anyway, but this keeps it consistent
        if self.needs_blocking_transport() {
            return self.run_round_trip_test_blocking(transport_config);
        }

        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // Create async client transport and connect
        let mut client_transport = TransportFactory::create(&self.mechanism)?;
        client_transport.start_client(transport_config).await?;

        // Set client CPU affinity if specified
        if let Some(core) = self.config.client_affinity {
            self.set_affinity(core)?;
        }

        // Send messages and measure round-trip latency
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_time = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::Request);

                match client_transport.send(&message).await {
                    Ok(_) => {}
                    Err(_) => break,
                }

                match client_transport.receive().await {
                    Ok(_) => {}
                    Err(_) => break,
                }

                let latency = send_time.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                let send_time = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);

                client_transport.send(&message).await?;
                client_transport.receive().await?;

                let latency = send_time.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    continue;
                }

                metrics_collector.record_message(self.config.message_size, Some(latency))?;
                streaming_records.push(MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Cleanup
        client_transport.close().await?;
        let _ = container.wait();

        Ok((metrics_collector.get_metrics(), streaming_records))
    }

    /// Run combined one-way and round-trip test with container server (async).
    ///
    /// This method measures BOTH one-way and round-trip latencies for the SAME message,
    /// ensuring that streaming output has correct paired latencies. For each message:
    /// 1. Record send start time
    /// 2. Send message → one-way latency = time to send
    /// 3. Receive response → round-trip latency = total time
    ///
    /// Returns tuple of (one_way_metrics, round_trip_metrics) and streams records directly.
    async fn run_combined_test_async(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<(PerformanceMetrics, PerformanceMetrics)> {
        use crate::results::MessageLatencyRecord;

        // For PMQ and SHM, use blocking transport for compatibility
        if self.needs_blocking_transport() {
            // SHM doesn't support combined test anyway (no bidirectional)
            // PMQ needs blocking - fall back to blocking combined test
            return self.run_combined_test_blocking_wrapper(transport_config, results_manager);
        }

        let mut one_way_metrics = MetricsCollector::new(
            Some(LatencyType::OneWay),
            self.config.percentiles.clone(),
        )?;
        let mut round_trip_metrics = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Spawn container as server
        let (mut container, mut reader) = self.spawn_container_server(transport_config)?;
        self.wait_for_container_ready(&mut reader)?;

        // Create async client transport and connect
        let mut client_transport = TransportFactory::create(&self.mechanism)?;
        client_transport.start_client(transport_config).await?;

        // Send messages and measure BOTH latencies for each message
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();
        let mut skip_first = !self.config.include_first_message;

        // Collect records to stream after test completes
        let mut streaming_records: Vec<MessageLatencyRecord> = Vec::new();

        // Duration mode vs message count mode
        if let Some(duration) = self.config.duration {
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_start = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::Request);

                match client_transport.send(&message).await {
                    Ok(_) => {}
                    Err(_) => break,
                }

                // Capture one-way latency (time to send)
                let one_way_latency = send_start.elapsed();

                match client_transport.receive().await {
                    Ok(_) => {}
                    Err(_) => break,
                }

                // Capture round-trip latency (total time)
                let round_trip_latency = send_start.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    i += 1;
                    continue;
                }

                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;

                // Create combined record with both latencies
                streaming_records.push(MessageLatencyRecord::new_combined(
                    i,
                    self.mechanism,
                    self.config.message_size,
                    one_way_latency,
                    round_trip_latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                i += 1;
            }
        } else {
            let msg_count = self.config.msg_count.unwrap_or(1000);
            for i in 0..msg_count {
                let send_start = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);

                client_transport
                    .send(&message)
                    .await
                    .with_context(|| format!("Failed to send request {}", i))?;

                // Capture one-way latency (time to send)
                let one_way_latency = send_start.elapsed();

                let _response = client_transport
                    .receive()
                    .await
                    .with_context(|| format!("Failed to receive response for request {}", i))?;

                // Capture round-trip latency (total time)
                let round_trip_latency = send_start.elapsed();

                if skip_first && i == 0 {
                    skip_first = false;
                    continue;
                }

                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;

                // Create combined record with both latencies
                streaming_records.push(MessageLatencyRecord::new_combined(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    one_way_latency,
                    round_trip_latency,
                ));

                if let Some(delay) = self.config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Cleanup
        client_transport.close().await?;
        let _ = container.wait();

        // Stream records if manager provided
        if let Some(rm) = results_manager {
            for record in &streaming_records {
                rm.write_streaming_record_direct(record).await?;
            }
        }

        Ok((one_way_metrics.get_metrics(), round_trip_metrics.get_metrics()))
    }

    /// Wrapper to call blocking combined test from async context.
    ///
    /// Used for PMQ which needs blocking transport even in async mode.
    fn run_combined_test_blocking_wrapper(
        &self,
        transport_config: &TransportConfig,
        _results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<(PerformanceMetrics, PerformanceMetrics)> {
        // For PMQ, we need to run the blocking version
        // Note: We can't easily pass the async ResultsManager to blocking code,
        // so streaming will be handled separately for PMQ
        self.run_combined_test_blocking(transport_config, None)
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
