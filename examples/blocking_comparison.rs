//! Async vs. Blocking Mode Comparison Example
//!
//! This example demonstrates how to run the same benchmark in both async and
//! blocking modes for direct performance comparison.
//!
//! # Purpose
//!
//! Compare the performance characteristics of:
//! - **Async Mode**: Uses Tokio runtime with async/await (default)
//! - **Blocking Mode**: Uses pure std library with blocking I/O
//!
//! This helps answer questions like:
//! - How much overhead does the async runtime add?
//! - Which mode has lower latency for my use case?
//! - Which mode has better throughput?
//! - How do latency distributions differ?
//!
//! # Usage
//!
//! ```bash
//! cargo run --example blocking_comparison
//! ```
//!
//! # What This Example Does
//!
//! 1. Runs the same TCP benchmark in async mode
//! 2. Runs the identical benchmark in blocking mode
//! 3. Compares the results side-by-side
//! 4. Highlights key differences in latency and throughput
//!
//! # Expected Results
//!
//! You may observe:
//! - **Async Mode**: Slightly higher mean latency due to runtime overhead
//! - **Blocking Mode**: More predictable latency (lower variance)
//! - **Throughput**: Usually similar for single connections
//! - **P99/P99.9**: Blocking often shows tighter tail latencies
//!
//! # Performance Notes
//!
//! - Use CPU affinity flags for more stable comparisons
//! - Run on an idle system to reduce noise
//! - Use larger message counts for statistical significance
//! - Warmup iterations help stabilize results

use anyhow::Result;
use ipc_benchmark::{
    benchmark::BenchmarkRunner, benchmark_blocking::BlockingBenchmarkRunner, cli::Args,
    BenchmarkConfig, IpcMechanism,
};
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for console output
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN) // Reduce noise for comparison
        .with_target(false)
        .init();

    println!("=== Async vs. Blocking Mode Comparison ===\n");
    println!("This example runs the same benchmark in both modes to compare");
    println!("performance characteristics.\n");

    // Shared test parameters
    let message_size = 1024;
    let msg_count = 25000;
    let warmup_iterations = 2000;
    let percentiles = vec![50.0, 95.0, 99.0, 99.9];

    println!("Test Configuration:");
    println!("  Mechanism:      TCP Socket");
    println!("  Message Size:   {} bytes", message_size);
    println!("  Message Count:  {}", msg_count);
    println!("  Warmup:         {} iterations", warmup_iterations);
    println!("  Tests:          One-Way + Round-Trip\n");

    // ========================
    // Run Async Mode Benchmark
    // ========================
    println!("Running ASYNC mode benchmark...");
    let async_start = Instant::now();

    let async_args = create_args(
        message_size,
        msg_count,
        warmup_iterations,
        percentiles.clone(),
        false, // async mode
    );

    let async_config = BenchmarkConfig::from_args(&async_args)?;
    let async_runner = BenchmarkRunner::new(async_config, IpcMechanism::TcpSocket, async_args);

    let async_results = async_runner.run(None).await?;
    let async_duration = async_start.elapsed();
    println!("  Completed in {:.2}s\n", async_duration.as_secs_f64());

    // ===========================
    // Run Blocking Mode Benchmark
    // ===========================
    println!("Running BLOCKING mode benchmark...");
    let blocking_start = Instant::now();

    let blocking_args = create_args(
        message_size,
        msg_count,
        warmup_iterations,
        percentiles.clone(),
        true, // blocking mode
    );

    let blocking_config = BenchmarkConfig::from_args(&blocking_args)?;
    let blocking_runner =
        BlockingBenchmarkRunner::new(blocking_config, IpcMechanism::TcpSocket, blocking_args);

    let blocking_results = blocking_runner.run(None)?;
    let blocking_duration = blocking_start.elapsed();
    println!("  Completed in {:.2}s\n", blocking_duration.as_secs_f64());

    // ========================
    // Compare Results
    // ========================
    println!("=== PERFORMANCE COMPARISON ===\n");

    // One-Way Comparison
    if let (Some(async_ow), Some(blocking_ow)) = (
        &async_results.one_way_results,
        &blocking_results.one_way_results,
    ) {
        if let (Some(async_lat), Some(blocking_lat)) = (&async_ow.latency, &blocking_ow.latency) {
            println!("One-Way Latency:");
            println!("┌─────────────┬──────────────┬──────────────┬──────────────┐");
            println!("│   Metric    │  Async Mode  │ Blocking Mode│   Difference │");
            println!("├─────────────┼──────────────┼──────────────┼──────────────┤");

            print_comparison_row("Mean", async_lat.mean_ns, blocking_lat.mean_ns);
            print_comparison_row("Median", async_lat.median_ns, blocking_lat.median_ns);
            print_comparison_row("Min", async_lat.min_ns as f64, blocking_lat.min_ns as f64);
            print_comparison_row("Max", async_lat.max_ns as f64, blocking_lat.max_ns as f64);

            for i in 0..async_lat.percentiles.len() {
                let async_p = &async_lat.percentiles[i];
                let blocking_p = &blocking_lat.percentiles[i];
                print_comparison_row(
                    &format!("P{:.1}", async_p.percentile),
                    async_p.value_ns as f64,
                    blocking_p.value_ns as f64,
                );
            }

            println!("└─────────────┴──────────────┴──────────────┴──────────────┘\n");
        }

        println!("One-Way Throughput:");
        println!(
            "  Async:    {:.2} msg/s ({:.2} MB/s)",
            async_ow.throughput.messages_per_second,
            async_ow.throughput.bytes_per_second / 1_000_000.0
        );
        println!(
            "  Blocking: {:.2} msg/s ({:.2} MB/s)",
            blocking_ow.throughput.messages_per_second,
            blocking_ow.throughput.bytes_per_second / 1_000_000.0
        );

        let throughput_diff = ((blocking_ow.throughput.messages_per_second
            - async_ow.throughput.messages_per_second)
            / async_ow.throughput.messages_per_second)
            * 100.0;
        println!("  Difference: {:.1}%\n", throughput_diff);
    }

    // Round-Trip Comparison
    if let (Some(async_rt), Some(blocking_rt)) = (
        &async_results.round_trip_results,
        &blocking_results.round_trip_results,
    ) {
        if let (Some(async_lat), Some(blocking_lat)) = (&async_rt.latency, &blocking_rt.latency) {
            println!("Round-Trip Latency:");
            println!("┌─────────────┬──────────────┬──────────────┬──────────────┐");
            println!("│   Metric    │  Async Mode  │ Blocking Mode│   Difference │");
            println!("├─────────────┼──────────────┼──────────────┼──────────────┤");

            print_comparison_row("Mean", async_lat.mean_ns, blocking_lat.mean_ns);
            print_comparison_row("Median", async_lat.median_ns, blocking_lat.median_ns);
            print_comparison_row("Min", async_lat.min_ns as f64, blocking_lat.min_ns as f64);
            print_comparison_row("Max", async_lat.max_ns as f64, blocking_lat.max_ns as f64);

            for i in 0..async_lat.percentiles.len() {
                let async_p = &async_lat.percentiles[i];
                let blocking_p = &blocking_lat.percentiles[i];
                print_comparison_row(
                    &format!("P{:.1}", async_p.percentile),
                    async_p.value_ns as f64,
                    blocking_p.value_ns as f64,
                );
            }

            println!("└─────────────┴──────────────┴──────────────┴──────────────┘\n");
        }

        println!("Round-Trip Throughput:");
        println!(
            "  Async:    {:.2} msg/s ({:.2} MB/s)",
            async_rt.throughput.messages_per_second,
            async_rt.throughput.bytes_per_second / 1_000_000.0
        );
        println!(
            "  Blocking: {:.2} msg/s ({:.2} MB/s)",
            blocking_rt.throughput.messages_per_second,
            blocking_rt.throughput.bytes_per_second / 1_000_000.0
        );

        let throughput_diff = ((blocking_rt.throughput.messages_per_second
            - async_rt.throughput.messages_per_second)
            / async_rt.throughput.messages_per_second)
            * 100.0;
        println!("  Difference: {:.1}%\n", throughput_diff);
    }

    // Summary
    println!("=== SUMMARY ===\n");
    println!("Key Observations:");

    if let (Some(async_ow), Some(blocking_ow)) = (
        &async_results.one_way_results,
        &blocking_results.one_way_results,
    ) {
        if let (Some(async_lat), Some(blocking_lat)) = (&async_ow.latency, &blocking_ow.latency) {
            let latency_diff =
                ((blocking_lat.mean_ns - async_lat.mean_ns) / async_lat.mean_ns) * 100.0;

            if latency_diff.abs() < 5.0 {
                println!("- Latencies are similar (within 5%)");
            } else if latency_diff < 0.0 {
                println!(
                    "- Blocking mode has {:.1}% lower latency",
                    latency_diff.abs()
                );
            } else {
                println!("- Async mode has {:.1}% lower latency", latency_diff);
            }
        }
    }

    println!("- Both modes handle this workload effectively");
    println!("- For production, choose based on your application's architecture");
    println!("- Async scales better with high concurrency (>100 connections)");
    println!("- Blocking is simpler and has lower runtime overhead");

    println!("\nComparison complete!");

    Ok(())
}

