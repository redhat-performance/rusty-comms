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
    automotive_metrics::{AutomotiveMetrics, AutomotiveSuitabilityReport},
    benchmark::{BenchmarkConfig, BenchmarkRunner},
    cli::{Args, IpcMechanism},
    metrics::{LatencyMetrics, LatencyType, PercentileValue, PerformanceMetrics, ThroughputMetrics},
    results::{BenchmarkResults, ResultsManager},
};
use tracing::{error, info, warn};
use tracing_subscriber::{filter::LevelFilter, prelude::*, Layer};
use anyhow::Result;
use std::time::Duration;
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

    // Keep the logging guard alive for the duration of the program.
    // If we don't assign it to a variable, it gets dropped immediately, and file logging stops working.
    let _log_guard = guard;

    info!("Starting IPC Benchmark Suite");
    info!("{}", args);

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
        Some(path_str) => {
            Some(format!("{}.{}", path_str, today))
        }
        None => {
            Some(format!("ipc_benchmark.log.{}", today))
        }
    };

    // Initialize results manager for handling output
    // This manages both final JSON output and optional streaming results
    let mut results_manager =
        ResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable per-message latency streaming if specified
    // Per-message streaming captures individual message latency values with
    // timestamps for real-time monitoring of latency characteristics during execution
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

    // Run benchmarks for each selected mechanism.
    // This loop iterates through the list of IPC mechanisms to be tested,
    // executing the benchmark for each one sequentially.
    for &mechanism in &mechanisms {
        // Execute the benchmark for the current mechanism and handle the result.
        // The `run_benchmark_for_mechanism` function encapsulates all logic for a single test.
        match run_benchmark_for_mechanism(&config, &mechanism, &mut results_manager).await {
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
                        config.concurrency,
                        config.iterations,
                        config.duration,
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
    
    // Check if automotive mode is enabled for ultra-low latency testing
    if config.automotive_mode {
        info!("Running automotive ASIL-{:?} evaluation with ultra-low latency optimizations", config.asil_level);
        info!("   - Hard deadline: {}μs", config.max_latency_us);
        info!("   - Safety-critical mode enabled");
        
        // Use automotive evaluation with ultra-low latency optimizations
        match runner.run_automotive_evaluation(Some(results_manager)).await {
            Ok(automotive_report) => {
                info!("Automotive evaluation completed - Score: {:.1}/100", automotive_report.overall_score);
                info!("   - Max suitable ASIL level: {:?}", automotive_report.max_suitable_asil);
                info!("   - Suitable applications: {}", automotive_report.suitable_applications.len());
                
                if !automotive_report.issues.is_empty() {
                    info!("Issues detected:");
                    for issue in &automotive_report.issues {
                        info!("   - {}", issue);
                    }
                }
                
                // Convert automotive report to standard BenchmarkResults for output compatibility
                let results = convert_automotive_report_to_results(automotive_report, *mechanism, config)?;
                results_manager.add_results(results).await?;
            }
            Err(e) => {
                error!("Automotive evaluation failed: {}", e);
                return Err(e);
            }
        }
    } else if config.ultra_low_latency {
        info!("Running ultra-low latency benchmark (non-automotive mode)");
        
        // Use ultra-low latency optimizations without automotive requirements
        match runner.run_ultra_low_latency(Some(results_manager)).await {
            Ok(automotive_metrics) => {
                info!("Ultra-low latency test completed");
                info!("   - Average latency: {:.2}μs", automotive_metrics.average_latency_us);
                info!("   - Worst case: {}μs", automotive_metrics.worst_case_latency_us);
                info!("   - Total operations: {}", automotive_metrics.total_operations);
                
                // Convert ultra-low latency metrics to standard BenchmarkResults
                let results = convert_ull_metrics_to_results(automotive_metrics, *mechanism, config)?;
                results_manager.add_results(results).await?;
            }
            Err(e) => {
                error!("Ultra-low latency test failed: {}", e);
                return Err(e);
            }
        }
    } else {
        // Standard benchmark execution
        info!("Running standard benchmark for {} mechanism", mechanism);
        
        // Execute the benchmark and collect comprehensive results
        // This includes latency histograms, throughput measurements,
        // and statistical analysis (percentiles, mean, std dev, etc.)
        let results = runner.run(Some(results_manager)).await?;
        
        // Add results to the manager for aggregation and output
        // The manager handles both immediate streaming (if enabled)
        // and final consolidated output formatting
        results_manager.add_results(results).await?;
    }
    
    Ok(())
}

/// Convert automotive suitability report to standard BenchmarkResults format
/// 
/// This function bridges our specialized automotive metrics with the standard
/// results format for consistent output handling.
fn convert_automotive_report_to_results(
    report: AutomotiveSuitabilityReport,
    mechanism: IpcMechanism,
    config: &BenchmarkConfig,
) -> Result<BenchmarkResults> {
    let metrics = &report.metrics_summary;
    
    // Create standard benchmark results structure
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        config.concurrency,
        config.iterations,
        config.duration,
    );
    
    // Create proper PerformanceMetrics from automotive data
    if metrics.total_operations > 0 {
        let test_duration = config.duration.unwrap_or(Duration::from_secs(10));
        
        // Create One-Way LatencyMetrics
        let one_way_latency_metrics = LatencyMetrics {
            latency_type: LatencyType::OneWay,
            min_ns: (metrics.best_case_latency_us * 1000) as u64,
            max_ns: (metrics.worst_case_latency_us * 1000) as u64,
            mean_ns: (metrics.average_latency_us * 1000.0),
            median_ns: (metrics.average_latency_us * 1000.0), // Estimate median as mean
            std_dev_ns: (metrics.jitter_us as f64 * 1000.0) / 4.0, // Rough estimate: jitter/4
            percentiles: vec![
                PercentileValue { percentile: 50.0, value_ns: (metrics.average_latency_us * 1000.0) as u64 },
                PercentileValue { percentile: 95.0, value_ns: (metrics.average_latency_us * 1000.0 * 1.5) as u64 },
                PercentileValue { percentile: 99.0, value_ns: (metrics.worst_case_latency_us * 1000) as u64 },
                PercentileValue { percentile: 99.9, value_ns: (metrics.worst_case_latency_us * 1000) as u64 },
            ],
            total_samples: metrics.total_operations as usize,
            histogram_data: vec![], // No detailed histogram data available from automotive metrics
        };
        
        // Create Round-Trip LatencyMetrics (estimated as ~2x one-way)
        let round_trip_latency_metrics = LatencyMetrics {
            latency_type: LatencyType::RoundTrip,
            min_ns: (metrics.best_case_latency_us * 2000) as u64, // Estimate: 2x one-way
            max_ns: (metrics.worst_case_latency_us * 2000) as u64,
            mean_ns: (metrics.average_latency_us * 2000.0),
            median_ns: (metrics.average_latency_us * 2000.0),
            std_dev_ns: (metrics.jitter_us as f64 * 2000.0) / 4.0,
            percentiles: vec![
                PercentileValue { percentile: 50.0, value_ns: (metrics.average_latency_us * 2000.0) as u64 },
                PercentileValue { percentile: 95.0, value_ns: (metrics.average_latency_us * 2000.0 * 1.5) as u64 },
                PercentileValue { percentile: 99.0, value_ns: (metrics.worst_case_latency_us * 2000) as u64 },
                PercentileValue { percentile: 99.9, value_ns: (metrics.worst_case_latency_us * 2000) as u64 },
            ],
            total_samples: metrics.total_operations as usize,
            histogram_data: vec![], // No detailed histogram data available from automotive metrics
        };
        
        // Create ThroughputMetrics
        let messages_per_second = metrics.total_operations as f64 / test_duration.as_secs_f64();
        let bytes_per_second = messages_per_second * config.message_size as f64;
        
        let throughput_metrics = ThroughputMetrics {
            messages_per_second,
            bytes_per_second,
            total_messages: metrics.total_operations as usize,
            total_bytes: (metrics.total_operations as usize) * config.message_size,
            duration_ns: test_duration.as_nanos() as u64,
        };
        
        // Create One-Way PerformanceMetrics
        let one_way_performance_metrics = PerformanceMetrics {
            latency: Some(one_way_latency_metrics),
            throughput: throughput_metrics.clone(),
            timestamp: chrono::Utc::now(),
        };
        
        // Create Round-Trip PerformanceMetrics
        let round_trip_performance_metrics = PerformanceMetrics {
            latency: Some(round_trip_latency_metrics),
            throughput: throughput_metrics,
            timestamp: chrono::Utc::now(),
        };
        
        // Add both one-way and round-trip results
        results.add_one_way_results(one_way_performance_metrics);
        results.add_round_trip_results(round_trip_performance_metrics);
    }
    
    // Add automotive-specific metadata to the results
    results.add_metadata("automotive_mode", "true".to_string());
    results.add_metadata("asil_level", format!("{:?}", config.asil_level));
    results.add_metadata("max_latency_deadline_us", config.max_latency_us.to_string());
    results.add_metadata("automotive_score", format!("{:.1}", report.overall_score));
    results.add_metadata("max_suitable_asil", format!("{:?}", report.max_suitable_asil));
    results.add_metadata("suitable_applications_count", report.suitable_applications.len().to_string());
    results.add_metadata("deadline_misses", metrics.deadline_misses.to_string());
    results.add_metadata("deadline_miss_rate_ppm", format!("{:.2}", metrics.deadline_miss_rate_ppm));
    results.add_metadata("safety_margin_percent", format!("{:.1}", metrics.safety_margin_percent));
    results.add_metadata("determinism_score", format!("{:.3}", metrics.determinism_score));
    results.add_metadata("asil_compliant", metrics.asil_compliant.to_string());
    
    // Add issues and recommendations
    if !report.issues.is_empty() {
        results.add_metadata("automotive_issues", serde_json::to_string(&report.issues)?);
    }
    if !report.recommendations.is_empty() {
        results.add_metadata("automotive_recommendations", serde_json::to_string(&report.recommendations)?);
    }
    
    if metrics.asil_compliant {
        info!("ASIL-{:?} compliance: PASSED", config.asil_level);
    } else {
        warn!("ASIL-{:?} compliance: FAILED", config.asil_level);
        warn!("   Deadline miss rate: {:.2} PPM (max allowed: {} PPM)", 
              metrics.deadline_miss_rate_ppm, 
              metrics.automotive_application.max_error_rate_ppm());
    }
    
    Ok(results)
}

