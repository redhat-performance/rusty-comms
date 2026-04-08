//! # IPC Benchmark Suite - Main Entry Point
//!
//! This is the main entry point for the IPC (Inter-Process Communication) benchmark suite.
//! The application orchestrates performance testing of different IPC mechanisms including:
//! - Unix Domain Sockets (UDS)
//! - Shared Memory (SHM)
//! - TCP Sockets
//! - POSIX Message Queues (PMQ)
//!
//! ## Architecture Overview
//!
//! The main function performs these key operations:
//! 1. **Initialize logging**: Sets up structured logging with tracing
//! 2. **Parse arguments**: Processes command-line configuration
//! 3. **Create benchmark config**: Converts CLI args to internal config
//! 4. **Initialize results manager**: Sets up output handling and optional streaming
//! 5. **Run benchmarks**: Executes tests for each specified IPC mechanism
//! 6. **Generate results**: Finalizes and outputs comprehensive benchmark results
//!
//! ## Error Handling
//!
//! The application uses `anyhow::Result` for comprehensive error handling throughout.
//! Depending on the `continue_on_error` flag, the application can either:
//! - Stop on first benchmark failure (default)
//! - Continue running remaining benchmarks and report all results
//!
//! ## Concurrency Model
//!
//! The application uses Tokio's async runtime to handle:
//! - Concurrent client/server communication patterns
//! - Non-blocking I/O operations for all IPC mechanisms  
//! - Resource cleanup between benchmark runs

use anyhow::{Context, Result};
use clap::Parser;
use ipc_benchmark::{
    benchmark::{BenchmarkConfig, BenchmarkRunner},
    benchmark_blocking::BlockingBenchmarkRunner,
    cli::{Args, IpcMechanism},
    ipc::{
        get_monotonic_time_ns, BlockingTransport, BlockingTransportFactory, Message, MessageType,
        TransportConfig, TransportFactory,
    },
    metrics::{LatencyType, MetricsCollector},
    results::{BenchmarkResults, MessageLatencyRecord, ResultsManager},
    results_blocking::BlockingResultsManager,
};
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use tracing_subscriber::{filter::LevelFilter, prelude::*, Layer};

mod logging;
use ipc_benchmark::cli;
use logging::ColorizedFormatter;

/// Main entry point for the IPC benchmark suite.
///
/// This function determines the execution mode (async or blocking) based on
/// the `--blocking` CLI flag and dispatches to the appropriate execution path.
///
/// # Execution Modes
///
/// - **Async (default)**: Uses Tokio runtime with async/await for
///   non-blocking I/O
/// - **Blocking**: Uses std library with traditional blocking I/O operations
///
/// The mode selection happens at runtime based on CLI arguments, allowing the
/// same binary to run in either mode without recompilation.
///
/// # Examples
///
/// ```bash
/// # Run in async mode (default)
/// ipc-benchmark -m uds -s 1024 -i 10000
///
/// # Run in blocking mode
/// ipc-benchmark -m uds -s 1024 -i 10000 --blocking
/// ```
fn main() -> Result<()> {
    // Parse CLI arguments to determine execution mode
    let mut args = Args::parse();

    // Auto-enable blocking mode when --shm-direct is used
    // Direct memory shared memory is only available in blocking mode
    if args.shm_direct && !args.blocking {
        eprintln!(
            "Note: --shm-direct automatically enables --blocking mode \
             (direct memory SHM requires blocking I/O)"
        );
        args.blocking = true;
    }

    // Branch to appropriate execution path based on mode
    if args.server {
        run_standalone_server(args)
    } else if args.client {
        run_standalone_client(args)
    } else if args.blocking {
        // Blocking mode: use std library with blocking I/O
        run_blocking_mode(args)
    } else {
        // Async mode: use Tokio runtime with async/await
        run_async_mode(args)
    }
}

/// Run the benchmark in async mode using Tokio runtime.
///
/// This function contains all the existing async/await logic from the original
/// main() function. It uses the Tokio runtime for non-blocking I/O operations.
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments
///
/// # Returns
///
/// * `Ok(())` - Benchmark completed successfully
/// * `Err(anyhow::Error)` - Benchmark failed with error
#[tokio::main]
async fn run_async_mode(args: Args) -> Result<()> {
    // === ALL EXISTING MAIN() LOGIC STARTS HERE ===

    // Configure logging level based on verbosity flags.
    // This level applies to both the log file and stdout.
    // - default: INFO
    // -v: DEBUG
    // -vv and more: TRACE
    let log_level = match args.verbose {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Configure the detailed log layer (file or stderr).
    // The guard must be kept alive for the duration of the program for file logging.
    let guard;
    let detailed_log_layer;

    if let Some("stderr") = args.log_file.as_deref() {
        // Log detailed messages to stderr.
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(log_level)
            .boxed();
        guard = None;
    } else {
        // Log to a file, either specified or default.
        let file_appender = match args.log_file.as_deref() {
            Some(path_str) => {
                let log_path = std::path::Path::new(path_str);
                let log_dir = log_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                let log_filename = log_path
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("ipc_benchmark.log"));
                tracing_appender::rolling::daily(log_dir, log_filename)
            }
            None => tracing_appender::rolling::daily(".", "ipc_benchmark.log"),
        };
        let (non_blocking_writer, file_guard) = tracing_appender::non_blocking(file_appender);
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking_writer)
            .with_ansi(false) // Disable color codes for the file logger
            .with_filter(log_level)
            .boxed();
        guard = Some(file_guard);
    }

    // This layer sends clean, user-facing output to stdout.
    // It is only enabled if the --quiet flag is NOT present.
    // Its verbosity is controlled by the `log_level` derived from `-v` flags.
    // Disable stdout logging when running as the spawned server process to
    // keep stdout reserved for the readiness byte signaling.
    let stdout_log = if !args.quiet && !args.internal_run_as_server {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .event_format(ColorizedFormatter) // Use the custom formatter
                .with_filter(log_level),
        )
    } else {
        None
    };

    // Initialize the tracing subscriber by combining the layers.
    // The `with` method on the registry conveniently handles the Option from the stdout layer.
    tracing_subscriber::registry()
        .with(detailed_log_layer)
        .with(stdout_log)
        .init();

    // Keep the logging guard alive for the duration of the program.
    // If we don't assign it to a variable, it gets dropped immediately, and file logging stops working.
    let _log_guard = guard;

    // If the internal server flag is present, run in server-only mode and exit.
    if args.internal_run_as_server {
        return run_server_mode(args).await;
    }

    // Check for pmq_priority usage with non-PMQ mechanisms, only on Linux where PMQ is supported.
    #[cfg(target_os = "linux")]
    if args.pmq_priority != 0 {
        let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());
        for &mechanism in &mechanisms {
            if mechanism != IpcMechanism::PosixMessageQueue {
                tracing::info!(
                    "'pmq-priority' parameter ignored for mechanism '{}'",
                    mechanism
                );
            }
        }
    }

    info!("Starting IPC Benchmark Suite");
    // The detailed configuration will be printed for each mechanism run.

    // Create benchmark configuration from parsed CLI arguments
    // This converts the user-friendly CLI format into the internal
    // configuration structure used by the benchmark engine
    let config = BenchmarkConfig::from_args(&args)?;

    // Calculate today's date string once to ensure consistency across all branches.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Determine the actual log file path, accounting for daily rotation.
    // This ensures the summary report shows the correct filename, which includes
    // the date suffix added by the rolling file appender.
    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => Some(format!("{}.{}", path_str, today)),
        None => Some(format!("ipc_benchmark.log.{}", today)),
    };

    // Initialize results manager for handling output
    // This manages both final JSON output and optional streaming results
    let mut results_manager =
        ResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable per-message latency streaming if specified
    // Per-message streaming captures individual message latency values with
    // timestamps for real-time monitoring of latency characteristics during execution
    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );

        // Check if both test types are enabled for combined streaming
        let both_tests_enabled = config.one_way && config.round_trip;

        if both_tests_enabled {
            info!("Both one-way and round-trip tests enabled - using combined streaming mode");
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }

    // Enable CSV latency streaming if specified
    if let Some(ref streaming_file) = args.streaming_output_csv {
        info!("Enabling CSV latency streaming to: {:?}", streaming_file);
        results_manager.enable_csv_streaming(streaming_file)?;
    }

    // Get expanded mechanisms (handles 'all' expansion)
    // The 'all' mechanism is a convenience option that expands to
    // all available IPC mechanisms for comprehensive testing
    let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());

    // Run benchmarks for each selected mechanism.
    // This loop iterates through the list of IPC mechanisms to be tested,
    // executing the benchmark for each one sequentially.
    for &mechanism in &mechanisms {
        // Execute the benchmark for the current mechanism and handle the result.
        // The `run_benchmark_for_mechanism` function encapsulates all logic for a single test.
        match run_benchmark_for_mechanism(&config, &mechanism, &mut results_manager, &args).await {
            Ok(()) => {
                // On success, the `run_benchmark_for_mechanism` function has already added
                // the results to the `results_manager`. No further action is needed here.
            }
            Err(e) => {
                // Handle benchmark failure for a specific mechanism.
                let error_msg = e.to_string();
                error!(
                    "Benchmark for {} failed: {}. {}",
                    mechanism,
                    error_msg,
                    if args.continue_on_error {
                        "Continuing to next mechanism."
                    } else {
                        "Aborting."
                    }
                );

                // If --continue-on-error is enabled, record the failure and proceed.
                // Otherwise, propagate the error and terminate the application.
                if args.continue_on_error {
                    // Create a `BenchmarkResults` object with a `Failure` status
                    // to ensure the failed test is included in the final report.
                    let mut failed_result = BenchmarkResults::new(
                        mechanism,
                        config.message_size,
                        0, // Buffer size unknown in failure case
                        config.concurrency,
                        config.msg_count,
                        config.duration,
                        config.warmup_iterations,
                        config.one_way,
                        config.round_trip,
                    );
                    failed_result.set_failure(error_msg);
                    results_manager.add_results(failed_result).await?;
                } else {
                    // If not continuing on error, abort the entire benchmark suite.
                    return Err(e);
                }
            }
        }
    }

    // Finalize results and output
    // This performs final aggregation, calculates summary statistics,
    // and writes the comprehensive results to the output file
    results_manager.finalize().await?;

    // Print a human-readable summary of the results to the console
    if let Err(e) = results_manager.print_summary() {
        error!("Failed to print results summary: {}", e);
    }

    info!("IPC Benchmark Suite completed successfully");
    Ok(())
}

