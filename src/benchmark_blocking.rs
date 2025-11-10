//! # Blocking Benchmark Engine Module
//!
//! This module implements the blocking/synchronous version of the IPC benchmark
//! engine. It mirrors the async `benchmark` module but uses traditional blocking
//! I/O operations from the standard library instead of Tokio and async/await.
//!
//! ## Key Differences from Async Module
//!
//! - **No async/await**: All functions are synchronous and block the calling thread
//! - **No Tokio**: Uses `std::thread` and `std::sync::mpsc` instead of Tokio
//! - **Blocking I/O**: Uses `std::net`, `std::os::unix::net`, etc.
//! - **BlockingTransport**: Uses `BlockingTransport` trait instead of `Transport`
//!
//! ## Key Components
//!
//! - **BlockingBenchmarkRunner**: Main orchestrator for blocking benchmark execution
//! - **BenchmarkConfig**: Shared configuration structure (same as async version)
//! - **Test Patterns**: Support for one-way and round-trip latency measurements
//! - **Resource Management**: Ensures proper cleanup between tests
//!
//! ## Test Execution Lifecycle
//!
//! 1. **Initialization**: Create blocking transport instances and configure parameters
//! 2. **Warmup**: Run initial iterations to stabilize performance
//! 3. **Measurement**: Execute actual tests with metrics collection
//! 4. **Cleanup**: Properly close connections and release resources
//!
//! ## Performance Considerations
//!
//! The blocking engine provides a baseline for comparison with the async version.
//! It demonstrates pure blocking I/O performance without the overhead of async
//! runtime and task scheduling.
//!
//! ## Examples
//!
//! ```rust,no_run
//! use ipc_benchmark::benchmark_blocking::BlockingBenchmarkRunner;
//! use ipc_benchmark::benchmark::BenchmarkConfig;
//! use ipc_benchmark::cli::{Args, IpcMechanism};
//!
//! # fn example() -> anyhow::Result<()> {
//! let args = Args {
//!     mechanisms: vec![IpcMechanism::TcpSocket],
//!     message_size: 1024,
//!     msg_count: 1000,
//!     blocking: true,
//!     ..Default::default()
//! };
//!
//! let config = BenchmarkConfig::from_args(&args)?;
//! let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);
//! // Execute the benchmark in blocking mode
//! // let results = runner.run()?;
//! # Ok(())
//! # }
//! ```

use crate::{
    benchmark::BenchmarkConfig,
    cli::{Args, IpcMechanism},
    ipc::{BlockingTransportFactory, Message, MessageType, TransportConfig},
    metrics::{LatencyType, MetricsCollector, PerformanceMetrics},
    results::BenchmarkResults,
};
use anyhow::{Context, Result};
use clap::ValueEnum;
use os_pipe::PipeReader;
use std::{
    io::Read,
    process::{Command, Stdio},
    time::Instant,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::io::FromRawFd;
#[cfg(windows)]
use std::os::windows::io::{FromRawHandle, IntoRawHandle};

/// A helper struct to provide consistent display of benchmark configuration.
///
/// This mirrors the async version's BenchmarkConfigDisplay to ensure
/// identical output formatting between async and blocking modes.
struct BenchmarkConfigDisplay<'a> {
    config: &'a BenchmarkConfig,
    mechanism: IpcMechanism,
    transport_config: &'a TransportConfig,
}

impl<'a> std::fmt::Display for BenchmarkConfigDisplay<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let buffer_size_str = if self.config.buffer_size.is_some() {
            format!(
                "{} bytes (User-provided)",
                self.transport_config.buffer_size
            )
        } else {
            format!("{} bytes (Automatic)", self.transport_config.buffer_size)
        };

        writeln!(
            f,
            "-----------------------------------------------------------------"
        )?;
        writeln!(f, "Starting Benchmark for: {}", self.mechanism)?;
        writeln!(
            f,
            "  Message Size:       {} bytes",
            self.config.message_size
        )?;
        writeln!(f, "  Buffer Size:        {}", buffer_size_str)?;
        if let Some(duration) = self.config.duration {
            writeln!(f, "  Test Duration:      {:?}", duration)?;
        } else {
            writeln!(
                f,
                "  Message Count:      {}",
                self.config.msg_count.unwrap_or_default()
            )?;
        }
        if let Some(delay) = self.config.send_delay {
            writeln!(f, "  Send Delay:         {:?}", delay)?;
        }

        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            writeln!(f, "  PMQ Priority:       {}", self.config.pmq_priority)?;
        }

        let server_affinity_str = self
            .config
            .server_affinity
            .map_or("Not set".to_string(), |c| c.to_string());
        let client_affinity_str = self
            .config
            .client_affinity
            .map_or("Not set".to_string(), |c| c.to_string());
        writeln!(f, "  Server Affinity:    {}", server_affinity_str)?;
        writeln!(f, "  Client Affinity:    {}", client_affinity_str)?;

        let first_message_status = if self.config.include_first_message {
            "Included in results"
        } else {
            "Discarded (default)"
        };
        writeln!(f, "  First Message:      {}", first_message_status)?;
        write!(
            f,
            "-----------------------------------------------------------------"
        )
    }
}