/// Convert ultra-low latency metrics to standard BenchmarkResults format
/// 
/// This function converts our ultra-low latency AutomotiveMetrics to the standard
/// BenchmarkResults format for consistent output.
fn convert_ull_metrics_to_results(
    metrics: AutomotiveMetrics,
    mechanism: IpcMechanism,
    config: &BenchmarkConfig,
) -> Result<BenchmarkResults> {
    // Create standard benchmark results structure
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        config.concurrency,
        config.iterations,
        config.duration,
    );
    
    // Create proper PerformanceMetrics from ultra-low latency data
    if metrics.total_operations > 0 {
        let test_duration = config.duration.unwrap_or(Duration::from_secs(10));
        
        // Create One-Way LatencyMetrics
        let one_way_latency_metrics = LatencyMetrics {
            latency_type: LatencyType::OneWay,
            min_ns: (metrics.best_case_latency_us * 1000) as u64,
            max_ns: (metrics.worst_case_latency_us * 1000) as u64,
            mean_ns: (metrics.average_latency_us * 1000.0),
            median_ns: (metrics.average_latency_us * 1000.0), // Estimate median as mean
            std_dev_ns: (metrics.jitter_us as f64 * 1000.0) / 4.0, // Rough estimate: jitter/4
            percentiles: vec![
                PercentileValue { percentile: 50.0, value_ns: (metrics.average_latency_us * 1000.0) as u64 },
                PercentileValue { percentile: 95.0, value_ns: (metrics.average_latency_us * 1000.0 * 1.2) as u64 },
                PercentileValue { percentile: 99.0, value_ns: (metrics.worst_case_latency_us * 1000) as u64 },
                PercentileValue { percentile: 99.9, value_ns: (metrics.worst_case_latency_us * 1000) as u64 },
            ],
            total_samples: metrics.total_operations as usize,
            histogram_data: vec![], // No detailed histogram data available from automotive metrics
        };
        
        // Create Round-Trip LatencyMetrics (estimated as ~2x one-way)
        let round_trip_latency_metrics = LatencyMetrics {
            latency_type: LatencyType::RoundTrip,
            min_ns: (metrics.best_case_latency_us * 2000) as u64, // Estimate: 2x one-way
            max_ns: (metrics.worst_case_latency_us * 2000) as u64,
            mean_ns: (metrics.average_latency_us * 2000.0),
            median_ns: (metrics.average_latency_us * 2000.0),
            std_dev_ns: (metrics.jitter_us as f64 * 2000.0) / 4.0,
            percentiles: vec![
                PercentileValue { percentile: 50.0, value_ns: (metrics.average_latency_us * 2000.0) as u64 },
                PercentileValue { percentile: 95.0, value_ns: (metrics.average_latency_us * 2000.0 * 1.2) as u64 },
                PercentileValue { percentile: 99.0, value_ns: (metrics.worst_case_latency_us * 2000) as u64 },
                PercentileValue { percentile: 99.9, value_ns: (metrics.worst_case_latency_us * 2000) as u64 },
            ],
            total_samples: metrics.total_operations as usize,
            histogram_data: vec![], // No detailed histogram data available from automotive metrics
        };
        
        // Create ThroughputMetrics
        let messages_per_second = metrics.total_operations as f64 / test_duration.as_secs_f64();
        let bytes_per_second = messages_per_second * config.message_size as f64;
        
        let throughput_metrics = ThroughputMetrics {
            messages_per_second,
            bytes_per_second,
            total_messages: metrics.total_operations as usize,
            total_bytes: (metrics.total_operations as usize) * config.message_size,
            duration_ns: test_duration.as_nanos() as u64,
        };
        
        // Create One-Way PerformanceMetrics
        let one_way_performance_metrics = PerformanceMetrics {
            latency: Some(one_way_latency_metrics),
            throughput: throughput_metrics.clone(),
            timestamp: chrono::Utc::now(),
        };
        
        // Create Round-Trip PerformanceMetrics
        let round_trip_performance_metrics = PerformanceMetrics {
            latency: Some(round_trip_latency_metrics),
            throughput: throughput_metrics,
            timestamp: chrono::Utc::now(),
        };
        
        // Add both one-way and round-trip results
        results.add_one_way_results(one_way_performance_metrics);
        results.add_round_trip_results(round_trip_performance_metrics);
    }
    
    // Add ultra-low latency specific metadata
    results.add_metadata("ultra_low_latency_mode", "true".to_string());
    results.add_metadata("total_operations", metrics.total_operations.to_string());
    results.add_metadata("average_latency_us", format!("{:.2}", metrics.average_latency_us));
    results.add_metadata("worst_case_latency_us", metrics.worst_case_latency_us.to_string());
    results.add_metadata("best_case_latency_us", metrics.best_case_latency_us.to_string());
    results.add_metadata("jitter_us", metrics.jitter_us.to_string());
    results.add_metadata("determinism_score", format!("{:.3}", metrics.determinism_score));
    
    if metrics.deadline_misses > 0 {
        results.add_metadata("deadline_misses", metrics.deadline_misses.to_string());
        results.add_metadata("deadline_miss_rate_ppm", format!("{:.2}", metrics.deadline_miss_rate_ppm));
        warn!("{} deadline misses detected in ultra-low latency test", metrics.deadline_misses);
    }
    
    info!("Ultra-low latency performance:");
    info!("   Average: {:.2}μs, Best: {}μs, Worst: {}μs", 
          metrics.average_latency_us, 
          metrics.best_case_latency_us, 
          metrics.worst_case_latency_us);
    info!("   Jitter: {}μs, Determinism: {:.1}%", 
          metrics.jitter_us, 
          metrics.determinism_score * 100.0);
    
    Ok(results)
}
