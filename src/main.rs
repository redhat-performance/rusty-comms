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

use clap::Parser;
use ipc_benchmark::{
    benchmark::{BenchmarkConfig, BenchmarkRunner},
    cli::{Args, IpcMechanism},
    results::ResultsManager,
};
use tracing::{error, info};
use tracing_subscriber::{filter::LevelFilter, prelude::*, Layer};
use anyhow::Result;
use tracing_appender;

mod logging;
use logging::ColorizedFormatter;

/// Main application entry point
/// 
/// This async function coordinates the entire benchmark execution lifecycle.
/// It uses Tokio's multi-threaded runtime to handle async I/O operations
/// required by the various IPC mechanisms being benchmarked.
#[tokio::main]
async fn main() -> Result<()> {
    // Parse command-line arguments first, as they control logging behavior.
    let args = Args::parse();

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
                let log_dir = log_path.parent().unwrap_or_else(|| std::path::Path::new("."));
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
    let stdout_log = if !args.quiet {
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

    // Keep the guard alive for the duration of the program.
    // If we don't assign it, it gets dropped immediately, and file logging stops working.
    let _guard = guard;

    info!("Starting IPC Benchmark Suite");
    info!("{}", args);

    // Create benchmark configuration from parsed CLI arguments
    // This converts the user-friendly CLI format into the internal
    // configuration structure used by the benchmark engine
    let config = BenchmarkConfig::from_args(&args)?;

    // Initialize results manager for handling output
    // This manages both final JSON output and optional streaming results
    let mut results_manager = ResultsManager::new(&args.output_file)?;

    // Enable per-message latency streaming if specified
    // Per-message streaming captures individual message latency values with timestamps
    // for real-time monitoring of latency characteristics during execution
    if let Some(ref streaming_file) = args.streaming_output_json {
        info!("Enabling per-message latency streaming to: {:?}", streaming_file);
        
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

    // Run benchmarks for each specified IPC mechanism
    // Each mechanism is tested independently with proper resource cleanup
    for (i, mechanism) in mechanisms.iter().enumerate() {
        info!("Running benchmark for mechanism: {:?}", mechanism);

        // Add delay between mechanisms to allow proper resource cleanup
        // This prevents resource conflicts and ensures clean test environments
        // The delay is particularly important for:
        // - Socket file cleanup (Unix Domain Sockets)
        // - Shared memory segment cleanup
        // - Port binding conflicts (TCP)
        // - Message queue cleanup (POSIX Message Queues)
        if i > 0 {
            info!("Waiting for resource cleanup...");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        // Execute benchmark for this mechanism with comprehensive error handling
        match run_benchmark_for_mechanism(&config, mechanism, &mut results_manager).await {
            Ok(_) => info!("Benchmark completed successfully for {:?}", mechanism),
            Err(e) => {
                error!("Benchmark failed for {:?}: {}", mechanism, e);
                // Depending on configuration, either fail fast or continue
                // This allows users to get partial results even if one mechanism fails
                if !args.continue_on_error {
                    return Err(e);
                }
            }
        }
    }

    // Finalize results and output
    // This performs final aggregation, calculates summary statistics,
    // and writes the comprehensive results to the output file
    results_manager.finalize().await?;

    info!("IPC Benchmark Suite completed successfully");
    Ok(())
}

/// Run benchmark for a specific IPC mechanism
///
/// This function encapsulates the benchmark execution for a single IPC mechanism.
/// It creates a `BenchmarkRunner` configured for the specific mechanism and
/// executes both one-way and round-trip latency tests as configured.
///
/// ## Parameters
/// - `config`: Benchmark configuration (message size, iterations, etc.)
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
) -> Result<()> {
    // Create a benchmark runner for this specific mechanism
    // The runner encapsulates all the logic for setting up clients/servers,
    // running warmup iterations, executing tests, and collecting metrics
    let runner = BenchmarkRunner::new(config.clone(), *mechanism);
    
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
