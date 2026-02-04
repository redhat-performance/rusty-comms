//! Integration tests for POSIX Message Queues in blocking mode
//!
//! These tests verify that POSIX message queue transport works correctly in
//! blocking mode with spawned server processes. They are Linux-specific.

#![cfg(target_os = "linux")] // POSIX message queues are Linux-only

use anyhow::Result;
use ipc_benchmark::{cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism};
use std::io::Read;

/// Verify PMQ round-trip works end-to-end in blocking mode with a spawned
/// server process.
#[test]
fn pmq_round_trip_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true, // Enable blocking mode
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        buffer_size: Some(1024), // Use small buffer to fit in ulimit
        message_queue_name: Some("/ipc_test_blocking_pmq_rt".to_string()),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner =
        BlockingBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

    // Run blocking benchmark (blocks until complete - no .await)
    let _results = runner.run(None)?;

    Ok(())
}

/// Test PMQ blocking mode with one-way latency measurement
#[test]
fn pmq_one_way_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 64,
        buffer_size: Some(1024),
        message_queue_name: Some("/ipc_test_blocking_pmq_ow".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner =
        BlockingBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

    let _results = runner.run(None)?;
    Ok(())
}

/// Test PMQ blocking mode with various message sizes
#[test]
fn pmq_blocking_various_sizes() -> Result<()> {
    // Keep sizes small for PMQ (default max is 8192 bytes)
    for (idx, size) in [64, 256, 512].iter().enumerate() {
        let queue_name = format!("/ipc_test_blocking_pmq_size_{}", idx);

        let args = Args {
            mechanisms: vec![IpcMechanism::PosixMessageQueue],
            one_way: true,
            round_trip: false,
            warmup_iterations: 0,
            blocking: true,
            msg_count: 8,
            message_size: *size,
            buffer_size: Some(1024),
            message_queue_name: Some(queue_name),
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args)?;
        let runner =
            BlockingBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

        runner.run(None)?;
    }
    Ok(())
}

/// Test PMQ blocking mode with priority
#[test]
fn pmq_blocking_with_priority() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 128,
        buffer_size: Some(1024),
        message_queue_name: Some("/ipc_test_blocking_pmq_prio".to_string()),
        pmq_priority: 5, // Non-zero priority
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner =
        BlockingBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

    let _results = runner.run(None)?;
    Ok(())
}

/// Minimal PMQ blocking server spawn smoke test
#[test]
fn pmq_blocking_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 1,
        message_size: 64,
        buffer_size: Some(1024),
        message_queue_name: Some("/ipc_test_blocking_pmq_srv".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner =
        BlockingBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

    let transport_config = runner.create_transport_config_internal(&args)?;
    let (mut child, mut reader) = runner.spawn_server_process(&transport_config)?;

    // Wait for the server to signal readiness (no fixed sleeps).
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;

    // Gracefully shut down the server so it can unlink the queues.
    let mut client_transport = ipc_benchmark::ipc::BlockingTransportFactory::create(
        &IpcMechanism::PosixMessageQueue,
        args.shm_direct,
    )?;
    client_transport.start_client_blocking(&transport_config)?;
    client_transport.send_blocking(&ipc_benchmark::ipc::Message::new(
        0,
        Vec::new(),
        ipc_benchmark::ipc::MessageType::Shutdown,
    ))?;
    client_transport.close_blocking()?;

    let _ = child.wait();
    Ok(())
}
