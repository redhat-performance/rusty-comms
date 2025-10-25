//! Integration tests for TCP Socket in blocking mode
//!
//! These tests verify that TCP socket transport works correctly in blocking
//! mode with spawned server processes. They mirror the async TCP tests but
//! use pure blocking I/O operations.

use anyhow::Result;
use ipc_benchmark::{cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism};

/// Verify TCP round-trip works end-to-end in blocking mode with a spawned
/// server process.
///
/// This is a lightweight smoke test that ensures the process-based
/// client/server separation functions correctly for TCP in blocking mode.
#[test]
fn tcp_round_trip_blocking_smoke() -> Result<()> {
    // Keep the run short and deterministic
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true, // Enable blocking mode
        // Explicit network parameters
        host: "127.0.0.1".to_string(),
        port: 21000, // Unique port for blocking tests
        // Ensure non-zero concurrency and finite iteration mode
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

    // Run blocking benchmark (blocks until complete - no .await)
    let _results = runner.run(None)?;

    Ok(())
}

/// Test TCP blocking mode with one-way latency measurement
#[test]
fn tcp_one_way_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        host: "127.0.0.1".to_string(),
        port: 21001, // Different port to avoid conflicts
        msg_count: 16,
        message_size: 64,
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

    let _results = runner.run(None)?;
    Ok(())
}

/// Test TCP blocking mode with various message sizes
#[test]
fn tcp_blocking_various_sizes() -> Result<()> {
    for size in [64, 256, 1024] {
        let args = Args {
            mechanisms: vec![IpcMechanism::TcpSocket],
            one_way: true,
            round_trip: false,
            warmup_iterations: 0,
            blocking: true,
            host: "127.0.0.1".to_string(),
            port: 21002 + (size / 64) as u16, // Unique port per size
            msg_count: 8,
            message_size: size,
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args)?;
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

        runner.run(None)?;
    }
    Ok(())
}

/// Minimal TCP blocking server spawn smoke test
#[test]
fn tcp_blocking_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 1,
        message_size: 64,
        host: "127.0.0.1".to_string(),
        port: 21100,
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

    let transport_config = runner.create_transport_config_internal(&args)?;
    let (mut child, _reader) = runner.spawn_server_process(&transport_config)?;

    // Give server a moment to start (blocking sleep)
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = child.kill();
    Ok(())
}
