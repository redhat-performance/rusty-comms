//! Integration tests for host-to-container IPC benchmarking.
//!
//! These tests verify that the host-container mode works correctly
//! with various IPC mechanisms. They require:
//! - Podman installed and accessible
//! - Container image built: `podman build -t localhost/ipc-benchmark:latest .`
//!
//! Tests are marked `#[ignore]` by default to avoid CI failures on systems
//! without Podman. Run with: `cargo test --test integration_host_container -- --ignored`

#![cfg(target_os = "linux")] // Host-container mode is Linux-only

use anyhow::Result;
use ipc_benchmark::{
    cli::{Args, IpcMechanism, RunMode},
    container::ContainerManager,
    BenchmarkConfig, HostBenchmarkRunner,
};

/// Check if Podman and the container image are available.
fn prerequisites_available() -> bool {
    // Check Podman
    if !ContainerManager::is_podman_available().unwrap_or(false) {
        eprintln!("Podman not available, skipping test");
        return false;
    }

    // Check container image
    let manager = ContainerManager::new("rusty-comms", "localhost/ipc-benchmark:latest");
    if !manager.image_exists().unwrap_or(false) {
        eprintln!("Container image not found, skipping test");
        eprintln!("Build with: podman build -t localhost/ipc-benchmark:latest .");
        return false;
    }

    true
}

// =============================================================================
// UDS Host-Container Tests
// =============================================================================

/// Test UDS one-way latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
#[cfg(unix)]
fn host_container_uds_one_way() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket, args);

    let results = runner.run_blocking(None)?;

    // Verify we got results
    assert!(results.one_way_results.is_some());
    let one_way = results.one_way_results.as_ref().unwrap();
    assert!(one_way.throughput.total_messages > 0);

    Ok(())
}

/// Test UDS round-trip latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
#[cfg(unix)]
fn host_container_uds_round_trip() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket, args);

    let results = runner.run_blocking(None)?;

    // Verify we got round-trip results
    assert!(results.round_trip_results.is_some());
    let round_trip = results.round_trip_results.as_ref().unwrap();
    assert!(round_trip.throughput.total_messages > 0);

    Ok(())
}

// =============================================================================
// SHM Host-Container Tests
// =============================================================================

/// Test SHM one-way latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
fn host_container_shm_one_way() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args);

    let results = runner.run_blocking(None)?;

    // Verify we got results
    assert!(results.one_way_results.is_some());
    let one_way = results.one_way_results.as_ref().unwrap();
    assert!(one_way.throughput.total_messages > 0);

    Ok(())
}

/// Test SHM-direct one-way latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
fn host_container_shm_direct_one_way() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        shm_direct: true, // Use direct memory access
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::SharedMemory, args);

    let results = runner.run_blocking(None)?;

    // Verify we got results
    assert!(results.one_way_results.is_some());
    let one_way = results.one_way_results.as_ref().unwrap();
    assert!(one_way.throughput.total_messages > 0);

    Ok(())
}

// =============================================================================
// TCP Host-Container Tests
// =============================================================================

/// Test TCP one-way latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
fn host_container_tcp_one_way() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

    let results = runner.run_blocking(None)?;

    // Verify we got results
    assert!(results.one_way_results.is_some());
    let one_way = results.one_way_results.as_ref().unwrap();
    assert!(one_way.throughput.total_messages > 0);

    Ok(())
}

/// Test TCP round-trip latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
fn host_container_tcp_round_trip() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 10,
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::TcpSocket, args);

    let results = runner.run_blocking(None)?;

    // Verify we got round-trip results
    assert!(results.round_trip_results.is_some());
    let round_trip = results.round_trip_results.as_ref().unwrap();
    assert!(round_trip.throughput.total_messages > 0);

    Ok(())
}

// =============================================================================
// PMQ Host-Container Tests
// =============================================================================

/// Test PMQ one-way latency in host-container mode.
#[test]
#[ignore = "Requires Podman and container image"]
#[cfg(target_os = "linux")]
fn host_container_pmq_one_way() -> Result<()> {
    if !prerequisites_available() {
        return Ok(());
    }

    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        blocking: true,
        msg_count: 5, // Keep small due to PMQ queue depth limits
        message_size: 128,
        run_mode: RunMode::Host,
        container_image: "localhost/ipc-benchmark:latest".to_string(),
        container_prefix: "test-rusty-comms".to_string(),
        include_first_message: true,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = HostBenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args);

    let results = runner.run_blocking(None)?;

    // Verify we got results
    assert!(results.one_way_results.is_some());
    let one_way = results.one_way_results.as_ref().unwrap();
    assert!(one_way.throughput.total_messages > 0);

    Ok(())
}

// =============================================================================
// Container Lifecycle Tests
// =============================================================================

/// Test that ContainerManager can check for Podman availability.
#[test]
fn container_manager_podman_check() -> Result<()> {
    // This test always runs - just checks if we can query Podman
    let result = ContainerManager::is_podman_available();
    assert!(result.is_ok());
    // Value depends on system configuration
    Ok(())
}

/// Test that ContainerManager can check for image existence.
#[test]
fn container_manager_image_check() -> Result<()> {
    let manager = ContainerManager::new("test", "nonexistent-image:v999");
    let exists = manager.image_exists()?;
    assert!(!exists); // Should not exist
    Ok(())
}

/// Test that ContainerManager correctly generates container names.
#[test]
fn container_manager_naming() {
    let manager = ContainerManager::new("my-prefix", "test:latest");

    #[cfg(unix)]
    assert_eq!(
        manager.container_name(&IpcMechanism::UnixDomainSocket),
        "my-prefix-uds"
    );

    assert_eq!(
        manager.container_name(&IpcMechanism::SharedMemory),
        "my-prefix-shm"
    );

    assert_eq!(
        manager.container_name(&IpcMechanism::TcpSocket),
        "my-prefix-tcp"
    );

    #[cfg(target_os = "linux")]
    assert_eq!(
        manager.container_name(&IpcMechanism::PosixMessageQueue),
        "my-prefix-pmq"
    );
}

/// Test that container exists/running checks work on non-existent containers.
#[test]
#[ignore = "Requires Podman"]
fn container_lifecycle_nonexistent() -> Result<()> {
    if !ContainerManager::is_podman_available().unwrap_or(false) {
        return Ok(());
    }

    let manager = ContainerManager::new("test-nonexistent", "test:latest");
    let name = "test-nonexistent-does-not-exist";

    // Should not exist
    assert!(!manager.exists(name)?);
    assert!(!manager.is_running(name)?);

    // Stop/remove should be no-ops
    manager.stop(name)?;
    manager.remove(name)?;

    Ok(())
}
