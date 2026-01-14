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
    cli::{Args, IpcMechanism, RunMode},
    container::ContainerManager,
    host_container::HostBenchmarkRunner,
    ipc::{get_monotonic_time_ns, Message, MessageType, TransportFactory},
    results::{BenchmarkResults, ResultsManager},
    results_blocking::BlockingResultsManager,
};
use std::io::{self, Write};
use tracing::{debug, error, info, warn};

use tracing_subscriber::{filter::LevelFilter, prelude::*, Layer};

mod logging;
use ipc_benchmark::cli;
use logging::ColorizedFormatter;

/// Main entry point for the IPC benchmark suite.
///
/// This function determines the execution mode based on CLI flags and dispatches
/// to the appropriate execution path.
///
/// # Execution Modes
///
/// - **Async (default)**: Uses Tokio runtime with async/await for
///   non-blocking I/O
/// - **Blocking**: Uses std library with traditional blocking I/O operations
///
/// # Run Modes
///
/// - **Standalone (default)**: Spawns client as subprocess on same host
/// - **Host**: Creates Podman container as client, drives tests from host
/// - **Client**: Runs inside container, connects back to host server
///
/// The mode selection happens at runtime based on CLI arguments, allowing the
/// same binary to run in any mode without recompilation.
///
/// # Examples
///
/// ```bash
/// # Run in async mode (default)
/// ipc-benchmark -m uds -s 1024 -i 10000
///
/// # Run in blocking mode
/// ipc-benchmark -m uds -s 1024 -i 10000 --blocking
///
/// # Run in host mode (creates container)
/// ipc-benchmark -m uds --run-mode host -i 10000
///
/// # Stop containers
/// ipc-benchmark --stop-container all
/// ```
fn main() -> Result<()> {
    // Parse CLI arguments to determine execution mode
    let mut args = Args::parse();

    // Handle container stop command first (exits after)
    if let Some(ref mechanism) = args.stop_container {
        return stop_container_command(&args, mechanism);
    }

    // Handle container list command (exits after)
    if args.list_containers {
        return list_containers_command(&args);
    }

    // Auto-enable blocking mode when --shm-direct is used
    // Direct memory shared memory is only available in blocking mode
    if args.shm_direct && !args.blocking {
        eprintln!(
            "Note: --shm-direct automatically enables --blocking mode \
             (direct memory SHM requires blocking I/O)"
        );
        args.blocking = true;
    }

    // Branch based on run mode and execution mode (async/blocking)
    match args.run_mode {
        RunMode::Standalone => {
            // Existing behavior - spawn subprocess client
            if args.blocking {
                run_blocking_mode(args)
            } else {
                run_async_mode(args)
            }
        }
        RunMode::Host => {
            // Host mode - create container, drive tests
            if args.blocking {
                run_host_mode_blocking(args)
            } else {
                run_host_mode_async(args)
            }
        }
        RunMode::Client => {
            // Client mode - connect to host (run inside container)
            if args.blocking {
                run_client_mode_blocking(args)
            } else {
                run_client_mode_async(args)
            }
        }
    }
}

/// Stop container command handler.
///
/// Stops the specified benchmark container(s) and exits.
///
/// # Arguments
///
/// * `args` - CLI arguments containing container configuration
/// * `mechanism` - Mechanism name ("uds", "shm", "pmq") or "all"
///
/// # Returns
///
/// * `Ok(())` - Containers stopped successfully
/// * `Err` - Failed to stop containers
fn stop_container_command(args: &Args, mechanism: &str) -> Result<()> {
    let container_manager = ContainerManager::new(&args.container_prefix, &args.container_image);

    if mechanism.to_lowercase() == "all" {
        info!("Stopping all benchmark containers");
        container_manager.stop_all()?;
        container_manager.remove_all()?;
        info!("All benchmark containers stopped and removed");
    } else {
        // Parse mechanism name to IpcMechanism
        let mech = match mechanism.to_lowercase().as_str() {
            "uds" => IpcMechanism::UnixDomainSocket,
            "shm" => IpcMechanism::SharedMemory,
            "tcp" => IpcMechanism::TcpSocket,
            #[cfg(target_os = "linux")]
            "pmq" => IpcMechanism::PosixMessageQueue,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown mechanism: '{}'. Use uds, shm, tcp, pmq, or all.",
                    mechanism
                ));
            }
        };

        let container_name = container_manager.container_name(&mech);
        info!("Stopping container: {}", container_name);
        container_manager.stop(&container_name)?;
        container_manager.remove(&container_name)?;
        info!("Container '{}' stopped and removed", container_name);
    }

    Ok(())
}