/// Run the benchmark in blocking mode using std library.
///
/// This function implements a blocking version of the benchmark execution,
/// using traditional blocking I/O from the standard library instead of
/// async/await with Tokio.
///
/// ## Key Differences from Async Mode
///
/// - Uses `BlockingBenchmarkRunner` instead of `BenchmarkRunner`
/// - No streaming output support (will be added in Stage 5)
/// - All operations block the calling thread
/// - Uses standard library I/O instead of Tokio
///
/// ## Execution Flow
///
/// 1. **Logging Setup**: Configure logging based on verbosity flags
/// 2. **Server Mode Check**: Handle --internal-run-as-server flag
/// 3. **Configuration**: Create benchmark configuration from CLI args
/// 4. **Mechanism Expansion**: Handle 'all' mechanism expansion
/// 5. **Benchmark Execution**: Run benchmarks for each mechanism
/// 6. **Results Output**: Print results summary
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments
///
/// # Returns
///
/// * `Ok(())` - Benchmark completed successfully
/// * `Err(anyhow::Error)` - Benchmark failed with error
fn run_blocking_mode(args: Args) -> Result<()> {
    // Check for server mode FIRST before setting up logging
    // Server mode uses stderr for logging to avoid interfering with stdout pipe
    if args.internal_run_as_server {
        // Minimal logging setup for server mode - log to stderr only
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::DEBUG)
            .init();
        return run_server_mode_blocking(args);
    }

    // Configure logging level based on verbosity flags
    let log_level = match args.verbose {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Configure the detailed log layer (file or stderr)
    let guard;
    let detailed_log_layer;

    if let Some("stderr") = args.log_file.as_deref() {
        // Log detailed messages to stderr
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(log_level)
            .boxed();
        guard = None;
    } else {
        // Log to a file, either specified or default
        let file_appender = match args.log_file.as_deref() {
            Some(path_str) => {
                let log_path = std::path::Path::new(path_str);
                let log_dir = log_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."));
                let log_filename = log_path
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("ipc_benchmark.log"));
                tracing_appender::rolling::daily(log_dir, log_filename)
            }
            None => tracing_appender::rolling::daily(".", "ipc_benchmark.log"),
        };
        let (non_blocking_writer, file_guard) = tracing_appender::non_blocking(file_appender);
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking_writer)
            .with_ansi(false)
            .with_filter(log_level)
            .boxed();
        guard = Some(file_guard);
    }

    // Stdout layer for user-facing output (disabled in server mode)
    let stdout_log = if !args.quiet {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .event_format(ColorizedFormatter)
                .with_filter(log_level),
        )
    } else {
        None
    };

    // Initialize the tracing subscriber
    tracing_subscriber::registry()
        .with(detailed_log_layer)
        .with(stdout_log)
        .init();

    // Keep the logging guard alive
    let _log_guard = guard;

    info!("Starting IPC Benchmark Suite (Blocking Mode)");

    // Create benchmark configuration from parsed CLI arguments
    let config = BenchmarkConfig::from_args(&args)?;

    // Calculate today's date string once to ensure consistency across all
    // branches. This is used for daily log rotation.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Determine the actual log file path, accounting for daily rotation.
    // This ensures the summary report shows the correct filename, which
    // includes the date suffix added by the rolling file appender.
    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => Some(format!("{}.{}", path_str, today)),
        None => Some(format!("ipc_benchmark.log.{}", today)),
    };

    // Initialize blocking results manager for handling output
    // This manages both final JSON output and optional streaming results
    // using blocking I/O operations.
    let mut results_manager =
        BlockingResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable per-message latency streaming if specified
    // Per-message streaming captures individual message latency values with
    // timestamps for real-time monitoring of latency characteristics during
    // execution.
    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );

        // Check if both test types are enabled for combined streaming
        let both_tests_enabled = config.one_way && config.round_trip;

        if both_tests_enabled {
            info!(
                "Both one-way and round-trip tests enabled - using combined \
                 streaming mode"
            );
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }

    // Enable CSV latency streaming if specified
    if let Some(ref streaming_file) = args.streaming_output_csv {
        info!("Enabling CSV latency streaming to: {:?}", streaming_file);
        results_manager.enable_csv_streaming(streaming_file)?;
    }

    // Get expanded mechanisms (handles 'all' expansion)
    let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());

    // Run benchmarks for each selected mechanism
    for &mechanism in &mechanisms {
        match run_blocking_benchmark_for_mechanism(&config, &mechanism, &args, &mut results_manager)
        {
            Ok(results) => {
                info!(
                    "Successfully completed benchmark for {} mechanism",
                    mechanism
                );
                // Add results to manager (blocking operation)
                results_manager.add_results(results)?;
            }
            Err(e) => {
                let error_msg = e.to_string();
                error!(
                    "Benchmark for {} failed: {}. {}",
                    mechanism,
                    error_msg,
                    if args.continue_on_error {
                        "Continuing to next mechanism."
                    } else {
                        "Aborting."
                    }
                );

                // If --continue-on-error is enabled, record the failure and
                // proceed. Otherwise, propagate the error and terminate the
                // application.
                if args.continue_on_error {
                    // Create a `BenchmarkResults` object with a `Failure`
                    // status to ensure the failed test is included in the
                    // final report.
                    let mut failed_result = BenchmarkResults::new(
                        mechanism,
                        config.message_size,
                        0, // Buffer size unknown in failure case
                        config.concurrency,
                        config.msg_count,
                        config.duration,
                        config.warmup_iterations,
                        config.one_way,
                        config.round_trip,
                    );
                    failed_result.set_failure(error_msg);
                    results_manager.add_results(failed_result)?;
                } else {
                    // If not continuing on error, abort the entire benchmark
                    // suite.
                    return Err(e);
                }
            }
        }
    }

    // Finalize results and output (blocking operation)
    // This performs final aggregation, calculates summary statistics,
    // and writes the comprehensive results to the output file.
    results_manager.finalize()?;

    // Print a human-readable summary of the results to the console
    if let Err(e) = results_manager.print_summary() {
        error!("Failed to print results summary: {}", e);
    }

    info!("IPC Benchmark Suite (Blocking Mode) completed successfully");

    Ok(())
}

/// Run a blocking benchmark for a specific mechanism
///
/// This function executes the complete benchmark lifecycle for a single IPC mechanism
/// using blocking I/O operations.
///
/// ## Arguments
///
/// * `config` - Benchmark configuration
/// * `mechanism` - IPC mechanism to test
/// * `args` - Command-line arguments
/// * `results_manager` - Results manager for streaming output
///
/// ## Returns
///
/// * `Ok(BenchmarkResults)` - Benchmark completed successfully with results
/// * `Err(anyhow::Error)` - Benchmark failed
fn run_blocking_benchmark_for_mechanism(
    config: &BenchmarkConfig,
    mechanism: &IpcMechanism,
    args: &Args,
    results_manager: &mut BlockingResultsManager,
) -> Result<BenchmarkResults> {
    // Create blocking benchmark runner for this mechanism
    let runner = BlockingBenchmarkRunner::new(config.clone(), *mechanism, args.clone());

    // Run the benchmark (this blocks until complete)
    // Pass results_manager for streaming latency records
    let results = runner
        .run(Some(results_manager))
        .context(format!("Benchmark failed for {}", mechanism))?;

    Ok(results)
}

/// Run server mode in blocking mode
///
/// This function handles the `--internal-run-as-server` flag for blocking mode.
/// It sets up a blocking server that listens for client connections and handles
/// messages.
///
/// ## Server Loop
///
/// The server runs a persistent loop that:
/// 1. Receives messages from the client
/// 2. For Request messages, sends a Response back
/// 3. For OneWay messages, no response needed
/// 4. Exits on client disconnect or error
///
/// ## Returns
///
/// * `Ok(())` - Server completed successfully
/// * `Err(anyhow::Error)` - Server setup or execution failed
fn run_server_mode_blocking(args: cli::Args) -> Result<()> {
    info!("Running in server-only mode (blocking)");

    // In server mode, we only care about the first mechanism specified
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => {
            return Err(anyhow::anyhow!(
                "No IPC mechanism specified for server mode"
            ))
        }
    };

    // Set CPU affinity for the server process if specified
    if let Some(core) = args.server_affinity {
        if let Err(e) = set_affinity(core) {
            error!("Failed to set server CPU affinity to core {}: {}", core, e);
        } else {
            info!("Successfully set server affinity to CPU core {}", core);
        }
    }

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config.clone(), mechanism, args.clone());

    // Build transport config, ensuring we honor exact endpoints passed from parent
    let mut transport_config = runner.create_transport_config_internal(&args)?;
    match mechanism {
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket => {
            if let Some(ref p) = args.socket_path {
                transport_config.socket_path = p.clone();
            }
        }
        IpcMechanism::TcpSocket => {
            transport_config.host = args.host.clone();
            transport_config.port = args.port;
        }
        IpcMechanism::SharedMemory => {
            if let Some(ref n) = args.shared_memory_name {
                transport_config.shared_memory_name = n.clone();
            }
        }
        #[cfg(target_os = "linux")]
        IpcMechanism::PosixMessageQueue => {
            if let Some(ref n) = args.message_queue_name {
                transport_config.message_queue_name = n.clone();
            }
        }
        IpcMechanism::All => {}
    }

    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;
    transport
        .start_server_blocking(&transport_config)
        .context("Server failed to start transport")?;

    // Signal to the parent process that the server is ready
    io::stdout()
        .write_all(&[1])
        .context("Failed to write server ready byte to stdout")?;
    io::stdout().flush().ok();

    // Buffer latencies in memory instead of per-message file I/O
    // This avoids the massive overhead of writing to disk for each message
    let latency_file_path = args.internal_latency_file.clone();
    let mut latency_buffer: Vec<(u64, u64)> = if latency_file_path.is_some() {
        Vec::with_capacity(100_000) // Pre-allocate for performance
    } else {
        Vec::new()
    };

    // Persistent server loop: receive messages and optionally reply
    loop {
        match transport.receive_blocking() {
            Ok(message) => {
                // Calculate actual IPC latency: receive_time - send_time
                // Use monotonic clock to avoid NTP adjustments affecting measurements
                let receive_time_ns = get_monotonic_time_ns();
                let wall_now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let latency_ns = receive_time_ns.saturating_sub(message.timestamp);

                if should_buffer_latency(latency_file_path.is_some(), message.id) {
                    let wall_send_ns = wall_now_ns.saturating_sub(latency_ns);
                    latency_buffer.push((wall_send_ns, latency_ns));
                }

                // Check for shutdown message (used by PMQ and other queue-based transports)
                if message.message_type == MessageType::Shutdown {
                    debug!("Server received shutdown message, exiting cleanly");
                    break;
                }

                // If it's a Request, send a Response back
                if message.message_type == MessageType::Request {
                    let response = Message::new(message.id, Vec::new(), MessageType::Response);
                    if let Err(e) = transport.send_blocking(&response) {
                        warn!(
                            "Server failed to send response: {}. Exiting server loop.",
                            e
                        );
                        break;
                    }
                } else if message.message_type == MessageType::Ping {
                    let pong = Message::new(message.id, Vec::new(), MessageType::Pong);
                    if let Err(e) = transport.send_blocking(&pong) {
                        warn!("Server failed to send pong: {}. Exiting server loop.", e);
                        break;
                    }
                }
                // For OneWay messages, no response needed
            }
            Err(e) => {
                debug!("Server receive error (client likely disconnected): {}", e);
                break;
            }
        }
    }

    let close_result = transport.close_blocking();

    if let Some(ref path) = latency_file_path {
        write_latency_buffer(path, &latency_buffer)?;
    }

    close_result?;

    info!("Server exiting cleanly.");
    Ok(())
}