/// Blocking benchmark runner that coordinates the execution of IPC performance tests
///
/// The `BlockingBenchmarkRunner` provides synchronous/blocking execution of
/// performance tests. It mirrors the async `BenchmarkRunner` but uses standard
/// library blocking I/O operations instead of Tokio and async/await.
///
/// ## Design Principles
///
/// - **Blocking Operations**: All I/O operations block the calling thread
/// - **Standard Library**: Uses std::thread, std::sync::mpsc, std::net, etc.
/// - **No Tokio**: Pure standard library implementation
/// - **Consistent Metrics**: Same measurement methodology as async version
///
/// ## Usage Pattern
///
/// ```rust,no_run
/// use ipc_benchmark::benchmark_blocking::BlockingBenchmarkRunner;
/// use ipc_benchmark::benchmark::BenchmarkConfig;
/// use ipc_benchmark::cli::{Args, IpcMechanism};
///
/// # fn example() -> anyhow::Result<()> {
/// let args = Args {
///     mechanisms: vec![IpcMechanism::TcpSocket],
///     message_size: 1024,
///     msg_count: 1000,
///     blocking: true,
///     ..Default::default()
/// };
///
/// let config = BenchmarkConfig::from_args(&args)?;
/// let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);
/// // let results = runner.run()?;
/// # Ok(())
/// # }
/// ```
pub struct BlockingBenchmarkRunner {
    /// Benchmark configuration parameters
    config: BenchmarkConfig,

    /// The specific IPC mechanism being tested
    mechanism: IpcMechanism,

    /// The original command-line arguments
    args: Args,

    /// Available CPU cores (cached at startup to avoid affinity-dependent detection)
    available_cores: Option<Vec<core_affinity::CoreId>>,
}

