//! Basic Blocking Mode Example
//!
//! This example demonstrates the simplest use case for blocking mode:
//! running a single IPC benchmark using synchronous/blocking I/O.
//!
//! Blocking mode uses pure standard library primitives (std::net, std::thread)
//! without any async runtime overhead. This is ideal for:
//! - Testing synchronous systems
//! - Establishing baseline performance metrics
//! - Educational purposes (simpler execution model)
//! - Systems where async complexity is not warranted
//!
//! # Usage
//!
//! ```bash
//! cargo run --example blocking_basic
//! ```
//!
//! # What This Example Does
//!
//! 1. Configures a simple TCP socket benchmark
//! 2. Runs in blocking mode (--blocking flag)
//! 3. Sends 10,000 messages of 1KB each
//! 4. Performs both one-way and round-trip tests
//! 5. Prints results to console
//!
//! # Expected Output
//!
//! You should see:
//! - Configuration summary
//! - Progress indicators during execution
//! - Latency metrics (mean, median, percentiles)
//! - Throughput metrics (messages/sec, MB/s)
//!
//! # Performance Notes
//!
//! Blocking mode typically shows:
//! - Lower CPU overhead (no async runtime)
//! - Predictable latency (no async task scheduling)
//! - Simpler stack traces for debugging
//! - Similar throughput to async for single connections

use anyhow::Result;
use ipc_benchmark::{
    benchmark_blocking::BlockingBenchmarkRunner,
    cli::Args,
    BenchmarkConfig,
    IpcMechanism,
};

fn main() -> Result<()> {
    // Initialize tracing for console output
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();

    println!("=== Blocking Mode Basic Example ===\n");
    println!("This example runs a simple TCP benchmark in blocking mode.");
    println!("Blocking mode uses std::net and std::thread (no async runtime).\n");

    // Configure the benchmark parameters
    let args = Args {
        // Use TCP sockets for this example (works on all platforms)
        mechanisms: vec![IpcMechanism::TcpSocket],
        
        // Enable blocking mode - this is the key flag!
        blocking: true,
        
        // Message configuration
        message_size: 1024,  // 1 KB messages
        msg_count: 10000,    // Send 10,000 messages
        
        // Test types
        one_way: true,       // Measure one-way latency
        round_trip: true,    // Measure round-trip latency
        
        // Warmup for stable measurements
        warmup_iterations: 1000,
        
        // Performance analysis
        percentiles: vec![50.0, 95.0, 99.0, 99.9],
        
        // TCP-specific configuration
        host: "127.0.0.1".to_string(),
        port: 19000,  // Use a non-standard port
        
        // Single-threaded for simplicity
        concurrency: 1,
        
        // No output file - just print to console
        output_file: None,
        
        // Other flags - use sensible defaults
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
        
        // Internal flag (not for external use)
        internal_run_as_server: false,
        
        // Verbosity
        verbose: 0,
        
        // Other internal fields
        quiet: false,
        socket_path: None,
        shared_memory_name: None,
        message_queue_name: None,
    };

    println!("Configuration:");
    println!("  Mechanism:    TCP Socket");
    println!("  Mode:         Blocking (std::net)");
    println!("  Message Size: {} bytes", args.message_size);
    println!("  Message Count: {}", args.msg_count);
    println!("  Warmup:       {} iterations", args.warmup_iterations);
    println!("  Tests:        One-Way + Round-Trip\n");

    println!("Starting benchmark...\n");

    // Create benchmark configuration from args
    let config = BenchmarkConfig::from_args(&args)?;

    // Create the blocking benchmark runner for TCP
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args,
    );

    // Run the benchmark - this is a blocking call
    match runner.run() {
        Ok(results) => {
            println!("\n=== Benchmark Complete ===\n");
            
            // Print one-way results
            if let Some(one_way) = &results.one_way_results {
                if let Some(latency) = &one_way.latency {
                    println!("One-Way Latency:");
                    println!("  Mean:   {:.2} µs", latency.mean_ns as f64 / 1000.0);
                    println!("  Median: {:.2} µs", latency.median_ns as f64 / 1000.0);
                    println!("  Min:    {:.2} µs", latency.min_ns as f64 / 1000.0);
                    println!("  Max:    {:.2} µs", latency.max_ns as f64 / 1000.0);
                    
                    for percentile in &latency.percentiles {
                        println!("  P{:.1}:    {:.2} µs", 
                            percentile.percentile, 
                            percentile.value_ns as f64 / 1000.0);
                    }
                }
                
                println!("\nOne-Way Throughput:");
                println!("  {:.2} msg/s", one_way.throughput.messages_per_second);
                println!("  {:.2} MB/s", 
                    one_way.throughput.bytes_per_second / 1_000_000.0);
            }
            
            // Print round-trip results
            if let Some(round_trip) = &results.round_trip_results {
                if let Some(latency) = &round_trip.latency {
                    println!("\nRound-Trip Latency:");
                    println!("  Mean:   {:.2} µs", latency.mean_ns as f64 / 1000.0);
                    println!("  Median: {:.2} µs", latency.median_ns as f64 / 1000.0);
                    println!("  Min:    {:.2} µs", latency.min_ns as f64 / 1000.0);
                    println!("  Max:    {:.2} µs", latency.max_ns as f64 / 1000.0);
                    
                    for percentile in &latency.percentiles {
                        println!("  P{:.1}:    {:.2} µs", 
                            percentile.percentile, 
                            percentile.value_ns as f64 / 1000.0);
                    }
                }
                
                println!("\nRound-Trip Throughput:");
                println!("  {:.2} msg/s", round_trip.throughput.messages_per_second);
                println!("  {:.2} MB/s", 
                    round_trip.throughput.bytes_per_second / 1_000_000.0);
            }
            
            println!("\nTest Duration: {:.2}s", results.test_duration.as_secs_f64());
            println!("\nSuccess! Blocking mode works perfectly.");
            Ok(())
        }
        Err(e) => {
            eprintln!("\n=== Benchmark Failed ===");
            eprintln!("Error: {}", e);
            Err(e)
        }
    }
}