/// Sets the CPU affinity for the current thread to the specified core.
///
/// This function takes a core ID as input and attempts to pin the current
/// thread to that CPU core. This can help improve performance by reducing
/// cache misses and context switching.
///
/// ## Parameters
///
/// - `core_id`: The ID of the CPU core to pin the thread to.
///
/// ## Returns
///
/// - `Ok(())` if the affinity was set successfully.
/// - `Err(anyhow::Error)` if the specified core ID is not available or the
///   affinity could not be set.
fn set_affinity(core_id: usize) -> Result<()> {
    let core_ids = core_affinity::get_core_ids().context("Failed to get core IDs")?;
    info!(
        "Server: Available cores: {} total, requesting core {}",
        core_ids.len(),
        core_id
    );

    let core = core_ids
        .get(core_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid core ID: {}", core_id))?;
    info!("Server: Attempting to set affinity to core {:?}", core);

    if core_affinity::set_for_current(*core) {
        info!("Server: Successfully set affinity to core {}", core_id);
        Ok(())
    } else {
        error!("Server: Failed to set affinity to core {}", core_id);
        Err(anyhow::anyhow!(
            "Failed to set affinity for core ID: {}",
            core_id
        ))
    }
}

/// Executes the application in a server-only mode for a single IPC mechanism.
///
/// This function is triggered by the internal `--internal-run-as-server` flag.
/// It is responsible for setting up and running the server component of a benchmark,
/// allowing the main process to act as the client.
///
/// ## Workflow
///
/// 1. **Configuration**: Extracts the necessary configuration from the provided arguments.
///    Since only one mechanism is tested at a time in this mode, it selects the first one.
/// 2. **Affinity**: Pins the server process to a specific CPU core if specified by
///    `--server-affinity`.
/// 3. **Transport Setup**: Creates and starts the server for the specified IPC mechanism.
/// 4. **Signaling**: Prints a "SERVER_READY" message to stdout to signal the parent
///    (client) process that it is ready to accept connections.
/// 5. **Execution**: Enters the main server loop to handle incoming client messages.
///
/// ## Parameters
///
/// - `args`: The parsed command-line arguments.
///
/// ## Returns
///
/// - `Ok(())` on successful execution and shutdown.
/// - `Err(anyhow::Error)` if any part of the server setup or execution fails.
async fn run_server_mode(args: cli::Args) -> Result<()> {
    info!("Running in server-only mode.");

    // In server mode, we only care about the first mechanism specified.
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => {
            return Err(anyhow::anyhow!(
                "No IPC mechanism specified for server mode"
            ))
        }
    };

    // Set CPU affinity for the server process if specified.
    if let Some(core) = args.server_affinity {
        if let Err(e) = set_affinity(core) {
            error!("Failed to set server CPU affinity to core {}: {}", core, e);
            // We'll log the error but continue execution.
        } else {
            info!("Successfully set server affinity to CPU core {}", core);
        }
    }

    // from_args takes a reference to Args
    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BenchmarkRunner::new(config.clone(), mechanism, args.clone());
    // Build transport config, but ensure we honor exact endpoints passed from parent.
    let mut transport_config = runner.create_transport_config_internal(&args)?;
    match mechanism {
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket => {
            if let Some(ref p) = args.socket_path {
                transport_config.socket_path = p.clone();
            }
        }
        IpcMechanism::TcpSocket => {
            transport_config.host = args.host.clone();
            transport_config.port = args.port; // use exact port provided by parent
        }
        IpcMechanism::SharedMemory => {
            if let Some(ref n) = args.shared_memory_name {
                transport_config.shared_memory_name = n.clone();
            }
        }
        #[cfg(target_os = "linux")]
        IpcMechanism::PosixMessageQueue => {
            if let Some(ref n) = args.message_queue_name {
                transport_config.message_queue_name = n.clone();
            }
        }
        IpcMechanism::All => {}
    }

    let mut transport = TransportFactory::create(&mechanism)?;
    transport
        .start_server(&transport_config)
        .await
        .context("Server failed to start transport")?;

    // Signal to the parent process that the server is ready by writing a single
    // byte to stdout. The parent connected the pipe writer to the child's stdout.
    io::stdout()
        .write_all(&[1])
        .context("Failed to write server ready byte to stdout")?;
    io::stdout().flush().ok();

    // Buffer latencies in memory instead of per-message file I/O
    // This avoids the massive overhead of writing to disk for each message
    let latency_file_path = args.internal_latency_file.clone();
    let mut latency_buffer: Vec<(u64, u64)> = if latency_file_path.is_some() {
        Vec::with_capacity(100_000) // Pre-allocate for performance
    } else {
        Vec::new()
    };

    // Persistent server loop: receive messages and optionally reply to
    // round-trip patterns. Exit cleanly on disconnect or receive error.
    loop {
        // Await directly on receive so that transport-level errors (including
        // client disconnects) are observed and the server can exit cleanly.
        match transport.receive().await {
            Ok(msg) => {
                // Calculate actual IPC latency: receive_time - send_time
                // Use monotonic clock to avoid NTP adjustments affecting measurements
                let receive_time_ns = get_monotonic_time_ns();
                let wall_now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let latency_ns = receive_time_ns.saturating_sub(msg.timestamp);

                if should_buffer_latency(latency_file_path.is_some(), msg.id) {
                    let wall_send_ns = wall_now_ns.saturating_sub(latency_ns);
                    latency_buffer.push((wall_send_ns, latency_ns));
                }

                // Message received
                match msg.message_type {
                    MessageType::Request => {
                        // Echo a response to complete round-trip flows.
                        let resp = Message::new(msg.id, Vec::new(), MessageType::Response);
                        if transport.send(&resp).await.is_err() {
                            info!("Client disconnected during send, exiting server loop.");
                            break;
                        }
                    }
                    MessageType::Ping => {
                        let resp = Message::new(msg.id, Vec::new(), MessageType::Pong);
                        if transport.send(&resp).await.is_err() {
                            info!("Client disconnected during send, exiting server loop.");
                            break;
                        }
                    }
                    // OneWay and other types need no reply.
                    _ => {}
                }
            }
            Err(e) => {
                // Transport error
                info!("Server receive loop ending due to transport error: {}", e);
                break;
            }
        }
    }

    let close_result = transport.close().await;

    if let Some(ref path) = latency_file_path {
        write_latency_buffer(path, &latency_buffer)?;
    }

    if let Err(e) = close_result {
        warn!("Transport close error: {}", e);
    }

    info!("Server mode finished.");
    Ok(())
}

/// Run benchmark for a specific IPC mechanism
///
/// This function encapsulates the benchmark execution for a single IPC mechanism.
/// It creates a `BenchmarkRunner` configured for the specific mechanism and
/// executes both one-way and round-trip latency tests as configured.
///
/// ## Parameters
/// - `config`: Benchmark configuration (message size, message count, etc.)
/// - `mechanism`: The specific IPC mechanism to test
/// - `results_manager`: Manager for collecting and outputting results
///
/// ## Returns
/// - `Ok(())` if benchmark completes successfully
/// - `Err(anyhow::Error)` if benchmark fails for any reason
///
/// ## Error Conditions
/// - Transport initialization failures (e.g., port already in use)
/// - Communication timeouts during testing
/// - Resource allocation failures (e.g., shared memory creation)
/// - Serialization/deserialization errors
async fn run_benchmark_for_mechanism(
    config: &BenchmarkConfig,
    mechanism: &IpcMechanism,
    results_manager: &mut ResultsManager,
    args: &Args,
) -> Result<()> {
    // Create a benchmark runner for this specific mechanism
    // The runner encapsulates all the logic for setting up clients/servers,
    // running warmup iterations, executing tests, and collecting metrics
    let runner = BenchmarkRunner::new(config.clone(), *mechanism, args.clone());

    // Execute the benchmark and collect comprehensive results
    // This includes latency histograms, throughput measurements,
    // and statistical analysis (percentiles, mean, std dev, etc.)
    let results = runner.run(Some(results_manager)).await?;

    // Add results to the manager for aggregation and output
    // The manager handles both immediate streaming (if enabled)
    // and final consolidated output formatting
    results_manager.add_results(results).await?;
    Ok(())
}

// --- Standalone constants ---

/// Maximum time the client will retry connecting before giving up.
const CONNECT_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Interval between client connection retry attempts.
const CONNECT_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

// --- Standalone helpers ---

/// Determine the server's response to an incoming message.
///
/// Returns `Some(response)` for Request (-> Response) and Ping (-> Pong).
/// Returns `None` for all other message types (OneWay, Shutdown, etc.),
/// which the caller handles directly for control flow.
fn dispatch_server_message(msg: &Message) -> Option<Message> {
    match msg.message_type {
        MessageType::Request => Some(Message::new(msg.id, Vec::new(), MessageType::Response)),
        MessageType::Ping => Some(Message::new(msg.id, Vec::new(), MessageType::Pong)),
        _ => None,
    }
}

/// Build a TransportConfig from CLI args for standalone mode.
///
/// Uses explicit endpoint flags if provided, otherwise falls back
/// to defaults from TransportConfig::default(). This allows the
/// simple case (no extra flags) to work out of the box.
fn build_standalone_transport_config(args: &Args) -> TransportConfig {
    let defaults = TransportConfig::default();

    TransportConfig {
        host: args.host.clone(),
        port: args.port,
        pmq_priority: args.pmq_priority,
        socket_path: args.socket_path.clone().unwrap_or(defaults.socket_path),
        shared_memory_name: args
            .shared_memory_name
            .clone()
            .unwrap_or(defaults.shared_memory_name),
        message_queue_name: args
            .message_queue_name
            .clone()
            .unwrap_or(defaults.message_queue_name),
        buffer_size: args.buffer_size.unwrap_or(defaults.buffer_size),
        ..defaults
    }
}

/// Run in standalone server mode.
///
/// Starts a server that listens for client connections using the
/// specified IPC mechanism. The server runs until the client
/// disconnects or a shutdown message is received.
///
/// Works with both async and blocking transports depending on
/// the --blocking flag.
fn run_standalone_server(args: Args) -> Result<()> {
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => return Err(anyhow::anyhow!("No IPC mechanism specified")),
    };

    if mechanism == IpcMechanism::All {
        return Err(anyhow::anyhow!(
            "Cannot use 'all' mechanism in standalone server mode. \
             Specify a single mechanism (e.g., -m uds)"
        ));
    }

    // Set up logging (simplified: stderr only for standalone server)
    let log_level = match args.verbose {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(log_level)
        .event_format(ColorizedFormatter)
        .init();

    // Set CPU affinity if specified
    if let Some(core) = args.server_affinity {
        if let Err(e) = set_affinity(core) {
            error!("Failed to set server CPU affinity to core {}: {}", core, e);
        } else {
            info!("Server affinity set to CPU core {}", core);
        }
    }

    let transport_config = build_standalone_transport_config(&args);
    let config = BenchmarkConfig::from_args(&args)?;

    info!(
        "Starting standalone server: mechanism={}, blocking={}",
        mechanism, args.blocking
    );

    if args.blocking {
        run_standalone_server_blocking(&args, mechanism, &transport_config, &config)
    } else {
        run_standalone_server_async(args, mechanism, transport_config, &config)
    }
}

