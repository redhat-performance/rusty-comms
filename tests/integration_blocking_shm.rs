//! Integration tests for Shared Memory in blocking mode
//!
//! These tests verify that shared memory transport works correctly in blocking
//! mode with spawned server processes.

use anyhow::Result;
use ipc_benchmark::{cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism};

/// Verify Shared Memory one-way works end-to-end in blocking mode
///
/// Note: Shared memory uses a unidirectional ring buffer, so we test one-way
/// only. Round-trip would require two separate shared memory regions.
#[test]
fn shm_one_way_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false, // SHM is unidirectional
        warmup_iterations: 0,
        blocking: true, // Enable blocking mode
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        shared_memory_name: Some("ipc_test_blocking_shm_ow".to_string()),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args.clone());

    // Run blocking benchmark (blocks until complete - no .await)
    let _results = runner.run(None)?;

    Ok(())
}

/// Test Shared Memory blocking mode with various message sizes
#[test]
fn shm_blocking_various_sizes() -> Result<()> {
    for (idx, size) in [64, 256, 512].iter().enumerate() {
        let shm_name = format!("ipc_test_blocking_shm_size_{}", idx);

        let args = Args {
            mechanisms: vec![IpcMechanism::SharedMemory],
            one_way: true,
            round_trip: false,
            warmup_iterations: 0,
            blocking: true,
            msg_count: 8,
            message_size: *size,
            shared_memory_name: Some(shm_name),
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args)?;
        let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args.clone());

        runner.run(None)?;
    }
    Ok(())
}

/// Test Shared Memory blocking mode with first message included
#[test]
fn shm_blocking_with_first_message() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 128,
        shared_memory_name: Some("ipc_test_blocking_shm_first".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args.clone());

    let _results = runner.run(None)?;
    Ok(())
}

/// Minimal Shared Memory blocking server spawn smoke test
#[test]
fn shm_blocking_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 1,
        message_size: 64,
        shared_memory_name: Some("ipc_test_blocking_shm_srv".to_string()),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args.clone());

    let transport_config = runner.create_transport_config_internal(&args)?;
    let (mut child, _reader) = runner.spawn_server_process(&transport_config)?;

    // Give server a moment to start (blocking sleep)
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = child.kill();
    Ok(())
}
