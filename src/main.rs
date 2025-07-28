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
// Removed redundant import
use anyhow::Result;

/// Main application entry point
/// 
/// This async function coordinates the entire benchmark execution lifecycle.
/// It uses Tokio's multi-threaded runtime to handle async I/O operations
/// required by the various IPC mechanisms being benchmarked.
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging with tracing
    // The log level can be controlled via RUST_LOG environment variable
    // Example: RUST_LOG=debug cargo run -- --mechanisms all
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Parse command-line arguments using clap derive API
    // This includes validation of argument types and ranges
    let args = Args::parse();

    info!("Starting IPC Benchmark Suite");
    info!("Configuration: {:?}", args);

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
    if let Some(ref streaming_file) = args.streaming_output {
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
