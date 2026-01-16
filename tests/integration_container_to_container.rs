//! Integration tests for container-to-container IPC benchmarking.
//!
//! These tests verify that the container-to-container mode works correctly
//! with various IPC mechanisms. They require:
//! - Podman installed and accessible
//! - Container image built: `podman build -t localhost/ipc-benchmark:latest .`
//!
//! Tests are marked `#[ignore]` by default to avoid CI failures on systems
//! without Podman. Run with: `cargo test --test integration_container_to_container -- --ignored`

#![cfg(target_os = "linux")] // Container-to-container mode is Linux-only

use std::process::{Command, Stdio};
use std::time::Duration;
use std::thread;

/// Check if Podman is available.
fn is_podman_available() -> bool {
    Command::new("podman")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if the container image exists.
fn image_exists() -> bool {
    Command::new("podman")
        .args(["image", "exists", "localhost/ipc-benchmark:latest"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check prerequisites for C2C tests.
fn prerequisites_available() -> bool {
    if !is_podman_available() {
        eprintln!("Podman not available, skipping test");
        return false;
    }
    if !image_exists() {
        eprintln!("Container image not found, skipping test");
        eprintln!("Build with: podman build -t localhost/ipc-benchmark:latest .");
        return false;
    }
    true
}

/// Cleanup any leftover containers from previous test runs.
fn cleanup_containers(prefix: &str) {
    // Stop and remove server container
    let _ = Command::new("podman")
        .args(["stop", "-t", "1", &format!("{}-server", prefix)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("podman")
        .args(["rm", "-f", &format!("{}-server", prefix)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    
    // Stop and remove sender container
    let _ = Command::new("podman")
        .args(["stop", "-t", "1", &format!("{}-sender", prefix)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("podman")
        .args(["rm", "-f", &format!("{}-sender", prefix)])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// =============================================================================
// RunMode::Sender Tests
// =============================================================================

/// Test that RunMode::Sender is correctly parsed.
#[test]
fn test_run_mode_sender_parsing() {
    use ipc_benchmark::cli::{Args, RunMode};
    use clap::Parser;
    
    let args = Args::parse_from([
        "ipc-benchmark",
        "-m", "tcp",
        "--run-mode", "sender",
        "--host", "192.168.1.100",
        "--port", "9999",
    ]);
    
    assert_eq!(args.run_mode, RunMode::Sender);
    assert_eq!(args.host, "192.168.1.100");
    assert_eq!(args.port, 9999);
}

/// Test that Sender mode works with all transport options.
#[test]
fn test_run_mode_sender_with_all_transports() {
    use ipc_benchmark::cli::{Args, RunMode, IpcMechanism};
    use clap::Parser;
    
    // TCP
    let args_tcp = Args::parse_from([
        "ipc-benchmark",
        "-m", "tcp",
        "--run-mode", "sender",
        "--blocking",
    ]);
    assert_eq!(args_tcp.run_mode, RunMode::Sender);
    assert_eq!(args_tcp.mechanisms[0], IpcMechanism::TcpSocket);
    
    // UDS
    let args_uds = Args::parse_from([
        "ipc-benchmark",
        "-m", "uds",
        "--run-mode", "sender",
        "--socket-path", "/tmp/test.sock",
        "--blocking",
    ]);
    assert_eq!(args_uds.run_mode, RunMode::Sender);
    assert_eq!(args_uds.mechanisms[0], IpcMechanism::UnixDomainSocket);
    
    // SHM
    let args_shm = Args::parse_from([
        "ipc-benchmark",
        "-m", "shm",
        "--run-mode", "sender",
        "--shared-memory-name", "/ipc_test",
        "--blocking",
    ]);
    assert_eq!(args_shm.run_mode, RunMode::Sender);
    assert_eq!(args_shm.mechanisms[0], IpcMechanism::SharedMemory);
    
    // PMQ
    let args_pmq = Args::parse_from([
        "ipc-benchmark",
        "-m", "pmq",
        "--run-mode", "sender",
        "--message-queue-name", "/ipc_test_mq",
        "--blocking",
    ]);
    assert_eq!(args_pmq.run_mode, RunMode::Sender);
    assert_eq!(args_pmq.mechanisms[0], IpcMechanism::PosixMessageQueue);
}

// =============================================================================
// Container-to-Container TCP Tests
// =============================================================================

/// Test TCP communication between two containers.
#[test]
#[ignore = "Requires Podman and container image"]
fn container_to_container_tcp_basic() {
    if !prerequisites_available() {
        return;
    }
    
    let prefix = "c2c-test-tcp";
    cleanup_containers(prefix);
    
    // Create a pod for network sharing
    let pod_name = format!("{}-pod", prefix);
    let _ = Command::new("podman")
        .args(["pod", "rm", "-f", &pod_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    
    let pod_status = Command::new("podman")
        .args(["pod", "create", "--name", &pod_name])
        .stdout(Stdio::null())
        .status()
        .expect("Failed to create pod");
    assert!(pod_status.success(), "Failed to create pod");
    
    // Start server container (client mode = receiver)
    let server_name = format!("{}-server", prefix);
    let server = Command::new("podman")
        .args([
            "run", "-d",
            "--pod", &pod_name,
            "--name", &server_name,
            "localhost/ipc-benchmark:latest",
            "-m", "tcp",
            "--run-mode", "client",
            "--blocking",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--host", "127.0.0.1",
            "--port", "18080",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start server container");
    
    let server_output = server.wait_with_output().expect("Failed to get server output");
    assert!(server_output.status.success(), "Server container failed to start");
    
    // Wait for server to be ready
    thread::sleep(Duration::from_millis(500));
    
    // Start sender container
    let sender_name = format!("{}-sender", prefix);
    let sender_output = Command::new("podman")
        .args([
            "run", "--rm",
            "--pod", &pod_name,
            "--name", &sender_name,
            "localhost/ipc-benchmark:latest",
            "-m", "tcp",
            "--run-mode", "sender",
            "--blocking",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--host", "127.0.0.1",
            "--port", "18080",
        ])
        .output()
        .expect("Failed to run sender container");
    
    // Check sender completed
    let stdout = String::from_utf8_lossy(&sender_output.stdout);
    let stderr = String::from_utf8_lossy(&sender_output.stderr);
    
    // Cleanup
    cleanup_containers(prefix);
    let _ = Command::new("podman")
        .args(["pod", "rm", "-f", &pod_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    
    assert!(
        sender_output.status.success() || stdout.contains("messages sent") || stdout.contains("throughput"),
        "Sender failed. stdout: {}\nstderr: {}", stdout, stderr
    );
}

// =============================================================================
// Container-to-Container UDS Tests
// =============================================================================

/// Test UDS communication between two containers using shared volume.
#[test]
#[ignore = "Requires Podman and container image"]
fn container_to_container_uds_basic() {
    if !prerequisites_available() {
        return;
    }
    
    let prefix = "c2c-test-uds";
    cleanup_containers(prefix);
    
    // Create temp directory for socket
    let socket_dir = format!("/tmp/{}-sockets", prefix);
    let _ = std::fs::remove_dir_all(&socket_dir);
    std::fs::create_dir_all(&socket_dir).expect("Failed to create socket directory");
    
    let _socket_path = format!("{}/benchmark.sock", socket_dir);
    
    // Start server container (client mode = receiver)
    let server_name = format!("{}-server", prefix);
    let server = Command::new("podman")
        .args([
            "run", "-d",
            "-v", &format!("{}:/sockets:z", socket_dir),
            "--name", &server_name,
            "localhost/ipc-benchmark:latest",
            "-m", "uds",
            "--run-mode", "client",
            "--blocking",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--socket-path", "/sockets/benchmark.sock",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start server container");
    
    let server_output = server.wait_with_output().expect("Failed to get server output");
    assert!(server_output.status.success(), "Server container failed to start");
    
    // Wait for server to create socket
    thread::sleep(Duration::from_millis(500));
    
    // Start sender container
    let sender_name = format!("{}-sender", prefix);
    let sender_output = Command::new("podman")
        .args([
            "run", "--rm",
            "-v", &format!("{}:/sockets:z", socket_dir),
            "--name", &sender_name,
            "localhost/ipc-benchmark:latest",
            "-m", "uds",
            "--run-mode", "sender",
            "--blocking",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--socket-path", "/sockets/benchmark.sock",
        ])
        .output()
        .expect("Failed to run sender container");
    
    let stdout = String::from_utf8_lossy(&sender_output.stdout);
    let stderr = String::from_utf8_lossy(&sender_output.stderr);
    
    // Cleanup
    cleanup_containers(prefix);
    let _ = std::fs::remove_dir_all(&socket_dir);
    
    assert!(
        sender_output.status.success() || stdout.contains("messages sent") || stdout.contains("throughput"),
        "Sender failed. stdout: {}\nstderr: {}", stdout, stderr
    );
}

// =============================================================================
// Container-to-Container SHM Tests
// =============================================================================

/// Test shared memory communication between two containers.
#[test]
#[ignore = "Requires Podman and container image"]
fn container_to_container_shm_direct_basic() {
    if !prerequisites_available() {
        return;
    }
    
    let prefix = "c2c-test-shm";
    cleanup_containers(prefix);
    
    // Create temp directory for SHM files
    let shm_dir = format!("/tmp/{}-shm", prefix);
    let _ = std::fs::remove_dir_all(&shm_dir);
    std::fs::create_dir_all(&shm_dir).expect("Failed to create SHM directory");
    
    let shm_name = "/ipc_c2c_test";
    
    // Start server container (client mode = receiver)
    let server_name = format!("{}-server", prefix);
    let server = Command::new("podman")
        .args([
            "run", "-d",
            "-v", &format!("{}:/dev/shm:z", shm_dir),
            "--name", &server_name,
            "localhost/ipc-benchmark:latest",
            "-m", "shm",
            "--run-mode", "client",
            "--blocking",
            "--shm-direct",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--shared-memory-name", shm_name,
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start server container");
    
    let server_output = server.wait_with_output().expect("Failed to get server output");
    assert!(server_output.status.success(), "Server container failed to start");
    
    // Wait for server to create SHM
    thread::sleep(Duration::from_millis(500));
    
    // Start sender container
    let sender_name = format!("{}-sender", prefix);
    let sender_output = Command::new("podman")
        .args([
            "run", "--rm",
            "-v", &format!("{}:/dev/shm:z", shm_dir),
            "--name", &sender_name,
            "localhost/ipc-benchmark:latest",
            "-m", "shm",
            "--run-mode", "sender",
            "--blocking",
            "--shm-direct",
            "--one-way",
            "-i", "10",
            "-s", "128",
            "--shared-memory-name", shm_name,
        ])
        .output()
        .expect("Failed to run sender container");
    
    let stdout = String::from_utf8_lossy(&sender_output.stdout);
    let stderr = String::from_utf8_lossy(&sender_output.stderr);
    
    // Cleanup
    cleanup_containers(prefix);
    let _ = std::fs::remove_dir_all(&shm_dir);
    
    assert!(
        sender_output.status.success() || stdout.contains("messages sent") || stdout.contains("throughput"),
        "Sender failed. stdout: {}\nstderr: {}", stdout, stderr
    );
}

// =============================================================================
// Container-to-Container PMQ Tests  
// =============================================================================

/// Test POSIX message queue communication between two containers with shared IPC namespace.
#[test]
#[ignore = "Requires Podman and container image"]
fn container_to_container_pmq_basic() {
    if !prerequisites_available() {
        return;
    }
    
    let prefix = "c2c-test-pmq";
    cleanup_containers(prefix);
    
    let mq_name = "/ipc_c2c_test_mq";
    
    // Start server container with IPC=host (client mode = receiver)
    let server_name = format!("{}-server", prefix);
    let server = Command::new("podman")
        .args([
            "run", "-d",
            "--ipc", "host",
            "--name", &server_name,
            "localhost/ipc-benchmark:latest",
            "-m", "pmq",
            "--run-mode", "client",
            "--blocking",
            "--one-way",
            "-i", "5",  // PMQ has limited queue depth
            "-s", "128",
            "--message-queue-name", mq_name,
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start server container");
    
    let server_output = server.wait_with_output().expect("Failed to get server output");
    assert!(server_output.status.success(), "Server container failed to start");
    
    // Wait for server to create message queue
    thread::sleep(Duration::from_millis(500));
    
    // Start sender container with IPC=host
    let sender_name = format!("{}-sender", prefix);
    let sender_output = Command::new("podman")
        .args([
            "run", "--rm",
            "--ipc", "host",
            "--name", &sender_name,
            "localhost/ipc-benchmark:latest",
            "-m", "pmq",
            "--run-mode", "sender",
            "--blocking",
            "--one-way",
            "-i", "5",
            "-s", "128",
            "--message-queue-name", mq_name,
        ])
        .output()
        .expect("Failed to run sender container");
    
    let stdout = String::from_utf8_lossy(&sender_output.stdout);
    let stderr = String::from_utf8_lossy(&sender_output.stderr);
    
    // Cleanup
    cleanup_containers(prefix);
    
    assert!(
        sender_output.status.success() || stdout.contains("messages sent") || stdout.contains("throughput"),
        "Sender failed. stdout: {}\nstderr: {}", stdout, stderr
    );
}