/// List all benchmark containers managed by this tool.
///
/// Shows container names and their status (running/stopped/not found).
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments (for container prefix)
///
/// # Returns
///
/// * `Ok(())` - List completed successfully
/// * `Err` - Failed to query containers
fn list_containers_command(args: &Args) -> Result<()> {
    let container_manager = ContainerManager::new(&args.container_prefix, &args.container_image);

    println!("Benchmark containers (prefix: {}):", args.container_prefix);
    println!("{:-<50}", "");

    let mechanisms = vec![
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket,
        IpcMechanism::SharedMemory,
        IpcMechanism::TcpSocket,
        #[cfg(target_os = "linux")]
        IpcMechanism::PosixMessageQueue,
    ];

    let mut found_any = false;
    for mechanism in &mechanisms {
        let container_name = container_manager.container_name(mechanism);

        let exists = container_manager.exists(&container_name)?;
        if exists {
            found_any = true;
            let running = container_manager.is_running(&container_name)?;
            let status = if running { "running" } else { "stopped" };
            println!("  {} : {}", container_name, status);
        }
    }

    if !found_any {
        println!("  (no containers found)");
    }

    println!();
    println!("Use --stop-container <mechanism> to stop a container.");
    println!("Use --stop-container all to stop all containers.");

    Ok(())
}

/// Run benchmark in host mode with async I/O.
///
/// Creates Podman container client, drives tests, collects results.
/// The host acts as server and spawns a container running the benchmark
/// in client mode.
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments
///
/// # Returns
///
/// * `Ok(())` - Benchmark completed successfully
/// * `Err` - Benchmark or container management failed
#[tokio::main]
async fn run_host_mode_async(args: Args) -> Result<()> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    // Configure logging level based on verbosity flags
    let log_level = match args.verbose {
        0 => tracing_subscriber::filter::LevelFilter::INFO,
        1 => tracing_subscriber::filter::LevelFilter::DEBUG,
        _ => tracing_subscriber::filter::LevelFilter::TRACE,
    };

    // Configure the detailed log layer (file or stderr)
    let guard;
    let detailed_log_layer;

    if let Some("stderr") = args.log_file.as_deref() {
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(log_level)
            .boxed();
        guard = None;
    } else {
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

    // Stdout layer for user-facing output
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

    let _log_guard = guard;

    info!("Starting IPC Benchmark Suite (Host-Container Mode, Async)");

    // Create benchmark configuration from parsed CLI arguments
    let config = BenchmarkConfig::from_args(&args)?;

    // Calculate today's date for log file naming
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => Some(format!("{}.{}", path_str, today)),
        None => Some(format!("ipc_benchmark.log.{}", today)),
    };

    // Initialize async results manager
    let mut results_manager =
        ResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable streaming if specified
    let both_tests_enabled = config.one_way && config.round_trip;

    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );
        if both_tests_enabled {
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }

    if let Some(ref streaming_file) = args.streaming_output_csv {
        info!("Enabling CSV latency streaming to: {:?}", streaming_file);
        results_manager.enable_csv_streaming(streaming_file)?;
        // Set combined mode for CSV if both tests are enabled
        if both_tests_enabled {
            results_manager.set_combined_mode(true);
        }
    }

    // Get expanded mechanisms (handles 'all' expansion)
    let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());

    // Run benchmarks for each selected mechanism
    for &mechanism in &mechanisms {
        info!("Running async host-container benchmark for {}", mechanism);

        let runner = HostBenchmarkRunner::new(config.clone(), mechanism, args.clone());

        match runner.run(Some(&mut results_manager)).await {
            Ok(results) => {
                info!(
                    "Successfully completed async host-container benchmark for {} mechanism",
                    mechanism
                );
                results_manager.add_results(results).await?;
            }
            Err(e) => {
                let error_msg = e.to_string();
                error!(
                    "Async host-container benchmark for {} failed: {}. {}",
                    mechanism,
                    error_msg,
                    if args.continue_on_error {
                        "Continuing to next mechanism."
                    } else {
                        "Aborting."
                    }
                );

                if !args.continue_on_error {
                    return Err(e);
                }
            }
        }
    }

    // Finalize and write results
    results_manager.finalize().await?;
    results_manager.print_summary()?;

    info!("IPC Benchmark Suite (Host-Container Mode, Async) completed successfully");

    Ok(())
}