// Example output you can expect:
//
// === Blocking Mode Basic Example ===
//
// This example runs a simple TCP benchmark in blocking mode.
// Blocking mode uses std::net and std::thread (no async runtime).
//
// Configuration:
//   Mechanism:    TCP Socket
//   Mode:         Blocking (std::net)
//   Message Size: 1024 bytes
//   Message Count: 10000
//   Warmup:       1000 iterations
//   Tests:        One-Way + Round-Trip
//
// Starting benchmark...
//
// Benchmark Configuration:
// -----------------------------------------------------------------
//   Mechanisms:         TcpSocket
//   Message Size:       1024 bytes
//   Iterations:         10000
//   Warmup Iterations:  1000
//   Test Types:         One-Way, Round-Trip
//   Execution Mode:     Blocking
//   Log File:           blocking_basic_example.log
// -----------------------------------------------------------------
//
// [... benchmark runs ...]
//
// Benchmark Results:
// -----------------------------------------------------------------
// Mechanism: TcpSocket
//   Message Size: 1024 bytes
//   One-Way Latency:
//       Mean: 3.45 us, P95: 5.12 us, P99: 8.23 us
//       Min:  2.10 us, Max: 45.60 us
//   Round-Trip Latency:
//       Mean: 6.82 us, P95: 10.31 us, P99: 15.44 us
//       Min:  5.30 us, Max: 92.10 us
//   Throughput:
//       Average: 143.21 MB/s
//   Totals:
//       Messages: 20000, Data: 19.53 MB
// -----------------------------------------------------------------
//
// === Benchmark Complete ===
// Results are displayed above.
// Log file: blocking_basic_example.log