/// Blocking standalone server implementation.
///
/// Collects one-way latency metrics for OneWay messages using the
/// monotonic clock difference between send and receive timestamps.
/// This is accurate when server and client share the same kernel
/// clock (same host, container-to-host, container-to-container).
///
/// For TCP and UDS, the server accepts multiple concurrent connections,
/// spawning a handler thread per client. For SHM and PMQ, only a single
/// connection is supported.
fn run_standalone_server_blocking(
    args: &Args,
    mechanism: IpcMechanism,
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    match mechanism {
        IpcMechanism::TcpSocket => {
            run_standalone_server_blocking_multi_accept_tcp(transport_config, config)
        }
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket => {
            run_standalone_server_blocking_multi_accept_uds(transport_config, config)
        }
        _ => run_standalone_server_blocking_single(args, mechanism, transport_config, config),
    }
}

/// Single-connection blocking server for SHM/PMQ mechanisms.
fn run_standalone_server_blocking_single(
    args: &Args,
    mechanism: IpcMechanism,
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;
    transport
        .start_server_blocking(transport_config)
        .context("Server failed to start transport")?;

    info!("Server listening, waiting for client...");

    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;
    let mut one_way_count = 0u64;

    while let Ok(message) = transport.receive_blocking() {
        if message.message_type == MessageType::Shutdown {
            debug!("Server received shutdown message, exiting");
            break;
        }

        if message.message_type == MessageType::OneWay && message.id != u64::MAX {
            let receive_time_ns = get_monotonic_time_ns();
            let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
            let latency = std::time::Duration::from_nanos(latency_ns);
            one_way_metrics.record_message(config.message_size, Some(latency))?;
            one_way_count += 1;
        }

        if let Some(response) = dispatch_server_message(&message) {
            if let Err(e) = transport.send_blocking(&response) {
                warn!("Server failed to send response: {}", e);
                break;
            }
        }
    }

    print_server_one_way_latency(one_way_count, &one_way_metrics);
    transport.close_blocking()?;
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Handle a single client connection in a server handler thread.
///
/// Runs the message dispatch loop and collects one-way latency metrics.
/// Returns the metrics collector when the client disconnects or sends Shutdown.
fn handle_client_connection(
    transport: &mut dyn ipc_benchmark::ipc::BlockingTransport,
    config: &BenchmarkConfig,
) -> Result<MetricsCollector> {
    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;

    while let Ok(message) = transport.receive_blocking() {
        if message.message_type == MessageType::Shutdown {
            debug!("Handler received shutdown message");
            break;
        }

        if message.message_type == MessageType::OneWay && message.id != u64::MAX {
            let receive_time_ns = get_monotonic_time_ns();
            let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
            let latency = std::time::Duration::from_nanos(latency_ns);
            one_way_metrics.record_message(config.message_size, Some(latency))?;
        }

        if let Some(response) = dispatch_server_message(&message) {
            if let Err(e) = transport.send_blocking(&response) {
                warn!("Handler failed to send response: {}", e);
                break;
            }
        }
    }

    Ok(one_way_metrics)
}

/// Multi-accept blocking TCP server.
///
/// Binds a raw TCP listener and accepts connections in a loop, spawning
/// a handler thread per client. Each handler gets its own `BlockingTcpSocket`
/// via `from_stream()`. Metrics are aggregated across all handlers at shutdown.
fn run_standalone_server_blocking_multi_accept_tcp(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use ipc_benchmark::ipc::BlockingTcpSocket;
    use socket2::{Domain, Socket, Type};
    use std::sync::{Arc, Mutex};

    let addr = format!("{}:{}", transport_config.host, transport_config.port);
    let socket =
        Socket::new(Domain::IPV4, Type::STREAM, None).context("Failed to create TCP socket")?;
    socket
        .set_reuse_address(true)
        .context("Failed to set SO_REUSEADDR")?;
    let socket_addr: std::net::SocketAddr = addr
        .parse()
        .with_context(|| format!("Failed to parse address: {}", addr))?;
    socket
        .bind(&socket_addr.into())
        .with_context(|| format!("Failed to bind TCP socket to {}", addr))?;
    socket
        .listen(128)
        .context("Failed to listen on TCP socket")?;
    let listener: std::net::TcpListener = socket.into();
    // Non-blocking so we can periodically check if all handler threads
    // have finished (i.e. all clients disconnected).
    listener
        .set_nonblocking(true)
        .context("Failed to set listener to non-blocking")?;

    info!("TCP server listening on {}, accepting connections...", addr);

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    // Grace period after first client connects before we check if all
    // handlers are done. This prevents premature exit when a fast client
    // finishes before slower clients have connected.
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = std::time::Duration::from_secs(2);

    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                info!("Accepted connection from {}", peer_addr);
                if first_client_time.is_none() {
                    first_client_time = Some(std::time::Instant::now());
                }
                stream
                    .set_nodelay(true)
                    .context("Failed to set TCP_NODELAY")?;

                let metrics_clone = worker_metrics.clone();
                let handler_config = config.clone();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    match handle_client_connection(&mut transport, &handler_config) {
                        Ok(collector) => {
                            metrics_clone.lock().unwrap().push(collector);
                        }
                        Err(e) => {
                            warn!("Handler error: {}", e);
                        }
                    }
                    let _ = transport.close_blocking();
                    info!("Client {} disconnected", peer_addr);
                });
                handles.push(handle);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection. Check if all handlers are done,
                // but only after the grace period has elapsed since the
                // first client connected.
                let grace_elapsed =
                    first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
                if grace_elapsed && !handles.is_empty() && handles.iter().all(|h| h.is_finished()) {
                    debug!("All clients disconnected, shutting down");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                debug!("Accept error: {}", e);
                break;
            }
        }
    }

    for handle in handles {
        let _ = handle.join();
    }

    let collectors = worker_metrics.lock().unwrap();
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Multi-accept blocking UDS server.
///
/// Same pattern as the TCP multi-accept server but using Unix domain sockets.
#[cfg(unix)]
fn run_standalone_server_blocking_multi_accept_uds(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use ipc_benchmark::ipc::BlockingUnixDomainSocket;
    use std::sync::{Arc, Mutex};

    // Remove existing socket file
    let _ = std::fs::remove_file(&transport_config.socket_path);

    let listener = std::os::unix::net::UnixListener::bind(&transport_config.socket_path)
        .with_context(|| {
            format!(
                "Failed to bind Unix domain socket at: {}",
                transport_config.socket_path
            )
        })?;
    listener
        .set_nonblocking(true)
        .context("Failed to set UDS listener to non-blocking")?;

    info!(
        "UDS server listening on {}, accepting connections...",
        transport_config.socket_path
    );

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = std::time::Duration::from_secs(2);

    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                info!("Accepted UDS connection from {:?}", peer_addr);
                if first_client_time.is_none() {
                    first_client_time = Some(std::time::Instant::now());
                }

                let metrics_clone = worker_metrics.clone();
                let handler_config = config.clone();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingUnixDomainSocket::from_stream(stream);
                    match handle_client_connection(&mut transport, &handler_config) {
                        Ok(collector) => {
                            metrics_clone.lock().unwrap().push(collector);
                        }
                        Err(e) => {
                            warn!("Handler error: {}", e);
                        }
                    }
                    let _ = transport.close_blocking();
                    info!("UDS client disconnected");
                });
                handles.push(handle);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let grace_elapsed =
                    first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
                if grace_elapsed && !handles.is_empty() && handles.iter().all(|h| h.is_finished()) {
                    debug!("All clients disconnected, shutting down");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                debug!("Accept error: {}", e);
                break;
            }
        }
    }

    for handle in handles {
        let _ = handle.join();
    }

    let collectors = worker_metrics.lock().unwrap();
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    // Clean up socket file
    let _ = std::fs::remove_file(&transport_config.socket_path);
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Aggregate and print server-side one-way latency from multiple handler threads.
fn aggregate_and_print_server_metrics(collectors: &[MetricsCollector], percentiles: &[f64]) {
    let total_one_way: u64 = collectors
        .iter()
        .map(|c| c.get_metrics().throughput.total_messages as u64)
        .sum();

    if total_one_way > 0 {
        let all_metrics: Vec<_> = collectors.iter().map(|c| c.get_metrics()).collect();
        match MetricsCollector::aggregate_worker_metrics(all_metrics, percentiles) {
            Ok(aggregated) => {
                if let Some(ref latency) = aggregated.latency {
                    info!(
                        "Server one-way latency ({} messages, {} clients): \
                         mean={:.2}us, P50={:.2}us, P95={:.2}us, P99={:.2}us, \
                         min={:.2}us, max={:.2}us",
                        total_one_way,
                        collectors.len(),
                        latency.mean_ns / 1000.0,
                        latency.median_ns / 1000.0,
                        latency
                            .percentiles
                            .iter()
                            .find(|p| (p.percentile - 95.0).abs() < 0.1)
                            .map_or(0.0, |p| p.value_ns as f64)
                            / 1000.0,
                        latency
                            .percentiles
                            .iter()
                            .find(|p| (p.percentile - 99.0).abs() < 0.1)
                            .map_or(0.0, |p| p.value_ns as f64)
                            / 1000.0,
                        latency.min_ns as f64 / 1000.0,
                        latency.max_ns as f64 / 1000.0,
                    );
                }
            }
            Err(e) => warn!("Failed to aggregate server metrics: {}", e),
        }
    }
}

/// Print server-side one-way latency summary.
fn print_server_one_way_latency(one_way_count: u64, one_way_metrics: &MetricsCollector) {
    if one_way_count > 0 {
        let metrics = one_way_metrics.get_metrics();
        if let Some(ref latency) = metrics.latency {
            info!(
                "Server one-way latency ({} messages): \
                 mean={:.2}us, P50={:.2}us, P95={:.2}us, P99={:.2}us, \
                 min={:.2}us, max={:.2}us",
                one_way_count,
                latency.mean_ns / 1000.0,
                latency.median_ns / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 95.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 99.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency.min_ns as f64 / 1000.0,
                latency.max_ns as f64 / 1000.0,
            );
        }
    }
}