/// Run benchmark in host mode with blocking I/O.
///
/// Creates Podman container client, drives tests, collects results.
/// The host acts as server and spawns a container running the benchmark
/// in client mode.
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments
///
/// # Returns
///
/// * `Ok(())` - Benchmark completed successfully
/// * `Err` - Benchmark or container management failed
fn run_host_mode_blocking(args: Args) -> Result<()> {
    // Configure logging level based on verbosity flags
    let log_level = match args.verbose {
        0 => tracing_subscriber::filter::LevelFilter::INFO,
        1 => tracing_subscriber::filter::LevelFilter::DEBUG,
        _ => tracing_subscriber::filter::LevelFilter::TRACE,
    };

    // Configure the detailed log layer (file or stderr)
    let guard;
    let detailed_log_layer;

    if let Some("stderr") = args.log_file.as_deref() {
        detailed_log_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(log_level)
            .boxed();
        guard = None;
    } else {
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

    // Stdout layer for user-facing output
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

    let _log_guard = guard;

    info!("Starting IPC Benchmark Suite (Host-Container Mode, Blocking)");

    // Create benchmark configuration from parsed CLI arguments
    let config = BenchmarkConfig::from_args(&args)?;

    // Calculate today's date for log file naming
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => Some(format!("{}.{}", path_str, today)),
        None => Some(format!("ipc_benchmark.log.{}", today)),
    };

    // Initialize blocking results manager
    let mut results_manager =
        BlockingResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable streaming if specified
    let both_tests_enabled = config.one_way && config.round_trip;

    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );
        if both_tests_enabled {
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }

    if let Some(ref streaming_file) = args.streaming_output_csv {
        info!("Enabling CSV latency streaming to: {:?}", streaming_file);
        results_manager.enable_csv_streaming(streaming_file)?;
        // Set combined mode for CSV if both tests are enabled
        if both_tests_enabled {
            results_manager.set_combined_mode(true);
        }
    }

    // Get expanded mechanisms (handles 'all' expansion)
    let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());

    // Run benchmarks for each selected mechanism
    for &mechanism in &mechanisms {
        info!("Running host-container benchmark for {}", mechanism);

        let runner = HostBenchmarkRunner::new(config.clone(), mechanism, args.clone());

        match runner.run_blocking(Some(&mut results_manager)) {
            Ok(results) => {
                info!(
                    "Successfully completed host-container benchmark for {} mechanism",
                    mechanism
                );
                results_manager.add_results(results)?;
            }
            Err(e) => {
                let error_msg = e.to_string();
                error!(
                    "Host-container benchmark for {} failed: {}. {}",
                    mechanism,
                    error_msg,
                    if args.continue_on_error {
                        "Continuing to next mechanism."
                    } else {
                        "Aborting."
                    }
                );

                if args.continue_on_error {
                    let mut failed_result = BenchmarkResults::new(
                        mechanism,
                        config.message_size,
                        0,
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
                    return Err(e);
                }
            }
        }
    }

    // Finalize results
    results_manager.finalize()?;

    if let Err(e) = results_manager.print_summary() {
        error!("Failed to print results summary: {}", e);
    }

    info!("IPC Benchmark Suite (Host-Container Mode) completed successfully");

    Ok(())
}

/// Run benchmark in client mode with async I/O.
///
/// Runs inside container, connects back to host server.
/// This mode is automatically invoked by the host mode when it creates
/// the container.
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments (includes connection info)
///
/// # Returns
///
/// * `Ok(())` - Client completed successfully
/// * `Err` - Connection or benchmark failed
#[tokio::main]
async fn run_client_mode_async(_args: Args) -> Result<()> {
    // Client mode async is not yet implemented.
    Err(anyhow::anyhow!(
        "Client mode async is not yet implemented.\n\
         Use --blocking flag for client mode:\n\
         ipc-benchmark -m uds --run-mode client --blocking"
    ))
}

