//! Integration tests for Shared Memory Direct in blocking mode.
//!
//! These tests verify that the high-performance direct-memory shared memory
//! transport works correctly end-to-end in blocking mode with spawned server
//! processes.

use anyhow::Result;
use ipc_benchmark::{
    cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism,
};

/// Verify SHM-direct one-way works end-to-end in blocking mode.
///
/// SHM-direct uses a single fixed-size struct in shared memory with
/// pthread mutex/condvar (or futex on Linux) for synchronization.
#[test]
fn shm_direct_one_way_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        shm_direct: true,
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        shared_memory_name: Some(
            "ipc_test_direct_ow".to_string(),
        ),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::SharedMemory,
        args.clone(),
    );

    let _results = runner.run(None)?;
    Ok(())
}

/// Verify SHM-direct round-trip works end-to-end in blocking mode.
///
/// Unlike the ring-buffer SHM transport (which is unidirectional), SHM-direct
/// supports bidirectional communication through its per-direction condvars,
/// so round-trip tests should work correctly.
#[test]
fn shm_direct_round_trip_blocking_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true,
        shm_direct: true,
        concurrency: 1,
        msg_count: 32,
        message_size: 128,
        shared_memory_name: Some(
            "ipc_test_direct_rt".to_string(),
        ),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::SharedMemory,
        args.clone(),
    );

    let _results = runner.run(None)?;
    Ok(())
}

/// Test SHM-direct blocking mode with various message sizes.
///
/// Exercises the fixed-size struct layout with different payload sizes
/// to verify padding and alignment handling.
#[test]
fn shm_direct_blocking_various_sizes() -> Result<()> {
    for (idx, size) in [64, 256, 512, 1024].iter().enumerate() {
        let shm_name =
            format!("ipc_test_direct_size_{}", idx);

        let args = Args {
            mechanisms: vec![IpcMechanism::SharedMemory],
            one_way: true,
            round_trip: false,
            warmup_iterations: 0,
            blocking: true,
            shm_direct: true,
            msg_count: 8,
            message_size: *size,
            shared_memory_name: Some(shm_name),
            ..Default::default()
        };

        let config = BenchmarkConfig::from_args(&args)?;
        let runner = BlockingBenchmarkRunner::new(
            config,
            IpcMechanism::SharedMemory,
            args.clone(),
        );

        runner.run(None)?;
    }
    Ok(())
}

/// Test SHM-direct blocking mode with first message included in
/// results.
///
/// The first message typically has higher latency due to page faults
/// and cache warming. This test verifies that `include_first_message`
/// works correctly with SHM-direct.
#[test]
fn shm_direct_blocking_with_first_message() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        shm_direct: true,
        msg_count: 16,
        message_size: 128,
        shared_memory_name: Some(
            "ipc_test_direct_first".to_string(),
        ),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::SharedMemory,
        args.clone(),
    );

    let _results = runner.run(None)?;
    Ok(())
}

/// Minimal SHM-direct server spawn smoke test.
///
/// Verifies that the server process can be spawned and reaches the
/// ready state for SHM-direct mode.
#[test]
fn shm_direct_blocking_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        shm_direct: true,
        msg_count: 1,
        message_size: 64,
        shared_memory_name: Some(
            "ipc_test_direct_srv".to_string(),
        ),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::SharedMemory,
        args.clone(),
    );

    let transport_config =
        runner.create_transport_config_internal(&args)?;
    let (mut child, _reader) =
        runner.spawn_server_process(&transport_config)?;

    // Give server a moment to start (blocking sleep).
    // Justification: The spawned server process needs time to
    // initialize shared memory and signal readiness via pipe.
    std::thread::sleep(std::time::Duration::from_millis(100));

    let _ = child.kill();
    Ok(())
}

/// Test SHM-direct with warmup iterations.
///
/// Verifies that warmup iterations run correctly before measurement
/// begins, which is important for cache warming with direct memory
/// access.
#[test]
fn shm_direct_blocking_with_warmup() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 5,
        blocking: true,
        shm_direct: true,
        msg_count: 16,
        message_size: 128,
        shared_memory_name: Some(
            "ipc_test_direct_warmup".to_string(),
        ),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::SharedMemory,
        args.clone(),
    );

    let _results = runner.run(None)?;
    Ok(())
}