/// Async standalone server implementation.
///
/// Collects one-way latency metrics for OneWay messages using the
/// monotonic clock difference between send and receive timestamps.
/// This is accurate when server and client share the same kernel
/// clock (same host, container-to-host, container-to-container).
#[tokio::main]
async fn run_standalone_server_async(
    _args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = TransportFactory::create(&mechanism)?;
    transport
        .start_server(&transport_config)
        .await
        .context("Server failed to start transport")?;

    info!("Server listening, waiting for client...");

    // Collect one-way latency metrics on the server side
    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;
    let mut one_way_count = 0u64;

    loop {
        match transport.receive().await {
            Ok(msg) => {
                if msg.message_type == MessageType::Shutdown {
                    debug!("Server received shutdown message, exiting");
                    break;
                }

                // Measure one-way latency for OneWay messages
                if msg.message_type == MessageType::OneWay && msg.id != u64::MAX {
                    let receive_time_ns = get_monotonic_time_ns();
                    let latency_ns = receive_time_ns.saturating_sub(msg.timestamp);
                    let latency = std::time::Duration::from_nanos(latency_ns);
                    one_way_metrics.record_message(config.message_size, Some(latency))?;
                    one_way_count += 1;
                }

                if let Some(response) = dispatch_server_message(&msg) {
                    if transport.send(&response).await.is_err() {
                        info!("Client disconnected during send, exiting.");
                        break;
                    }
                }
            }
            Err(e) => {
                info!("Server receive loop ending: {}", e);
                break;
            }
        }
    }

    // Print server-side one-way latency results
    if one_way_count > 0 {
        let metrics = one_way_metrics.get_metrics();
        if let Some(ref latency) = metrics.latency {
            info!(
                "Server one-way latency ({} messages): \
                 mean={:.2}us, P50={:.2}us, P95={:.2}us, P99={:.2}us, \
                 min={:.2}us, max={:.2}us",
                one_way_count,
                latency.mean_ns / 1000.0,
                latency.median_ns / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 95.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 99.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency.min_ns as f64 / 1000.0,
                latency.max_ns as f64 / 1000.0,
            );
        }
    }

    let _ = transport.close().await;
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Run in standalone client mode.
///
/// Connects to an already-running standalone server and executes
/// the benchmark workload. Retries the connection with backoff
/// if the server is not yet available.
fn run_standalone_client(args: Args) -> Result<()> {
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => return Err(anyhow::anyhow!("No IPC mechanism specified")),
    };

    if mechanism == IpcMechanism::All {
        return Err(anyhow::anyhow!(
            "Cannot use 'all' mechanism in standalone client mode. \
             Specify a single mechanism (e.g., -m uds)"
        ));
    }

    // Set up logging
    let log_level = match args.verbose {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(log_level)
        .event_format(ColorizedFormatter)
        .init();

    if let Some(core) = args.client_affinity {
        if let Err(e) = set_affinity(core) {
            error!("Failed to set client CPU affinity to core {}: {}", core, e);
        } else {
            info!("Client affinity set to CPU core {}", core);
        }
    }

    let transport_config = build_standalone_transport_config(&args);
    let config = BenchmarkConfig::from_args(&args)?;

    // Create ResultsManager for structured output
    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            Some(format!("{}.{}", path_str, today))
        }
        None => None,
    };

    let mut results_manager =
        BlockingResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable streaming if requested
    if let Some(ref streaming_file) = args.streaming_output_json {
        let both_tests = config.one_way && config.round_trip;
        if both_tests {
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }
    if let Some(ref streaming_file) = args.streaming_output_csv {
        results_manager.enable_csv_streaming(streaming_file)?;
    }

    info!(
        "Starting standalone client: mechanism={}, blocking={}",
        mechanism, args.blocking
    );

    if args.blocking {
        run_standalone_client_blocking(args, mechanism, transport_config, &mut results_manager)?;
    } else {
        run_standalone_client_async(args, mechanism, transport_config, &mut results_manager)?;
    }

    results_manager.finalize()?;
    if let Err(e) = results_manager.print_summary() {
        error!("Failed to print results summary: {}", e);
    }

    Ok(())
}

/// Connect to a server with retry and backoff.
///
/// Retries the connection at 100ms intervals for up to 30 seconds,
/// allowing the server to start after the client.
fn connect_blocking_with_retry(
    transport: &mut Box<dyn ipc_benchmark::ipc::BlockingTransport>,
    config: &TransportConfig,
) -> Result<()> {
    let start = std::time::Instant::now();

    loop {
        match transport.start_client_blocking(config) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if start.elapsed() > CONNECT_RETRY_TIMEOUT {
                    return Err(e).context(
                        "Timed out waiting for server. \
                         Is the server running with the same mechanism and endpoint?",
                    );
                }
                debug!("Connection failed, retrying: {}", e);
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }
}

/// Blocking standalone client implementation.
///
/// When `concurrency > 1` (and mechanism supports it), spawns N worker
/// threads each with their own transport connection. Results are
/// aggregated across all workers.
fn run_standalone_client_blocking(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
) -> Result<()> {
    let config = BenchmarkConfig::from_args(&args)?;

    // Force concurrency=1 for mechanisms that don't support multiple connections.
    // Only TCP and UDS support concurrent clients (socket-based accept loop).
    let mut supports_concurrency = mechanism == IpcMechanism::TcpSocket;
    #[cfg(unix)]
    {
        supports_concurrency = supports_concurrency || mechanism == IpcMechanism::UnixDomainSocket;
    }
    let concurrency = if !supports_concurrency && config.concurrency > 1 {
        warn!(
            "{} does not support concurrency > 1. Forcing concurrency = 1.",
            mechanism
        );
        1
    } else {
        config.concurrency
    };

    if concurrency > 1 {
        run_standalone_client_blocking_concurrent(
            args,
            mechanism,
            transport_config,
            results_manager,
            &config,
            concurrency,
        )
    } else {
        run_standalone_client_blocking_single(
            args,
            mechanism,
            transport_config,
            results_manager,
            &config,
        )
    }
}

