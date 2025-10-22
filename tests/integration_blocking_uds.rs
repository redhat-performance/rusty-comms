//! Integration tests for Unix Domain Sockets in blocking mode
//!
//! These tests verify that UDS transport works correctly in blocking mode
//! with spawned server processes. They are Unix-specific and will not compile
//! on Windows.

#![cfg(unix)] // Unix Domain Sockets are Unix-only

use anyhow::Result;
use ipc_benchmark::{
    cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism,
};

/// Verify UDS round-trip works end-to-end in blocking mode with a spawned
/// server process.
#[test]
fn uds_round_trip_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true, // Enable blocking mode
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        socket_path: Some("/tmp/ipc_test_blocking_uds_rt.sock".to_string()),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::UnixDomainSocket,
        args.clone(),
    );

    // Run blocking benchmark (blocks until complete - no .await)
    let _results = runner.run()?;

    Ok(())
}

/// Test UDS blocking mode with one-way latency measurement
#[test]
fn uds_one_way_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 64,
        socket_path: Some("/tmp/ipc_test_blocking_uds_ow.sock".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::UnixDomainSocket,
        args.clone(),
    );

    let _results = runner.run()?;
    Ok(())
}

/// Test UDS blocking mode with various message sizes
#[test]
fn uds_blocking_various_sizes() -> Result<()> {
    for (idx, size) in [64, 256, 1024].iter().enumerate() {
        let socket_path = format!(
            "/tmp/ipc_test_blocking_uds_size_{}.sock",
            idx
        );

        let args = Args {
            mechanisms: vec![IpcMechanism::UnixDomainSocket],
            one_way: true,
            round_trip: false,
            warmup_iterations: 0,
            blocking: true,
            msg_count: 8,
            message_size: *size,
            socket_path: Some(socket_path),
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args)?;
        let runner = BlockingBenchmarkRunner::new(
            config,
            IpcMechanism::UnixDomainSocket,
            args.clone(),
        );

        runner.run()?;
    }
    Ok(())
}

/// Minimal UDS blocking server spawn smoke test
#[test]
fn uds_blocking_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 1,
        message_size: 64,
        socket_path: Some("/tmp/ipc_test_blocking_uds_srv.sock".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::UnixDomainSocket,
        args.clone(),
    );

    let transport_config = runner.create_transport_config_internal(&args)?;
    let (mut child, _reader) = runner.spawn_server_process(&transport_config)?;

    // Give server a moment to start (blocking sleep)
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = child.kill();
    Ok(())
}