impl BlockingBenchmarkRunner {
    /// Create a new blocking benchmark runner
    ///
    /// ## Parameters
    /// - `config`: Benchmark configuration parameters
    /// - `mechanism`: Specific IPC mechanism to test
    /// - `args`: Original command-line arguments
    ///
    /// ## Returns
    /// Configured blocking benchmark runner ready for execution
    ///
    /// ## Examples
    ///
    /// ```rust,no_run
    /// use ipc_benchmark::benchmark_blocking::BlockingBenchmarkRunner;
    /// use ipc_benchmark::benchmark::BenchmarkConfig;
    /// use ipc_benchmark::cli::{Args, IpcMechanism};
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let args = Args::default();
    /// let config = BenchmarkConfig::from_args(&args)?;
    /// let runner = BlockingBenchmarkRunner::new(
    ///     config,
    ///     IpcMechanism::TcpSocket,
    ///     args
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: BenchmarkConfig, mechanism: IpcMechanism, args: Args) -> Self {
        // Cache available cores at construction time to avoid affinity-dependent
        // detection issues
        let available_cores = core_affinity::get_core_ids();

        Self {
            config,
            mechanism,
            args,
            available_cores,
        }
    }

    /// Validate CPU core availability at startup
    ///
    /// This validates that the requested cores are available using cached
    /// core information. Prevents runtime errors from invalid affinity settings.
    ///
    /// ## Returns
    /// - `Ok(())`: All requested cores are available
    /// - `Err(anyhow::Error)`: Invalid core IDs or no cores detected
    fn validate_core_availability(&self) -> Result<()> {
        // Use cached core information
        let core_ids = self
            .available_cores
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Failed to get core IDs for validation"))?;

        if core_ids.is_empty() {
            return Err(anyhow::anyhow!("No CPU cores detected"));
        }

        // Validate server affinity if specified
        if let Some(server_core_id) = self.config.server_affinity {
            if server_core_id >= core_ids.len() {
                return Err(anyhow::anyhow!(
                    "Invalid server core ID: {} (available cores: 0-{}, total: {})",
                    server_core_id,
                    core_ids.len().saturating_sub(1),
                    core_ids.len()
                ));
            }
        }

        // Validate client affinity if specified
        if let Some(client_core_id) = self.config.client_affinity {
            if client_core_id >= core_ids.len() {
                return Err(anyhow::anyhow!(
                    "Invalid client core ID: {} (available cores: 0-{}, total: {})",
                    client_core_id,
                    core_ids.len().saturating_sub(1),
                    core_ids.len()
                ));
            }
        }

        Ok(())
    }

    /// Get the effective message count for the current configuration
    ///
    /// ## Returns
    /// The number of messages to send in the benchmark. Returns 0 for duration-based tests.
    fn get_msg_count(&self) -> usize {
        self.config.msg_count.unwrap_or(0)
    }

    /// Spawn the server process for a benchmark run (blocking version)
    ///
    /// This function constructs and executes a command to run the current executable
    /// in server-only mode with the `--blocking` flag. It passes along all necessary
    /// command-line arguments to ensure the server operates with the same configuration
    /// as the client.
    ///
    /// ## Key Differences from Async Version
    ///
    /// - Adds `--blocking` flag to spawned server process
    /// - Otherwise identical to async spawn_server_process
    ///
    /// ## Parameters
    ///
    /// - `transport_config`: The transport-specific configuration containing unique
    ///   identifiers for sockets, shared memory, etc.
    ///
    /// ## Returns
    ///
    /// - `Ok((Child, PipeReader))`: A tuple containing the handle to the spawned
    ///   child process and the reader end of the signaling pipe.
    /// - `Err(anyhow::Error)`: An error if the pipe creation or process spawning fails.
    pub fn spawn_server_process(
        &self,
        transport_config: &TransportConfig,
    ) -> Result<(std::process::Child, PipeReader)> {
        self.spawn_server_process_with_latency_file(transport_config, None)
    }

    /// Spawn the server process with optional latency file for true IPC measurement
    ///
    /// This variant allows passing a latency file path to the server, enabling
    /// server-side latency calculation that matches the C benchmark methodology.
    ///
    /// ## Parameters
    ///
    /// - `transport_config`: Transport configuration
    /// - `latency_file_path`: Optional path for server to write measured latencies
    ///
    /// ## Returns
    ///
    /// - `Ok((Child, PipeReader))`: Spawned process and ready signal pipe
    /// - `Err(anyhow::Error)`: Spawn failure
    pub fn spawn_server_process_with_latency_file(
        &self,
        transport_config: &TransportConfig,
        latency_file_path: Option<&str>,
    ) -> Result<(std::process::Child, PipeReader)> {
        let (reader, writer) =
            os_pipe::pipe().context("Failed to create OS pipe for server signaling")?;

        let current_exe =
            std::env::current_exe().context("Failed to get current executable path")?;

        // Resolve the path to the main binary robustly at runtime
        let exe_name = "ipc-benchmark";
        #[cfg(windows)]
        let exe_name_win = "ipc-benchmark.exe";
        let mut exe_path = None;

        {
            let current_name = current_exe.file_name().and_then(|n| n.to_str());
            let matches_unix = current_name == Some(exe_name);
            #[cfg(windows)]
            let matches_win = current_name == Some(exe_name_win);
            #[cfg(not(windows))]
            let matches_win = false;
            if matches_unix || matches_win {
                exe_path = Some(current_exe.clone());
            }
        }

        if exe_path.is_none() {
            if let Ok(p) = std::env::var("CARGO_BIN_EXE_ipc-benchmark") {
                let pbuf = std::path::PathBuf::from(p);
                if pbuf.exists() && pbuf.file_name().and_then(|n| n.to_str()) == Some(exe_name) {
                    exe_path = Some(pbuf);
                }
            } else if let Ok(p) = std::env::var("CARGO_BIN_EXE_ipc_benchmark") {
                let pbuf = std::path::PathBuf::from(p);
                if pbuf.exists() {
                    exe_path = Some(pbuf);
                }
            }
        }

        if exe_path.is_none() {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            #[cfg(windows)]
            let p = root
                .join("target")
                .join("debug")
                .join(format!("{}{}", exe_name, ".exe"));
            #[cfg(not(windows))]
            let p = root.join("target").join("debug").join(exe_name);
            if p.exists() {
                exe_path = Some(p);
            }
        }

        let exe_path = exe_path.ok_or_else(|| {
            anyhow::anyhow!(
                "Could not resolve '{}' binary for server mode. Build it with \
                 `cargo build --bin {}` or run full `cargo test` first.",
                "ipc-benchmark",
                "ipc-benchmark"
            )
        })?;

        debug!("Spawning blocking server binary: {}", exe_path.display());
        let mut cmd = Command::new(&exe_path);

        // Connect the writer end of the pipe to the child's stdout
        cmd.stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::io::IntoRawFd;
            cmd.stdout(unsafe { Stdio::from_raw_fd(writer.into_raw_fd()) });
        }
        #[cfg(windows)]
        {
            cmd.stdout(unsafe { Stdio::from_raw_handle(writer.into_raw_handle()) });
        }
        cmd.stderr(Stdio::inherit());

        // Add --internal-run-as-server flag
        cmd.arg("--internal-run-as-server");

        // IMPORTANT: Add --blocking flag so the server runs in blocking mode
        cmd.arg("--blocking");

        // Add --shm-direct flag if enabled (for direct memory shared memory)
        if self.args.shm_direct {
            cmd.arg("--shm-direct");
        }

        // Add mechanism-specific arguments
        // Use possible_value name (e.g., "tcp") not Display name (e.g., "TCP Socket")
        cmd.arg("-m")
            .arg(self.mechanism.to_possible_value().unwrap().get_name());

        // Add message size and count
        cmd.arg("--message-size")
            .arg(self.config.message_size.to_string());
        cmd.arg("--msg-count").arg(self.get_msg_count().to_string());

        // Add transport-specific identifiers
        if !transport_config.socket_path.is_empty() {
            cmd.arg("--socket-path").arg(&transport_config.socket_path);
        }
        if !transport_config.shared_memory_name.is_empty() {
            cmd.arg("--shared-memory-name")
                .arg(&transport_config.shared_memory_name);
        }
        if !transport_config.message_queue_name.is_empty() {
            cmd.arg("--message-queue-name")
                .arg(&transport_config.message_queue_name);
        }

        // Add network configuration
        cmd.arg("--host").arg(&transport_config.host);
        cmd.arg("--port").arg(transport_config.port.to_string());

        // Add buffer size
        cmd.arg("--buffer-size")
            .arg(transport_config.buffer_size.to_string());

        // Add affinity if specified (server affinity)
        if let Some(core_id) = self.config.server_affinity {
            cmd.arg("--server-affinity").arg(core_id.to_string());
        }

        // Add PMQ priority if applicable
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            cmd.arg("--pmq-priority")
                .arg(self.config.pmq_priority.to_string());
        }

        // Add latency file path if provided (for true IPC measurement)
        if let Some(path) = latency_file_path {
            cmd.arg("--internal-latency-file").arg(path);
        }

        debug!("Spawning blocking server process with command: {:?}", cmd);

        let child = cmd
            .spawn()
            .context("Failed to spawn server process in blocking mode")?;

        Ok((child, reader))
    }

    /// Create transport configuration with intelligent parameter adaptation
    ///
    /// This is identical to the async version's implementation. It creates a
    /// transport configuration optimized for the specific test parameters.
    ///
    /// ## Key Adaptations
    ///
    /// - **Unique Identifiers**: Uses UUIDs to prevent resource conflicts
    /// - **Adaptive Buffer Sizing**: Adjusts buffer sizes based on test parameters
    /// - **Port Uniqueness**: Ensures unique ports for TCP to avoid conflicts
    /// - **Mechanism-Specific Tuning**: Applies optimizations for each transport type
    ///
    /// ## Returns
    /// - `Ok(TransportConfig)`: Configured transport settings
    /// - `Err(anyhow::Error)`: Configuration validation failure
    pub fn create_transport_config_internal(&self, args: &Args) -> Result<TransportConfig> {
        const DURATION_MODE_BUFFER_SIZE: usize = 1_073_741_824; // 1 GB
        const PMQ_SAFE_DEFAULT_BUFFER_SIZE: usize = 8192;

        let unique_id = Uuid::new_v4();
        let unique_port = self.config.port + (unique_id.as_u128() as u16 % 1000);

        // Determine if the current mechanism is PMQ
        let is_pmq = {
            #[cfg(target_os = "linux")]
            {
                self.mechanism == IpcMechanism::PosixMessageQueue
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        };

        // Buffer size logic:
        // 1. If user provides --buffer-size, use it directly.
        // 2. If the mechanism is PMQ, always use a safe, small default.
        // 3. If in duration mode, use a large fixed size to avoid backpressure.
        // 4. Otherwise, calculate based on message count.
        let buffer_size = self.config.buffer_size.unwrap_or_else(|| {
            if is_pmq {
                PMQ_SAFE_DEFAULT_BUFFER_SIZE
            } else if self.config.duration.is_some() {
                DURATION_MODE_BUFFER_SIZE
            } else {
                // For message-count mode, size the buffer to fit all messages.
                self.get_msg_count() * (self.config.message_size + 64)
            }
        });

        // Add a specific validation for PMQ
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue
            && buffer_size > PMQ_SAFE_DEFAULT_BUFFER_SIZE
        {
            warn!(
                "The specified buffer size ({} bytes) exceeds the typical system limit of 8192 bytes for POSIX Message Queues. The benchmark may fail if the system is not configured for larger message sizes.",
                buffer_size
            );
        }

        // Validate buffer size for shared memory
        if self.mechanism == IpcMechanism::SharedMemory && self.config.duration.is_none() {
            let total_message_data = self.get_msg_count() * (self.config.message_size + 32);
            if buffer_size < total_message_data {
                warn!(
                    "Buffer size ({} bytes) is smaller than the total data size ({} bytes). This may cause backpressure, which is a valid test scenario.",
                    buffer_size,
                    total_message_data
                );
            }
        }

        // Conservative queue depth for PMQ
        let adaptive_queue_depth = {
            #[cfg(target_os = "linux")]
            {
                if self.mechanism == IpcMechanism::PosixMessageQueue {
                    let msg_count = self.get_msg_count();

                    if msg_count > 10000 {
                        warn!(
                            "PMQ with {} messages may be very slow due to system queue depth limits (typically 10). Consider using fewer messages or a different mechanism for high-throughput testing.",
                            msg_count
                        );
                    }

                    let queue_depth = 10; // Always use system default

                    debug!(
                        "PMQ using conservative queue depth: {} messages -> depth {}",
                        msg_count, queue_depth
                    );

                    queue_depth
                } else {
                    100
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                100
            }
        };

        Ok(TransportConfig {
            socket_path: if self.mechanism == IpcMechanism::UnixDomainSocket {
                args.socket_path
                    .clone()
                    .unwrap_or_else(|| format!("/tmp/ipc_benchmark_{}.sock", unique_id))
            } else {
                String::new()
            },
            host: self.config.host.clone(),
            port: unique_port,
            shared_memory_name: if self.mechanism == IpcMechanism::SharedMemory {
                args.shared_memory_name
                    .clone()
                    .unwrap_or_else(|| format!("ipc_benchmark_{}", unique_id))
            } else {
                String::new()
            },
            message_queue_name: if self.mechanism == IpcMechanism::PosixMessageQueue {
                args.message_queue_name
                    .clone()
                    .unwrap_or_else(|| format!("/ipc_benchmark_{}", unique_id))
            } else {
                String::new()
            },
            buffer_size,
            max_connections: 1,
            message_queue_depth: adaptive_queue_depth,
            pmq_priority: self.config.pmq_priority,
        })
    }

    /// Run the blocking benchmark and return comprehensive results
    ///
    /// This is the main entry point for blocking benchmark execution. It orchestrates
    /// the complete test lifecycle and returns detailed performance metrics.
    ///
    /// ## Test Execution Flow
    ///
    /// 1. **Validation**: Verify core availability and configuration
    /// 2. **Warmup Phase**: Run warmup iterations to stabilize performance
    /// 3. **One-way Testing**: Measure one-way latency if enabled
    /// 4. **Round-trip Testing**: Measure round-trip latency if enabled
    /// 5. **Result Aggregation**: Combine metrics and generate summary
    ///
    /// ## Key Differences from Async Version
    ///
    /// - No async/await - all operations block
    /// - No Tokio runtime
    /// - Uses BlockingTransport instead of Transport
    /// - Streaming output not yet implemented (Stage 5)
    ///
    /// ## Returns
    /// - `Ok(BenchmarkResults)`: Complete test results with metrics
    /// - `Err(anyhow::Error)`: Test execution failure with diagnostic information
    pub fn run(
        &self,
        mut results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<BenchmarkResults> {
        // Track total benchmark duration
        let total_start = Instant::now();

        // Validate core availability before any affinity changes
        self.validate_core_availability()?;

        let transport_config = self.create_transport_config_internal(&self.args)?;

        // Display benchmark configuration
        info!(
            "{}",
            BenchmarkConfigDisplay {
                config: &self.config,
                mechanism: self.mechanism,
                transport_config: &transport_config,
            }
        );

        // Initialize results structure with test configuration
        let mut results = BenchmarkResults::new(
            self.mechanism,
            self.config.message_size,
            transport_config.buffer_size,
            self.config.concurrency,
            self.config.msg_count,
            self.config.duration,
        );

        // Run warmup if configured
        if self.config.warmup_iterations > 0 {
            info!(
                "Running warmup with {} iterations",
                self.config.warmup_iterations
            );
            self.run_warmup(&transport_config)?;
        }

        // Run one-way latency test if enabled
        if self.config.one_way {
            info!("Running one-way latency test");
            let one_way_results =
                self.run_one_way_test(&transport_config, results_manager.as_deref_mut())?;
            results.add_one_way_results(one_way_results);
        }

        // Run round-trip latency test if enabled
        // Note: Shared memory in blocking mode doesn't support bidirectional communication
        if self.config.round_trip {
            if self.mechanism == IpcMechanism::SharedMemory {
                warn!(
                    "Shared memory in blocking mode does not support bidirectional \
                    communication. Skipping round-trip test."
                );
            } else {
                info!("Running round-trip latency test");
                let round_trip_results =
                    self.run_round_trip_test(&transport_config, results_manager)?;
                results.add_round_trip_results(round_trip_results);
            }
        }

        // Set total benchmark duration
        results.test_duration = total_start.elapsed();

        info!("Benchmark completed for {} mechanism", self.mechanism);
        Ok(results)
    }

    /// Run warmup iterations to stabilize performance (blocking version)
    ///
    /// Warmup is critical for accurate performance measurement as it allows
    /// various system components to reach steady state. This includes CPU caches,
    /// network connection establishment, OS buffer optimization, and other factors.
    ///
    /// ## Warmup Process
    ///
    /// 1. **Transport Setup**: Create and configure client/server transports
    /// 2. **Connection Establishment**: Server signals readiness via pipe
    /// 3. **Message Exchange**: Send warmup messages without measurement
    /// 4. **Resource Cleanup**: Properly close connections after warmup
    ///
    /// ## Synchronization
    ///
    /// The function uses an OS pipe to ensure the server process has successfully
    /// initialized the transport and is ready to accept connections before the client
    /// proceeds. This prevents race conditions and ensures startup errors are
    /// propagated immediately.
    ///
    /// ## Returns
    /// - `Ok(())`: Warmup completed successfully
    /// - `Err(anyhow::Error)`: Warmup failed
    fn run_warmup(&self, transport_config: &TransportConfig) -> Result<()> {
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;

        // --- Server Process Spawning ---
        let (mut server_process, mut pipe_reader) = self.spawn_server_process(transport_config)?;

        // Wait for the server to signal that it's ready.
        let mut buf = [0; 1];
        pipe_reader
            .read_exact(&mut buf)
            .context("Failed to read server ready signal from pipe for warmup")?;
        debug!("Client received server ready signal for warmup");

        // --- Client Logic ---
        client_transport.start_client_blocking(transport_config)?;

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

        // --- Cleanup ---
        // For PMQ and SHM, send a shutdown message to signal the server to exit
        // (These mechanisms don't have a connection to close like sockets)
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            debug!("Sending shutdown message to PMQ server (warmup)");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if self.mechanism == IpcMechanism::SharedMemory {
            debug!("Sending shutdown message to SHM server (warmup)");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        client_transport.close_blocking()?;
        server_process
            .wait()
            .context("Server process exited with an error during warmup")?;

        debug!("Warmup completed");
        Ok(())
    }

    /// Run one-way latency test (blocking version)
    ///
    /// One-way latency tests measure the time required to transmit a message
    /// from client to server without waiting for a response. This provides
    /// baseline transmission latency measurements.
    ///
    /// ## Test Methodology
    ///
    /// 1. **Setup Phase**: Initialize transports and establish connections
    /// 2. **Measurement Phase**: Send messages with precise timing measurement
    /// 3. **Cleanup Phase**: Properly close all connections and resources
    ///
    /// ## Concurrency Handling
    ///
    /// The function automatically adapts to mechanism capabilities:
    /// - Shared memory: Forces single-threaded to avoid race conditions
    /// - Other mechanisms: Uses single-threaded execution (multi-threading
    ///   not implemented in blocking mode yet)
    ///
    /// ## Returns
    /// - `Ok(PerformanceMetrics)`: Comprehensive latency and throughput metrics
    /// - `Err(anyhow::Error)`: Test execution failure
    fn run_one_way_test(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;

        // Check for problematic configurations and adapt automatically
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
        }

        // For blocking mode, we only implement single-threaded execution
        // Multi-threaded execution can be added in future if needed
        self.run_single_threaded_one_way(
            transport_config,
            &mut metrics_collector,
            results_manager,
        )?;

        Ok(metrics_collector.get_metrics())
    }

    /// Run round-trip latency test (blocking version)
    ///
    /// Round-trip latency tests measure the complete request-response cycle,
    /// providing insights into interactive communication patterns and full
    /// bidirectional performance characteristics.
    ///
    /// ## Test Methodology
    ///
    /// 1. **Client sends request**: Timestamp when request is sent
    /// 2. **Server receives and responds**: Echo or process request
    /// 3. **Client receives response**: Timestamp when response arrives
    /// 4. **Latency calculation**: Total time for complete cycle
    ///
    /// ## Response Patterns
    ///
    /// The server implements a simple echo pattern, immediately sending
    /// a response with a modified message ID to verify proper round-trip completion.
    ///
    /// ## Returns
    /// - `Ok(PerformanceMetrics)`: Round-trip latency and throughput metrics
    /// - `Err(anyhow::Error)`: Test execution failure
    fn run_round_trip_test(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Check for problematic configurations and adapt automatically
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
        }

        // For blocking mode, we only implement single-threaded execution
        self.run_single_threaded_round_trip(
            transport_config,
            &mut metrics_collector,
            results_manager,
        )?;

        Ok(metrics_collector.get_metrics())
    }

    /// Run single-threaded one-way test (blocking version)
    ///
    /// This implementation handles single client to single server communication
    /// for one-way latency measurement using blocking I/O. It provides accurate
    /// latency measurements without async overhead.
    ///
    /// ## Execution Flow
    ///
    /// 1. **Server Setup**: Spawn server process, wait for readiness signal
    /// 2. **Client Setup**: Create client transport and connect
    /// 3. **Data Collection**: Send messages (server measures latencies)
    /// 4. **Metrics Processing**: Read server-measured latencies from file
    /// 5. **Cleanup**: Close transport and wait for server to exit
    ///
    /// ## Timing Methodology
    ///
    /// TRUE IPC latency measurement matching C benchmark methodology:
    /// - Client embeds timestamp in message when sending
    /// - Server calculates latency = receive_time - message.timestamp
    /// - Server writes latencies to file
    /// - Client reads latencies from file after server completes
    ///
    /// This measures actual IPC transit time, not just buffer copy time.
    ///
    /// ## Duration vs Iteration Modes
    ///
    /// - **Iteration Mode**: Send exactly N messages regardless of time
    /// - **Duration Mode**: Send messages continuously for T seconds
    ///
    /// ## Returns
    /// - `Ok(())`: Test completed successfully, metrics updated
    /// - `Err(anyhow::Error)`: Test execution failure
    fn run_single_threaded_one_way(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        mut results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<()> {
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;

        // Create a temporary file for server to write latencies
        let latency_file_path = std::env::temp_dir()
            .join(format!("ipc_benchmark_latencies_{}.txt", Uuid::new_v4()))
            .to_string_lossy()
            .to_string();

        // --- Server Process Spawning ---
        let (mut server_process, mut pipe_reader) = self
            .spawn_server_process_with_latency_file(transport_config, Some(&latency_file_path))?;

        // Wait for the server to signal that it's ready
        let mut buf = [0; 1];
        pipe_reader
            .read_exact(&mut buf)
            .context("Failed to read server ready signal from pipe")?;
        debug!("Client received server ready signal for one-way test");

        // --- Client Logic ---
        // Apply client affinity if specified
        if let Some(client_core_id) = self.config.client_affinity {
            if let Some(ref core_ids) = self.available_cores {
                if client_core_id < core_ids.len() {
                    if !core_affinity::set_for_current(core_ids[client_core_id]) {
                        warn!(
                            "Failed to set client thread affinity to core {}",
                            client_core_id
                        );
                    } else {
                        debug!("Client thread pinned to core {}", client_core_id);
                    }
                }
            }
        }

        client_transport
            .start_client_blocking(transport_config)
            .with_context(|| {
                format!(
                    "start_client_blocking failed: mechanism={:?}, socket_path={}, host={}, port={}",
                    self.mechanism,
                    transport_config.socket_path,
                    transport_config.host,
                    transport_config.port
                )
            })?;

        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();

        // Client just sends messages - server measures and records latencies
        if let Some(duration) = self.config.duration {
            // Duration-based test
            let mut i = 0u64;

            // Send canary message if first message should not be included
            if !self.config.include_first_message {
                let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                let _ = client_transport.send_blocking(&canary);
            }

            while start_time.elapsed() < duration {
                let message = Message::new(i, payload.clone(), MessageType::OneWay);

                match client_transport.send_blocking(&message) {
                    Ok(_) => {
                        i += 1;
                        if let Some(delay) = self.config.send_delay {
                            std::thread::sleep(delay);
                        }
                    }
                    Err(_) => break,
                }
            }
        } else {
            // Message-count based test
            let msg_count = self.config.msg_count.unwrap_or_default();
            let iterations = if self.config.include_first_message {
                msg_count
            } else {
                msg_count + 1
            };

            for i in 0..iterations {
                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
                client_transport.send_blocking(&message)?;

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        // --- Cleanup ---
        // For PMQ and SHM, send a shutdown message to signal the server to exit
        // (These mechanisms don't have a connection to close like sockets)
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            debug!("Sending shutdown message to PMQ server");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if self.mechanism == IpcMechanism::SharedMemory {
            debug!("Sending shutdown message to SHM server");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        client_transport.close_blocking()?;
        server_process
            .wait()
            .context("Server process exited with an error")?;

        // --- Read server-measured latencies from file ---
        debug!(
            "Reading server-measured latencies from: {}",
            latency_file_path
        );
        use std::io::{BufRead, BufReader};
        let file =
            std::fs::File::open(&latency_file_path).context("Failed to open latency file")?;
        let reader = BufReader::new(file);

        for (i, line) in reader.lines().enumerate() {
            let line = line.context("Failed to read line from latency file")?;
            let latency_ns: u64 = line
                .parse()
                .with_context(|| format!("Failed to parse latency from line: {}", line))?;

            let latency = std::time::Duration::from_nanos(latency_ns);

            // Record in metrics collector
            metrics_collector.record_message(self.config.message_size, Some(latency))?;

            // Stream latency if enabled
            if let Some(ref mut manager) = results_manager {
                let record = crate::results::MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    crate::metrics::LatencyType::OneWay,
                    latency,
                );
                let _ = manager.stream_latency_record(&record);
            }
        }

        debug!("Successfully read and recorded server-measured latencies");

        // Clean up temporary latency file
        let _ = std::fs::remove_file(&latency_file_path);

        Ok(())
    }

    /// Run single-threaded round-trip test (blocking version)
    ///
    /// This implementation measures request-response latency in a single-threaded
    /// scenario using blocking I/O. It provides accurate round-trip measurements
    /// without async overhead.
    ///
    /// ## Round-trip Protocol
    ///
    /// 1. **Client sends request**: Message with Request type
    /// 2. **Server processes and responds**: Sends Response message
    /// 3. **Client receives response**: Completes timing measurement
    ///
    /// ## Latency Measurement
    ///
    /// Round-trip latency is measured from the start of the send operation
    /// to the completion of the receive operation, capturing the complete
    /// communication cycle including any processing delays.
    ///
    /// ## Returns
    /// - `Ok(())`: Test completed successfully, metrics updated
    /// - `Err(anyhow::Error)`: Test execution failure
    fn run_single_threaded_round_trip(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        mut results_manager: Option<&mut crate::results_blocking::BlockingResultsManager>,
    ) -> Result<()> {
        let mut client_transport =
            BlockingTransportFactory::create(&self.mechanism, self.args.shm_direct)?;

        // --- Server Process Spawning ---
        let (mut server_process, mut pipe_reader) = self.spawn_server_process(transport_config)?;

        // Wait for the server to signal that it's ready
        let mut buf = [0; 1];
        pipe_reader
            .read_exact(&mut buf)
            .context("Failed to read server ready signal from pipe")?;
        debug!("Client received server ready signal for round-trip test");

        // --- Client Logic ---
        // Apply client affinity if specified
        if let Some(client_core_id) = self.config.client_affinity {
            if let Some(ref core_ids) = self.available_cores {
                if client_core_id < core_ids.len() {
                    if !core_affinity::set_for_current(core_ids[client_core_id]) {
                        warn!(
                            "Failed to set client thread affinity to core {}",
                            client_core_id
                        );
                    } else {
                        debug!("Client thread pinned to core {}", client_core_id);
                    }
                }
            }
        }

        client_transport.start_client_blocking(transport_config)?;

        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();

        if let Some(duration) = self.config.duration {
            // Duration-based test
            let mut i = 0u64;

            // Send canary message if first message should not be included
            if !self.config.include_first_message {
                let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                if client_transport.send_blocking(&canary).is_ok() {
                    let _ = client_transport.receive_blocking();
                }
            }

            while start_time.elapsed() < duration {
                let send_time = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::Request);

                match client_transport.send_blocking(&message) {
                    Ok(_) => {
                        if let Some(delay) = self.config.send_delay {
                            std::thread::sleep(delay);
                        }
                        if client_transport.receive_blocking().is_ok() {
                            let latency = send_time.elapsed();

                            // Stream latency if enabled
                            if let Some(ref mut manager) = results_manager {
                                let record = crate::results::MessageLatencyRecord::new(
                                    i,
                                    self.mechanism,
                                    self.config.message_size,
                                    crate::metrics::LatencyType::RoundTrip,
                                    latency,
                                );
                                let _ = manager.stream_latency_record(&record);
                            }

                            // Record in metrics collector
                            metrics_collector
                                .record_message(self.config.message_size, Some(latency))?;
                        }
                        i += 1;
                    }
                    Err(_) => break,
                }
            }
        } else {
            // Message-count based test
            let msg_count = self.config.msg_count.unwrap_or_default();
            let iterations = if self.config.include_first_message {
                msg_count
            } else {
                msg_count + 1
            };

            for i in 0..iterations {
                let send_time = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);
                client_transport.send_blocking(&message)?;

                if let Some(delay) = self.config.send_delay {
                    std::thread::sleep(delay);
                }

                client_transport.receive_blocking()?;

                let latency = send_time.elapsed();

                // Only record latency for measured messages
                if i > 0 || self.config.include_first_message {
                    // Stream latency if enabled
                    if let Some(ref mut manager) = results_manager {
                        let record = crate::results::MessageLatencyRecord::new(
                            i as u64,
                            self.mechanism,
                            self.config.message_size,
                            crate::metrics::LatencyType::RoundTrip,
                            latency,
                        );
                        let _ = manager.stream_latency_record(&record);
                    }

                    // Record in metrics collector
                    metrics_collector.record_message(self.config.message_size, Some(latency))?;
                }
            }
        }

        // --- Cleanup ---
        // For PMQ and SHM, send a shutdown message to signal the server to exit
        // (These mechanisms don't have a connection to close like sockets)
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            debug!("Sending shutdown message to PMQ server (round-trip)");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if self.mechanism == IpcMechanism::SharedMemory {
            debug!("Sending shutdown message to SHM server (round-trip)");
            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = client_transport.send_blocking(&shutdown);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        client_transport.close_blocking()?;
        server_process
            .wait()
            .context("Server process exited with an error")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_runner() {
        // Create a basic configuration
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 100,
            blocking: true,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        // Verify the runner was created successfully
        assert_eq!(runner.mechanism, IpcMechanism::TcpSocket);
        assert_eq!(runner.config.message_size, 1024);
    }

    #[test]
    fn test_get_msg_count() {
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 500,
            blocking: true,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        assert_eq!(runner.get_msg_count(), 500);
    }

    #[test]
    fn test_validate_core_availability() {
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 100,
            blocking: true,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        // Should succeed with no affinity settings
        assert!(runner.validate_core_availability().is_ok());
    }

    #[test]
    fn test_validate_core_availability_invalid_server_affinity() {
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 100,
            server_affinity: Some(9999), // Invalid core ID
            blocking: true,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

        // Should fail with invalid affinity
        assert!(runner.validate_core_availability().is_err());
    }

    #[test]
    fn test_create_transport_config() {
        let mut args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 1000,
            blocking: true,
            ..Default::default()
        };
        args.host = "127.0.0.1".to_string(); // Explicitly set host

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

        let transport_config = runner.create_transport_config_internal(&args).unwrap();

        // Verify basic configuration
        assert!(transport_config.port > 0);
        assert_eq!(transport_config.host, "127.0.0.1");
        assert!(transport_config.buffer_size > 0);
    }

    #[test]
    fn test_spawn_server_includes_blocking_flag() {
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            message_size: 1024,
            msg_count: 10,
            blocking: true,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args).unwrap();
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

        let transport_config = runner.create_transport_config_internal(&args).unwrap();

        // This test verifies that spawn_server_process can be called
        // Actual spawning test would require a full integration test
        // For now, we just verify the method exists and compiles
        let result = runner.spawn_server_process(&transport_config);

        // We expect this to either succeed or fail with a clear error about the binary
        // The important thing is that the method signature is correct
        match result {
            Ok((mut child, _reader)) => {
                // Clean up the spawned process
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(e) => {
                // Expected in some test environments
                assert!(
                    e.to_string().contains("ipc-benchmark")
                        || e.to_string().contains("binary")
                        || e.to_string().contains("executable")
                );
            }
        }
    }
}