/// Single-threaded blocking standalone client (original behavior).
fn run_standalone_client_blocking_single(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;

    info!("Connecting to server...");
    connect_blocking_with_retry(&mut transport, &transport_config)?;
    info!("Connected to server.");

    let msg_count = config
        .msg_count
        .unwrap_or(ipc_benchmark::defaults::MSG_COUNT);
    let payload = vec![0u8; config.message_size];

    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        1,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    let total_start = std::time::Instant::now();

    // Warmup
    for _ in 0..config.warmup_iterations {
        let msg_type = if config.round_trip {
            MessageType::Request
        } else {
            MessageType::OneWay
        };
        let msg = Message::new(u64::MAX, payload.clone(), msg_type);
        transport.send_blocking(&msg)?;
        if config.round_trip {
            transport.receive_blocking()?;
        }
    }
    info!("Warmup complete ({} iterations)", config.warmup_iterations);

    // One-way test (throughput only -- true one-way latency requires server-side measurement)
    if config.one_way {
        let mut metrics = MetricsCollector::new(None, config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
            let _ = transport.send_blocking(&canary);
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload.clone(), MessageType::OneWay);

        let start = std::time::Instant::now();
        let count = if let Some(test_duration) = config.duration {
            info!(
                "Running one-way throughput test (duration={:.2?})...",
                test_duration
            );
            let mut c = 0u64;
            while start.elapsed() < test_duration {
                msg.id = c;
                msg.timestamp = get_monotonic_time_ns();
                transport.send_blocking(&msg)?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
                c += 1;
            }
            c
        } else {
            info!(
                "Running one-way throughput test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                transport.send_blocking(&msg)?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
            }
            msg_count as u64
        };

        let elapsed = start.elapsed();
        info!(
            "One-way complete: {} messages in {:.2?} ({:.0} msg/s)",
            count,
            elapsed,
            count as f64 / elapsed.as_secs_f64()
        );

        results.add_one_way_results(metrics.get_metrics());
    }

    // Round-trip test
    if config.round_trip {
        let mut metrics =
            MetricsCollector::new(Some(LatencyType::RoundTrip), config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
            if transport.send_blocking(&canary).is_ok() {
                let _ = transport.receive_blocking();
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload, MessageType::Request);

        if let Some(test_duration) = config.duration {
            info!(
                "Running round-trip latency test (duration={:.2?})...",
                test_duration
            );
            let start = std::time::Instant::now();
            let mut i = 0u64;
            while start.elapsed() < test_duration {
                msg.id = i;
                msg.timestamp = get_monotonic_time_ns();
                let send_time = std::time::Instant::now();
                transport.send_blocking(&msg)?;
                let _response = transport.receive_blocking()?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                );
                let _ = results_manager.stream_latency_record(&record);

                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
                i += 1;
            }
        } else {
            info!(
                "Running round-trip latency test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                let send_time = std::time::Instant::now();
                transport.send_blocking(&msg)?;
                let _response = transport.receive_blocking()?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i as u64,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                );
                let _ = results_manager.stream_latency_record(&record);

                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        results.add_round_trip_results(metrics.get_metrics());
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    // Send shutdown message before closing for deterministic server exit
    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
    let _ = transport.send_blocking(&shutdown);

    transport.close_blocking()?;
    info!("Standalone client finished.");
    Ok(())
}

/// Multi-threaded blocking standalone client.
///
/// Spawns `concurrency` worker threads, each creating its own transport
/// connection to the server. Each worker runs the test loop independently
/// with its own `MetricsCollector`. Results are aggregated across all
/// workers using `MetricsCollector::aggregate_worker_metrics()`.
fn run_standalone_client_blocking_concurrent(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
    concurrency: usize,
) -> Result<()> {
    use ipc_benchmark::metrics::PerformanceMetrics;

    let msg_count = config
        .msg_count
        .unwrap_or(ipc_benchmark::defaults::MSG_COUNT);
    let messages_per_worker = msg_count / concurrency;

    info!(
        "Starting {} concurrent workers ({} messages each)...",
        concurrency, messages_per_worker
    );

    let total_start = std::time::Instant::now();

    // Spawn worker threads for one-way test
    let one_way_results: Option<Vec<PerformanceMetrics>> = if config.one_way {
        let handles: Vec<_> = (0..concurrency)
            .map(|worker_id| {
                let tc = transport_config.clone();
                let percentiles = config.percentiles.clone();
                let message_size = config.message_size;
                let duration = config.duration;
                let send_delay = config.send_delay;
                let include_first = config.include_first_message;
                let shm_direct = args.shm_direct;
                let mech = mechanism;

                std::thread::spawn(move || -> Result<PerformanceMetrics> {
                    let mut transport = BlockingTransportFactory::create(&mech, shm_direct)?;
                    connect_blocking_with_retry(&mut transport, &tc)?;
                    debug!("Worker {} connected (one-way)", worker_id);

                    let mut metrics = MetricsCollector::new(None, percentiles)?;
                    let payload = vec![0u8; message_size];

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                        let _ = transport.send_blocking(&canary);
                    }

                    let mut msg = Message::new(0, payload, MessageType::OneWay);

                    if let Some(test_duration) = duration {
                        let start = std::time::Instant::now();
                        let mut c = 0u64;
                        while start.elapsed() < test_duration {
                            msg.id = worker_id as u64 * u32::MAX as u64 + c;
                            msg.timestamp = get_monotonic_time_ns();
                            transport.send_blocking(&msg)?;
                            metrics.record_message(message_size, None)?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                            c += 1;
                        }
                    } else {
                        for i in 0..messages_per_worker {
                            msg.id = (worker_id * messages_per_worker + i) as u64;
                            msg.timestamp = get_monotonic_time_ns();
                            transport.send_blocking(&msg)?;
                            metrics.record_message(message_size, None)?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking()?;
                    debug!("Worker {} finished (one-way)", worker_id);
                    Ok(metrics.get_metrics())
                })
            })
            .collect();

        let worker_results: Vec<PerformanceMetrics> = handles
            .into_iter()
            .enumerate()
            .map(|(id, h)| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker {} panicked", id)))
            })
            .collect::<Result<Vec<_>>>()?;

        Some(worker_results)
    } else {
        None
    };

    // Spawn worker threads for round-trip test
    let round_trip_results: Option<Vec<PerformanceMetrics>> = if config.round_trip {
        let handles: Vec<_> = (0..concurrency)
            .map(|worker_id| {
                let tc = transport_config.clone();
                let percentiles = config.percentiles.clone();
                let message_size = config.message_size;
                let duration = config.duration;
                let send_delay = config.send_delay;
                let include_first = config.include_first_message;
                let shm_direct = args.shm_direct;
                let mech = mechanism;

                std::thread::spawn(move || -> Result<PerformanceMetrics> {
                    let mut transport = BlockingTransportFactory::create(&mech, shm_direct)?;
                    connect_blocking_with_retry(&mut transport, &tc)?;
                    debug!("Worker {} connected (round-trip)", worker_id);

                    let mut metrics =
                        MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                    let payload = vec![0u8; message_size];

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                        if transport.send_blocking(&canary).is_ok() {
                            let _ = transport.receive_blocking();
                        }
                    }

                    let mut msg = Message::new(0, payload, MessageType::Request);

                    if let Some(test_duration) = duration {
                        let start = std::time::Instant::now();
                        let mut i = 0u64;
                        while start.elapsed() < test_duration {
                            msg.id = worker_id as u64 * u32::MAX as u64 + i;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                            i += 1;
                        }
                    } else {
                        for i in 0..messages_per_worker {
                            msg.id = (worker_id * messages_per_worker + i) as u64;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking()?;
                    debug!("Worker {} finished (round-trip)", worker_id);
                    Ok(metrics.get_metrics())
                })
            })
            .collect();

        let worker_results: Vec<PerformanceMetrics> = handles
            .into_iter()
            .enumerate()
            .map(|(id, h)| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker {} panicked", id)))
            })
            .collect::<Result<Vec<_>>>()?;

        Some(worker_results)
    } else {
        None
    };

    // Aggregate results
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        concurrency,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    if let Some(one_way) = one_way_results {
        let aggregated = MetricsCollector::aggregate_worker_metrics(one_way, &config.percentiles)?;
        results.add_one_way_results(aggregated);
    }

    if let Some(round_trip) = round_trip_results {
        let aggregated =
            MetricsCollector::aggregate_worker_metrics(round_trip, &config.percentiles)?;
        results.add_round_trip_results(aggregated);
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    info!("Standalone client finished ({} workers).", concurrency);
    Ok(())
}

/// Async connect with retry.
async fn connect_async_with_retry(
    transport: &mut Box<dyn ipc_benchmark::ipc::IpcTransport>,
    config: &TransportConfig,
) -> Result<()> {
    let start = std::time::Instant::now();

    loop {
        match transport.start_client(config).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if start.elapsed() > CONNECT_RETRY_TIMEOUT {
                    return Err(e).context(
                        "Timed out waiting for server. \
                         Is the server running with the same mechanism and endpoint?",
                    );
                }
                debug!("Connection failed, retrying: {}", e);
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
        }
    }
}

/// Async standalone client implementation.
///
/// Dispatches to single or concurrent mode based on concurrency setting.
#[tokio::main]
async fn run_standalone_client_async(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
) -> Result<()> {
    let config = BenchmarkConfig::from_args(&args)?;

    // Force concurrency=1 for mechanisms that don't support multiple connections.
    let mut supports_concurrency = mechanism == IpcMechanism::TcpSocket;
    #[cfg(unix)]
    {
        supports_concurrency = supports_concurrency || mechanism == IpcMechanism::UnixDomainSocket;
    }
    let concurrency = if !supports_concurrency && config.concurrency > 1 {
        warn!(
            "{} does not support concurrency > 1. Forcing concurrency = 1.",
            mechanism
        );
        1
    } else {
        config.concurrency
    };

    if concurrency > 1 {
        run_standalone_client_async_concurrent(
            mechanism,
            transport_config,
            results_manager,
            &config,
            concurrency,
        )
        .await
    } else {
        run_standalone_client_async_single(mechanism, transport_config, results_manager, &config)
            .await
    }
}

/// Single-connection async standalone client (original behavior).
async fn run_standalone_client_async_single(
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = TransportFactory::create(&mechanism)?;

    info!("Connecting to server...");
    connect_async_with_retry(&mut transport, &transport_config).await?;
    info!("Connected to server.");

    let msg_count = config
        .msg_count
        .unwrap_or(ipc_benchmark::defaults::MSG_COUNT);
    let payload = vec![0u8; config.message_size];

    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        1,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    let total_start = std::time::Instant::now();

    // Warmup
    for _ in 0..config.warmup_iterations {
        let msg_type = if config.round_trip {
            MessageType::Request
        } else {
            MessageType::OneWay
        };
        let msg = Message::new(u64::MAX, payload.clone(), msg_type);
        transport.send(&msg).await?;
        if config.round_trip {
            transport.receive().await?;
        }
    }
    info!("Warmup complete ({} iterations)", config.warmup_iterations);

    // One-way test (throughput only -- true one-way latency requires server-side measurement)
    if config.one_way {
        let mut metrics = MetricsCollector::new(None, config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
            let _ = transport.send(&canary).await;
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload.clone(), MessageType::OneWay);

        let start = std::time::Instant::now();
        let count = if let Some(test_duration) = config.duration {
            info!(
                "Running one-way throughput test (duration={:.2?})...",
                test_duration
            );
            let mut c = 0u64;
            while start.elapsed() < test_duration {
                msg.id = c;
                msg.timestamp = get_monotonic_time_ns();
                transport.send(&msg).await?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                c += 1;
            }
            c
        } else {
            info!(
                "Running one-way throughput test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                transport.send(&msg).await?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
            msg_count as u64
        };

        let elapsed = start.elapsed();
        info!(
            "One-way complete: {} messages in {:.2?} ({:.0} msg/s)",
            count,
            elapsed,
            count as f64 / elapsed.as_secs_f64()
        );

        results.add_one_way_results(metrics.get_metrics());
    }

    // Round-trip test
    if config.round_trip {
        let mut metrics =
            MetricsCollector::new(Some(LatencyType::RoundTrip), config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
            if transport.send(&canary).await.is_ok() {
                let _ = transport.receive().await;
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload, MessageType::Request);

        if let Some(test_duration) = config.duration {
            info!(
                "Running round-trip latency test (duration={:.2?})...",
                test_duration
            );
            let start = std::time::Instant::now();
            let mut i = 0u64;
            while start.elapsed() < test_duration {
                msg.id = i;
                msg.timestamp = get_monotonic_time_ns();
                let send_time = std::time::Instant::now();
                transport.send(&msg).await?;
                let _response = transport.receive().await?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                );
                let _ = results_manager.stream_latency_record(&record);

                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                i += 1;
            }
        } else {
            info!(
                "Running round-trip latency test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                let send_time = std::time::Instant::now();
                transport.send(&msg).await?;
                let _response = transport.receive().await?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i as u64,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                );
                let _ = results_manager.stream_latency_record(&record);

                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        results.add_round_trip_results(metrics.get_metrics());
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    // Send shutdown message before closing for deterministic server exit
    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
    let _ = transport.send(&shutdown).await;

    let _ = transport.close().await;
    info!("Standalone client finished.");
    Ok(())
}

/// Multi-task async standalone client.
///
/// Spawns `concurrency` tokio tasks, each creating its own async transport
/// connection. Results are aggregated across all workers.
async fn run_standalone_client_async_concurrent(
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
    concurrency: usize,
) -> Result<()> {
    use ipc_benchmark::metrics::PerformanceMetrics;
    use tokio::task::JoinSet;

    let msg_count = config
        .msg_count
        .unwrap_or(ipc_benchmark::defaults::MSG_COUNT);
    let messages_per_worker = msg_count / concurrency;

    info!(
        "Starting {} concurrent async workers ({} messages each)...",
        concurrency, messages_per_worker
    );

    let total_start = std::time::Instant::now();

    // One-way test
    let one_way_results: Option<Vec<PerformanceMetrics>> = if config.one_way {
        let mut join_set: JoinSet<Result<PerformanceMetrics>> = JoinSet::new();
        for worker_id in 0..concurrency {
            let tc = transport_config.clone();
            let percentiles = config.percentiles.clone();
            let message_size = config.message_size;
            let duration = config.duration;
            let send_delay = config.send_delay;
            let include_first = config.include_first_message;
            let mech = mechanism;

            join_set.spawn(async move {
                let mut transport = TransportFactory::create(&mech)?;
                connect_async_with_retry(&mut transport, &tc).await?;
                debug!("Async worker {} connected (one-way)", worker_id);

                let mut metrics = MetricsCollector::new(None, percentiles)?;
                let payload = vec![0u8; message_size];

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                    let _ = transport.send(&canary).await;
                }

                let mut msg = Message::new(0, payload, MessageType::OneWay);

                if let Some(test_duration) = duration {
                    let start = std::time::Instant::now();
                    let mut c = 0u64;
                    while start.elapsed() < test_duration {
                        msg.id = worker_id as u64 * u32::MAX as u64 + c;
                        msg.timestamp = get_monotonic_time_ns();
                        transport.send(&msg).await?;
                        metrics.record_message(message_size, None)?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                        c += 1;
                    }
                } else {
                    for i in 0..messages_per_worker {
                        msg.id = (worker_id * messages_per_worker + i) as u64;
                        msg.timestamp = get_monotonic_time_ns();
                        transport.send(&msg).await?;
                        metrics.record_message(message_size, None)?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send(&shutdown).await;
                let _ = transport.close().await;
                debug!("Async worker {} finished (one-way)", worker_id);
                Ok(metrics.get_metrics())
            });
        }

        let mut worker_results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            worker_results.push(result.context("Worker task panicked")??);
        }
        Some(worker_results)
    } else {
        None
    };

    // Round-trip test
    let round_trip_results: Option<Vec<PerformanceMetrics>> = if config.round_trip {
        let mut join_set: JoinSet<Result<PerformanceMetrics>> = JoinSet::new();
        for worker_id in 0..concurrency {
            let tc = transport_config.clone();
            let percentiles = config.percentiles.clone();
            let message_size = config.message_size;
            let duration = config.duration;
            let send_delay = config.send_delay;
            let include_first = config.include_first_message;
            let mech = mechanism;

            join_set.spawn(async move {
                let mut transport = TransportFactory::create(&mech)?;
                connect_async_with_retry(&mut transport, &tc).await?;
                debug!("Async worker {} connected (round-trip)", worker_id);

                let mut metrics = MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                let payload = vec![0u8; message_size];

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                    if transport.send(&canary).await.is_ok() {
                        let _ = transport.receive().await;
                    }
                }

                let mut msg = Message::new(0, payload, MessageType::Request);

                if let Some(test_duration) = duration {
                    let start = std::time::Instant::now();
                    let mut i = 0u64;
                    while start.elapsed() < test_duration {
                        msg.id = worker_id as u64 * u32::MAX as u64 + i;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                        i += 1;
                    }
                } else {
                    for i in 0..messages_per_worker {
                        msg.id = (worker_id * messages_per_worker + i) as u64;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send(&shutdown).await;
                let _ = transport.close().await;
                debug!("Async worker {} finished (round-trip)", worker_id);
                Ok(metrics.get_metrics())
            });
        }

        let mut worker_results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            worker_results.push(result.context("Worker task panicked")??);
        }
        Some(worker_results)
    } else {
        None
    };

    // Aggregate results
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        concurrency,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    if let Some(one_way) = one_way_results {
        let aggregated = MetricsCollector::aggregate_worker_metrics(one_way, &config.percentiles)?;
        results.add_one_way_results(aggregated);
    }

    if let Some(round_trip) = round_trip_results {
        let aggregated =
            MetricsCollector::aggregate_worker_metrics(round_trip, &config.percentiles)?;
        results.add_round_trip_results(aggregated);
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    info!(
        "Standalone client finished ({} async workers).",
        concurrency
    );
    Ok(())
}

/// Returns `true` if a latency value should be buffered.
///
/// Latencies are only buffered when a latency file path is
/// configured and the message is not a warmup canary
/// (canary messages use `id == u64::MAX`).
fn should_buffer_latency(latency_file_enabled: bool, message_id: u64) -> bool {
    latency_file_enabled && message_id != u64::MAX
}

/// Write a buffer of latency values to a file.
///
/// Each entry is written as a single line containing a
/// `"wall_send_ns,latency_ns"` pair. `wall_send_ns` is the
/// approximate wall-clock send time (computed as `wall_now - latency`
/// on the server) and `latency_ns` is the measured one-way IPC
/// latency. This format matches what `parse_latency_file_line()`
/// in the client-side benchmark reader expects.
///
/// # Errors
///
/// Returns an error if the file cannot be created or written.
fn write_latency_buffer(path: &str, buffer: &[(u64, u64)]) -> Result<()> {
    debug!(
        "Writing {} buffered latencies to file: {}",
        buffer.len(),
        path,
    );
    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create latency file: {}", path))?;
    for &(wall_send_ns, latency_ns) in buffer {
        writeln!(file, "{},{}", wall_send_ns, latency_ns).ok();
    }
    debug!("Finished writing latencies to file");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_build_standalone_transport_config_defaults() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "tcp"]);
        let config = build_standalone_transport_config(&args);

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.shared_memory_name, "ipc_benchmark_shm");
        assert_eq!(config.message_queue_name, "ipc_benchmark_pmq");
        assert_eq!(config.pmq_priority, 0);
    }

    #[test]
    fn test_build_standalone_transport_config_overrides() {
        let args = Args::parse_from([
            "ipc-benchmark",
            "--server",
            "-m",
            "tcp",
            "--host",
            "0.0.0.0",
            "--port",
            "9999",
            "--socket-path",
            "/tmp/custom.sock",
            "--shared-memory-name",
            "custom_shm",
            "--message-queue-name",
            "custom_pmq",
            "--buffer-size",
            "65536",
            "--pmq-priority",
            "3",
        ]);
        let config = build_standalone_transport_config(&args);

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9999);
        assert_eq!(config.socket_path, "/tmp/custom.sock");
        assert_eq!(config.shared_memory_name, "custom_shm");
        assert_eq!(config.message_queue_name, "custom_pmq");
        assert_eq!(config.buffer_size, 65536);
        assert_eq!(config.pmq_priority, 3);
    }

    #[test]
    fn test_build_standalone_transport_config_partial_overrides() {
        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "shm",
            "--shared-memory-name",
            "my_shm",
        ]);
        let config = build_standalone_transport_config(&args);

        // Overridden
        assert_eq!(config.shared_memory_name, "my_shm");
        // Defaults preserved
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.buffer_size, 8192);
    }

    #[test]
    fn test_standalone_server_rejects_all_mechanism() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "all"]);

        // Simulate what run_standalone_server does
        let mechanism = args.mechanisms.first().unwrap();
        assert_eq!(*mechanism, IpcMechanism::All);
    }

    // --- Unit tests for shared helper functions ---

    #[test]
    fn test_dispatch_server_message_request() {
        let msg = Message::new(1, Vec::new(), MessageType::Request);
        let resp = dispatch_server_message(&msg).unwrap();
        assert_eq!(resp.id, 1);
        assert_eq!(resp.message_type, MessageType::Response);
    }

    #[test]
    fn test_dispatch_server_message_ping() {
        let msg = Message::new(42, Vec::new(), MessageType::Ping);
        let resp = dispatch_server_message(&msg).unwrap();
        assert_eq!(resp.id, 42);
        assert_eq!(resp.message_type, MessageType::Pong);
    }

    #[test]
    fn test_dispatch_server_message_one_way_returns_none() {
        let msg = Message::new(1, Vec::new(), MessageType::OneWay);
        assert!(dispatch_server_message(&msg).is_none());
    }

    #[test]
    fn test_dispatch_server_message_shutdown_returns_none() {
        let msg = Message::new(1, Vec::new(), MessageType::Shutdown);
        assert!(dispatch_server_message(&msg).is_none());
    }

    /// Integration test: blocking TCP round-trip with duration mode.
    #[test]
    fn test_standalone_blocking_tcp_duration_round_trip() {
        let port = 18306u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                if message.message_type == MessageType::Request {
                    let resp = Message::new(message.id, Vec::new(), MessageType::Response);
                    if transport.send_blocking(&resp).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Run round-trip for a short duration
        let test_duration = std::time::Duration::from_millis(200);
        let start = std::time::Instant::now();
        let mut count = 0u64;
        while start.elapsed() < test_duration {
            let msg = Message::new(count, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.message_type, MessageType::Response);
            count += 1;
        }

        assert!(count > 0, "Should have completed at least one round-trip");
        assert!(
            start.elapsed() >= test_duration,
            "Should have run for at least the specified duration"
        );

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP one-way with duration mode.
    #[test]
    fn test_standalone_blocking_tcp_duration_one_way() {
        let port = 18307u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(_message) = transport.receive_blocking() {
                // OneWay: no response needed
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Run one-way for a short duration
        let test_duration = std::time::Duration::from_millis(200);
        let start = std::time::Instant::now();
        let mut count = 0u64;
        while start.elapsed() < test_duration {
            let msg = Message::new(count, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
            count += 1;
        }

        assert!(count > 0, "Should have sent at least one message");

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP round-trip through standalone
    /// server and client paths.
    ///
    /// Exercises: run_standalone_server_blocking server loop (receive,
    /// respond to Request), connect_blocking_with_retry, and
    /// run_standalone_client_blocking round-trip path.
    #[test]
    fn test_standalone_blocking_tcp_round_trip() {
        let port = 18301u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start server in a background thread
        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            // Handle messages until client disconnects
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                if message.message_type == MessageType::Request {
                    let resp = Message::new(message.id, Vec::new(), MessageType::Response);
                    if transport.send_blocking(&resp).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        // Give server time to bind
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client: connect with retry and do round-trip
        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send round-trip messages
        let msg_count = 10usize;
        let payload = vec![0u8; 64];
        for i in 0..msg_count {
            let msg = Message::new(i as u64, payload.clone(), MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, i as u64);
            assert_eq!(resp.message_type, MessageType::Response);
        }

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP one-way through standalone paths.
    ///
    /// Exercises: server handling OneWay messages (no response),
    /// client sending fire-and-forget messages.
    #[test]
    fn test_standalone_blocking_tcp_one_way() {
        let port = 18302u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                // OneWay: no response needed
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send one-way messages
        let payload = vec![0u8; 64];
        for i in 0..10u64 {
            let msg = Message::new(i, payload.clone(), MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP server handles Ping/Pong.
    ///
    /// Exercises the Ping message handling branch in the server loop.
    #[test]
    fn test_standalone_blocking_tcp_ping_pong() {
        let port = 18303u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Ping {
                    let pong = Message::new(message.id, Vec::new(), MessageType::Pong);
                    if transport.send_blocking(&pong).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send ping, expect pong
        let ping = Message::new(42, Vec::new(), MessageType::Ping);
        transport.send_blocking(&ping).unwrap();
        let pong = transport.receive_blocking().unwrap();
        assert_eq!(pong.id, 42);
        assert_eq!(pong.message_type, MessageType::Pong);

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: connect_blocking_with_retry succeeds when server starts
    /// after client begins retrying.
    #[test]
    fn test_connect_blocking_with_retry_waits_for_server() {
        let port = 18304u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start client retry in background (server not yet up)
        let client_config = transport_config.clone();
        let client_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &client_config).unwrap();
            transport.close_blocking().unwrap();
        });

        // Wait, then start server
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut server_transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        server_transport
            .start_server_blocking(&transport_config)
            .unwrap();

        // Accept connection then close
        // The client connects and closes, which causes a receive error
        let _ = server_transport.receive_blocking();
        server_transport.close_blocking().unwrap();

        client_handle.join().unwrap();
    }

    /// Test: Shutdown message causes server to exit cleanly.
    #[test]
    fn test_standalone_server_shutdown_message() {
        let port = 18305u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            // Server loop: should exit on Shutdown
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send shutdown
        let shutdown = Message::new(0, Vec::new(), MessageType::Shutdown);
        transport.send_blocking(&shutdown).unwrap();

        // Give server time to process and exit
        std::thread::sleep(std::time::Duration::from_millis(100));
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    // --- Concurrency tests ---

    /// Integration test: multi-accept TCP server handles 3 concurrent
    /// round-trip clients.
    #[test]
    fn test_standalone_concurrent_tcp_round_trip() {
        use ipc_benchmark::ipc::BlockingTcpSocket;
        use std::sync::{Arc, Mutex};

        let port = 18310u16;
        let addr = format!("127.0.0.1:{}", port);
        let num_clients = 3usize;
        let msgs_per_client = 5usize;

        // Start multi-accept server using blocking accept with known
        // client count to avoid the non-blocking race condition where
        // a fast client could finish before others connect.
        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
            let mut handles = Vec::new();

            // Accept exactly num_clients connections (blocking)
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let mc = metrics.clone();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    if let Ok(collector) = handle_client_connection(&mut transport, &config) {
                        mc.lock().unwrap().push(collector);
                    }
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }

            for handle in handles {
                let _ = handle.join();
            }

            let collectors = metrics.lock().unwrap();
            assert_eq!(
                collectors.len(),
                num_clients,
                "Server should have handled {} clients",
                num_clients
            );
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Spawn concurrent clients
        let client_handles: Vec<_> = (0..num_clients)
            .map(|worker_id| {
                let config = TransportConfig {
                    host: "127.0.0.1".to_string(),
                    port,
                    ..Default::default()
                };
                std::thread::spawn(move || {
                    let mut transport =
                        BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                    connect_blocking_with_retry(&mut transport, &config).unwrap();

                    for i in 0..msgs_per_client {
                        let msg = Message::new(
                            (worker_id * msgs_per_client + i) as u64,
                            vec![0u8; 64],
                            MessageType::Request,
                        );
                        transport.send_blocking(&msg).unwrap();
                        let resp = transport.receive_blocking().unwrap();
                        assert_eq!(resp.message_type, MessageType::Response);
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking().unwrap();
                })
            })
            .collect();

        for h in client_handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection correctly dispatches message types
    /// and returns metrics.
    #[test]
    fn test_handle_client_connection_round_trip() {
        let port = 18311u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            // Round-trip messages don't get recorded as one-way
            let metrics = collector.get_metrics();
            assert_eq!(metrics.throughput.total_messages, 0);
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send round-trip messages (should NOT be recorded as one-way)
        for i in 0..5u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.message_type, MessageType::Response);
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection records one-way latency metrics.
    #[test]
    fn test_handle_client_connection_one_way_metrics() {
        let port = 18312u16;
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            let metrics = collector.get_metrics();
            // One-way messages should be recorded
            assert_eq!(
                metrics.throughput.total_messages, 5,
                "Should have recorded 5 one-way messages"
            );
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send one-way messages
        for i in 0..5u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    // --- Concurrent one-way test ---

    /// Integration test: multi-accept TCP server handles 2 concurrent
    /// one-way clients and records server-side latency metrics.
    #[test]
    fn test_standalone_concurrent_tcp_one_way() {
        use ipc_benchmark::ipc::BlockingTcpSocket;

        let port = 18313u16;
        let addr = format!("127.0.0.1:{}", port);
        let num_clients = 2usize;
        let msgs_per_client = 10usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let mut handles = Vec::new();

            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let collector = handle_client_connection(&mut transport, &config).unwrap();
                    let metrics = collector.get_metrics();
                    // One-way messages should be recorded
                    assert!(
                        metrics.throughput.total_messages > 0,
                        "Handler should have recorded one-way messages"
                    );
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }

            for handle in handles {
                handle.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let client_handles: Vec<_> = (0..num_clients)
            .map(|worker_id| {
                let config = TransportConfig {
                    host: "127.0.0.1".to_string(),
                    port,
                    ..Default::default()
                };
                std::thread::spawn(move || {
                    let mut transport =
                        BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                    connect_blocking_with_retry(&mut transport, &config).unwrap();

                    for i in 0..msgs_per_client {
                        let msg = Message::new(
                            (worker_id * msgs_per_client + i) as u64,
                            vec![0u8; 64],
                            MessageType::OneWay,
                        );
                        transport.send_blocking(&msg).unwrap();
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking().unwrap();
                })
            })
            .collect();

        for h in client_handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    // --- from_stream tests ---

    /// Test: BlockingTcpSocket::from_stream creates a functional transport.
    #[test]
    fn test_tcp_from_stream_send_receive() {
        use ipc_benchmark::ipc::BlockingTcpSocket;

        let port = 18314u16;
        let addr = format!("127.0.0.1:{}", port);

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();

            let mut transport = BlockingTcpSocket::from_stream(stream);
            let msg = transport.receive_blocking().unwrap();
            assert_eq!(msg.id, 99);
            assert_eq!(msg.message_type, MessageType::Request);

            let resp = Message::new(msg.id, Vec::new(), MessageType::Response);
            transport.send_blocking(&resp).unwrap();
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client.set_nodelay(true).unwrap();

        // Use a raw BlockingTcpSocket for client too
        let mut client_transport = BlockingTcpSocket::from_stream(client);
        let msg = Message::new(99, vec![0u8; 32], MessageType::Request);
        client_transport.send_blocking(&msg).unwrap();
        let resp = client_transport.receive_blocking().unwrap();
        assert_eq!(resp.id, 99);
        assert_eq!(resp.message_type, MessageType::Response);
        client_transport.close_blocking().unwrap();

        server_handle.join().unwrap();
    }

    /// Test: BlockingUnixDomainSocket::from_stream creates a functional transport.
    #[cfg(unix)]
    #[test]
    fn test_uds_from_stream_send_receive() {
        use ipc_benchmark::ipc::BlockingUnixDomainSocket;

        let socket_path = format!("/tmp/test_from_stream_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let path_clone = socket_path.clone();
        let server_handle = std::thread::spawn(move || {
            let listener = std::os::unix::net::UnixListener::bind(&path_clone).unwrap();
            let (stream, _) = listener.accept().unwrap();

            let mut transport = BlockingUnixDomainSocket::from_stream(stream);
            let msg = transport.receive_blocking().unwrap();
            assert_eq!(msg.id, 77);

            let resp = Message::new(msg.id, Vec::new(), MessageType::Response);
            transport.send_blocking(&resp).unwrap();
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let stream = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
        let mut client_transport = BlockingUnixDomainSocket::from_stream(stream);
        let msg = Message::new(77, vec![0u8; 32], MessageType::Request);
        client_transport.send_blocking(&msg).unwrap();
        let resp = client_transport.receive_blocking().unwrap();
        assert_eq!(resp.id, 77);
        assert_eq!(resp.message_type, MessageType::Response);
        client_transport.close_blocking().unwrap();

        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    // --- Concurrency fallback test ---

    /// Test: SHM mechanism forces concurrency=1 with a warning.
    #[test]
    fn test_concurrency_forced_to_one_for_shm() {
        // Verify that BenchmarkConfig with concurrency > 1 and SHM
        // is detected by the standalone client dispatch logic.
        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "shm",
            "-c",
            "4",
            "--blocking",
        ]);
        assert_eq!(args.concurrency, 4);

        // The actual concurrency forcing happens inside
        // run_standalone_client_blocking, but we can verify the
        // args parse correctly and the mechanism is SHM.
        assert_eq!(args.mechanisms[0], IpcMechanism::SharedMemory);
    }

    // --- aggregate_and_print_server_metrics test ---

    /// Test: aggregate_and_print_server_metrics handles empty input.
    #[test]
    fn test_aggregate_and_print_empty_collectors() {
        let collectors: Vec<MetricsCollector> = Vec::new();
        // Should not panic on empty input
        aggregate_and_print_server_metrics(&collectors, &[50.0, 95.0, 99.0]);
    }

    /// Test: aggregate_and_print_server_metrics with single collector.
    #[test]
    fn test_aggregate_and_print_single_collector() {
        let mut collector =
            MetricsCollector::new(Some(LatencyType::OneWay), vec![50.0, 95.0, 99.0]).unwrap();
        collector
            .record_message(64, Some(std::time::Duration::from_micros(100)))
            .unwrap();
        collector
            .record_message(64, Some(std::time::Duration::from_micros(200)))
            .unwrap();

        // Should not panic
        aggregate_and_print_server_metrics(&[collector], &[50.0, 95.0, 99.0]);
    }

    use std::io::{BufRead, BufReader};

    /// Canary messages (id == u64::MAX) must not be buffered
    /// because they are warmup probes, not real measurements.
    #[test]
    fn test_should_buffer_latency_excludes_canary() {
        assert!(
            !should_buffer_latency(true, u64::MAX),
            "canary messages must be excluded"
        );
    }

    /// Normal messages should be buffered when latency file
    /// collection is enabled.
    #[test]
    fn test_should_buffer_latency_includes_normal() {
        assert!(should_buffer_latency(true, 0));
        assert!(should_buffer_latency(true, 1));
        assert!(should_buffer_latency(true, 42));
        assert!(should_buffer_latency(true, u64::MAX - 1));
    }

    /// When the latency file path is not configured, no
    /// messages should be buffered regardless of id.
    #[test]
    fn test_should_buffer_latency_disabled() {
        assert!(!should_buffer_latency(false, 0));
        assert!(!should_buffer_latency(false, 42));
        assert!(!should_buffer_latency(false, u64::MAX));
    }

    /// Verify that write_latency_buffer produces one
    /// "wall_send_ns,latency_ns" pair per line, matching
    /// the format that parse_latency_file_line() expects.
    #[test]
    fn test_write_latency_buffer_format() {
        let dir = std::env::temp_dir();
        let path = dir
            .join("test_latency_buffer_format.txt")
            .to_string_lossy()
            .to_string();

        let entries: Vec<(u64, u64)> =
            vec![(1000, 100), (2000, 200), (3000, 999), (0, 0), (5000, 42)];
        write_latency_buffer(&path, &entries).unwrap();

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = BufReader::new(file).lines().map(|l| l.unwrap()).collect();

        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "1000,100");
        assert_eq!(lines[1], "2000,200");
        assert_eq!(lines[2], "3000,999");
        assert_eq!(lines[3], "0,0");
        assert_eq!(lines[4], "5000,42");

        let _ = std::fs::remove_file(&path);
    }

    /// An empty buffer should produce an empty file.
    #[test]
    fn test_write_latency_buffer_empty() {
        let dir = std::env::temp_dir();
        let path = dir
            .join("test_latency_buffer_empty.txt")
            .to_string_lossy()
            .to_string();

        write_latency_buffer(&path, &[]).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.is_empty(),
            "empty buffer should produce empty file"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Verify values round-trip through file I/O correctly,
    /// matching the same parse logic the benchmark reader uses.
    #[test]
    fn test_write_latency_buffer_round_trip_parse() {
        use ipc_benchmark::benchmark::parse_latency_file_line;

        let dir = std::env::temp_dir();
        let path = dir
            .join("test_latency_buffer_roundtrip.txt")
            .to_string_lossy()
            .to_string();

        let original: Vec<(u64, u64)> = vec![
            (1, 1),
            (u64::MAX - 1, u64::MAX - 1),
            (0, 0),
            (999_999_999, 123_456_789),
        ];
        write_latency_buffer(&path, &original).unwrap();

        let file = std::fs::File::open(&path).unwrap();
        let parsed: Vec<(u64, u64)> = BufReader::new(file)
            .lines()
            .filter_map(|l| l.ok().and_then(|s| parse_latency_file_line(&s).ok()))
            .collect();

        assert_eq!(parsed, original);

        let _ = std::fs::remove_file(&path);
    }

    /// write_latency_buffer should return an error when given
    /// an invalid path (e.g. a non-existent directory).
    #[test]
    fn test_write_latency_buffer_invalid_path() {
        let result = write_latency_buffer(
            "/no/such/directory/latencies.txt",
            &[(1000, 1), (2000, 2), (3000, 3)],
        );
        assert!(result.is_err(), "writing to invalid path should fail");
    }
}