/// Helper function to create Args with specified parameters
fn create_args(
    message_size: usize,
    msg_count: usize,
    warmup_iterations: usize,
    percentiles: Vec<f64>,
    blocking: bool,
) -> Args {
    Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        blocking,
        shm_direct: false,
        message_size,
        msg_count,
        one_way: true,
        round_trip: true,
        warmup_iterations,
        percentiles,
        host: "127.0.0.1".to_string(),
        port: 19100,
        concurrency: 1,
        output_file: None,
        duration: None,
        streaming_output_json: None,
        streaming_output_csv: None,
        log_file: None,
        continue_on_error: false,
        include_first_message: false,
        send_delay: None,
        server_affinity: None,
        client_affinity: None,
        buffer_size: None,
        pmq_priority: 0,
        internal_run_as_server: false,
        verbose: 0,
        quiet: false,
        socket_path: None,
        shared_memory_name: None,
        message_queue_name: None,
        internal_latency_file: None,
        // Host-container mode options
        run_mode: ipc_benchmark::cli::RunMode::Standalone,
        stop_container: None,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "rusty-comms".to_string(),
    }
}

/// Helper function to print a comparison row in the table
fn print_comparison_row(label: &str, async_ns: f64, blocking_ns: f64) {
    let async_us = async_ns / 1000.0;
    let blocking_us = blocking_ns / 1000.0;
    let diff_pct = ((blocking_ns - async_ns) / async_ns) * 100.0;

    let diff_str = if diff_pct > 0.0 {
        format!("+{:.1}%", diff_pct)
    } else {
        format!("{:.1}%", diff_pct)
    };

    println!(
        "│ {:<11} │ {:>10.2} µs│ {:>10.2} µs│ {:>12} │",
        label, async_us, blocking_us, diff_str
    );
}
