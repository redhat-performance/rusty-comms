//! Advanced integration tests for blocking mode
//!
//! These tests verify advanced features like CPU affinity, send delays,
//! and consistency checks between async and blocking modes.

use anyhow::Result;
use ipc_benchmark::{
    cli::Args, BenchmarkConfig, BlockingBenchmarkRunner, IpcMechanism,
};

/// Test blocking mode with CPU affinity for client
#[test]
fn blocking_with_cpu_affinity() -> Result<()> {
    // Only run if we have multiple cores
    let core_ids = core_affinity::get_core_ids();
    if core_ids.is_none() || core_ids.as_ref().unwrap().len() < 2 {
        // Skip test if CPU affinity not supported or not enough cores
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 128,
        host: "127.0.0.1".to_string(),
        port: 22000,
        client_affinity: Some(0), // Pin client to core 0
        server_affinity: Some(1), // Pin server to core 1
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    let _results = runner.run()?;
    Ok(())
}

/// Test blocking mode with send delays
#[test]
fn blocking_with_send_delay() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 8,
        message_size: 64,
        host: "127.0.0.1".to_string(),
        port: 22100,
        send_delay: Some(std::time::Duration::from_millis(10)),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    let _results = runner.run()?;
    Ok(())
}

/// Test blocking mode with both one-way and round-trip enabled
#[test]
fn blocking_both_test_types() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,  // Both enabled
        round_trip: true,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 16,
        message_size: 128,
        host: "127.0.0.1".to_string(),
        port: 22200,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    let results = runner.run()?;

    // Verify both test types produced results
    assert!(
        results.one_way_results.is_some(),
        "One-way results should be present"
    );
    assert!(
        results.round_trip_results.is_some(),
        "Round-trip results should be present"
    );

    Ok(())
}

/// Test blocking mode result structure matches expected format
#[test]
fn blocking_result_structure() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 8,
        message_size: 64,
        host: "127.0.0.1".to_string(),
        port: 22300,
        percentiles: vec![50.0, 95.0, 99.0, 99.9],
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    let results = runner.run()?;

    // Verify result structure
    assert_eq!(results.mechanism, IpcMechanism::TcpSocket);
    assert!(results.one_way_results.is_some());
    
    // Verify one-way results have expected structure
    let one_way = results.one_way_results.unwrap();
    assert!(one_way.latency.is_some());
    
    let latency = one_way.latency.unwrap();
    assert_eq!(latency.total_samples, 8); // Should match msg_count
    assert!(latency.mean_ns > 0.0);
    assert!(latency.median_ns > 0.0);
    assert!(!latency.percentiles.is_empty());
    
    // Verify throughput metrics
    assert!(one_way.throughput.messages_per_second > 0.0);

    Ok(())
}

/// Test that blocking mode handles errors gracefully
#[test]
fn blocking_error_handling() -> Result<()> {
    // Try to use an invalid port (0) which should fail
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 1,
        message_size: 64,
        host: "127.0.0.1".to_string(),
        port: 0, // Invalid port
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    // This should fail, but we're testing that it fails gracefully
    let result = runner.run();
    assert!(result.is_err(), "Should fail with invalid port");

    Ok(())
}

/// Test blocking mode with duration-based test instead of message count
#[test]
fn blocking_duration_based() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 0, // Use duration instead
        duration: Some(std::time::Duration::from_secs(1)),
        message_size: 128,
        host: "127.0.0.1".to_string(),
        port: 22400,
        percentiles: vec![50.0, 95.0, 99.0, 99.9],
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BlockingBenchmarkRunner::new(
        config,
        IpcMechanism::TcpSocket,
        args.clone(),
    );

    let results = runner.run()?;
    
    // Should have run for approximately 1 second (allow some overhead)
    assert!(results.test_duration.as_millis() >= 800, 
            "Expected at least 800ms, got {}ms", 
            results.test_duration.as_millis());

    Ok(())
}