/// Run benchmark in client mode with blocking I/O.
///
/// Runs inside container, acts as IPC server (receiver/responder).
/// This mode is automatically invoked by the host when it creates
/// the container.
///
/// The client mode is essentially the same as `--internal-run-as-server`
/// but designed for container execution. It:
/// 1. Sets up IPC transport as server
/// 2. Signals readiness to stdout
/// 3. Receives messages and responds (for round-trip tests)
/// 4. Exits on shutdown message or client disconnect
///
/// # Arguments
///
/// * `args` - Parsed command-line arguments (includes connection info)
///
/// # Returns
///
/// * `Ok(())` - Client completed successfully
/// * `Err` - Connection or benchmark failed
fn run_client_mode_blocking(args: Args) -> Result<()> {
    use ipc_benchmark::ipc::BlockingTransportFactory;

    // Minimal logging setup for container mode - log to stderr only
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("Starting IPC Benchmark (Client/Container Mode, Blocking)");

    // In client mode, we only care about the first mechanism specified
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => {
            return Err(anyhow::anyhow!(
                "No IPC mechanism specified for client mode"
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

    // Build transport config from CLI args
    // We use mutable defaults + reassignment because we conditionally set
    // different fields based on the mechanism type below.
    #[allow(clippy::field_reassign_with_default)]
    let mut transport_config = {
        let mut tc = ipc_benchmark::ipc::TransportConfig::default();
        tc.buffer_size = config
            .buffer_size
            .unwrap_or_else(|| std::cmp::max(config.message_size * 2, 4096));
        tc
    };

    // Set mechanism-specific configuration from CLI args
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
        #[allow(unreachable_patterns)]
        _ => {}
    }

    // Create and start transport as server
    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;
    transport
        .start_server_blocking(&transport_config)
        .context("Failed to start transport in client/server mode")?;

    // Signal readiness to host via stdout
    // The host waits for this marker before connecting
    println!("SERVER_READY");
    io::stdout().flush().ok();

    info!(
        "Client mode server ready for {} on {:?}",
        mechanism, transport_config
    );

    // Server loop: receive messages and optionally respond
    loop {
        match transport.receive_blocking() {
            Ok(message) => {
                // Calculate actual IPC latency for one-way measurements
                let receive_time_ns = get_monotonic_time_ns();
                let _latency_ns = receive_time_ns.saturating_sub(message.timestamp);

                // Check for shutdown message
                if message.message_type == MessageType::Shutdown {
                    debug!("Received shutdown message, exiting cleanly");
                    break;
                }

                // Handle different message types
                match message.message_type {
                    MessageType::Request => {
                        // Echo response for round-trip tests
                        let response = Message::new(message.id, Vec::new(), MessageType::Response);
                        if let Err(e) = transport.send_blocking(&response) {
                            warn!("Failed to send response: {}. Exiting.", e);
                            break;
                        }
                    }
                    MessageType::Ping => {
                        let pong = Message::new(message.id, Vec::new(), MessageType::Pong);
                        if let Err(e) = transport.send_blocking(&pong) {
                            warn!("Failed to send pong: {}. Exiting.", e);
                            break;
                        }
                    }
                    // OneWay messages need no response
                    _ => {}
                }
            }
            Err(e) => {
                debug!("Server receive error (client disconnected?): {}", e);
                break;
            }
        }
    }

    transport.close_blocking()?;
    info!("Client mode server exiting cleanly.");
    Ok(())
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
    // Check if both test types are enabled for combined streaming
    let both_tests_enabled = config.one_way && config.round_trip;

    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );

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
        // Set combined mode for CSV if both tests are enabled
        if both_tests_enabled {
            results_manager.set_combined_mode(true);
        }
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
                        0, // Buffer size is unknown/irrelevant in a failure case.
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
    // Check if both test types are enabled for combined streaming
    let both_tests_enabled = config.one_way && config.round_trip;

    if let Some(ref streaming_file) = args.streaming_output_json {
        info!(
            "Enabling per-message latency streaming to: {:?}",
            streaming_file
        );

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
        // Set combined mode for CSV if both tests are enabled
        if both_tests_enabled {
            results_manager.set_combined_mode(true);
        }
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
                        0, // Buffer size is unknown/irrelevant in failure
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
    use ipc_benchmark::ipc::BlockingTransportFactory;

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

    // Open latency file for writing if specified
    let mut latency_file = if let Some(ref path) = args.internal_latency_file {
        Some(
            std::fs::File::create(path)
                .with_context(|| format!("Failed to create latency file: {}", path))?,
        )
    } else {
        None
    };

    // Persistent server loop: receive messages and optionally reply
    loop {
        match transport.receive_blocking() {
            Ok(message) => {
                // Calculate actual IPC latency: receive_time - send_time
                // Use monotonic clock to avoid NTP adjustments affecting measurements
                let receive_time_ns = get_monotonic_time_ns();
                let latency_ns = receive_time_ns.saturating_sub(message.timestamp);

                // Write latency to file if enabled (one latency per line in nanoseconds)
                // Skip canary messages (ID == u64::MAX) which are used for warmup
                if let Some(ref mut file) = latency_file {
                    if message.id != u64::MAX {
                        writeln!(file, "{}", latency_ns).ok();
                    }
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

    transport.close_blocking()?;
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

    // Open latency file for writing if specified
    let mut latency_file = if let Some(ref path) = args.internal_latency_file {
        Some(
            tokio::fs::File::create(path)
                .await
                .with_context(|| format!("Failed to create latency file: {}", path))?,
        )
    } else {
        None
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
                let latency_ns = receive_time_ns.saturating_sub(msg.timestamp);

                // Write latency to file if enabled (one latency per line in nanoseconds)
                // Skip canary messages (ID == u64::MAX) which are used for warmup
                if let Some(ref mut file) = latency_file {
                    if msg.id != u64::MAX {
                        use tokio::io::AsyncWriteExt;
                        let _ = file.write_all(format!("{}\n", latency_ns).as_bytes()).await;
                    }
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

    let _ = transport.close().await;
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
