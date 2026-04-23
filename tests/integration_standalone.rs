//! Integration tests for standalone server/client mode.
//!
//! These tests spawn separate `--server` and `--client` processes using the
//! built binary, verifying end-to-end standalone mode across TCP and UDS
//! mechanisms in both blocking and async modes.

use std::process::{Command, Stdio};
use std::time::Duration;

fn get_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn binary_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("ipc-benchmark");
    path
}

/// Helper: spawn a standalone server, run a client against it, assert both exit cleanly.
fn run_standalone_pair(server_args: &[&str], client_args: &[&str]) {
    let bin = binary_path();

    let server = Command::new(&bin)
        .args(server_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn server");

    // Give the server time to bind and listen
    std::thread::sleep(Duration::from_millis(300));

    let client_output = Command::new(&bin)
        .args(client_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to run client");

    let client_stderr = String::from_utf8_lossy(&client_output.stderr);
    assert!(
        client_output.status.success(),
        "Client exited with error: {}",
        client_stderr
    );

    // Wait for server to exit (client sends shutdown message)
    let server_output = server
        .wait_with_output()
        .expect("Failed to wait for server");

    let server_stderr = String::from_utf8_lossy(&server_output.stderr);
    assert!(
        server_output.status.success(),
        "Server exited with error: {}",
        server_stderr
    );
}

// --- TCP Blocking Tests ---

#[test]
fn standalone_tcp_blocking_round_trip() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--blocking", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-i",
            "100",
            "--round-trip",
        ],
    );
}

#[test]
fn standalone_tcp_blocking_one_way() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--blocking", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-i",
            "100",
            "--one-way",
        ],
    );
}

#[test]
fn standalone_tcp_blocking_both_tests() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--blocking", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-i",
            "100",
            "--one-way",
            "--round-trip",
        ],
    );
}

#[test]
fn standalone_tcp_blocking_concurrent() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &[
            "--server",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-c",
            "2",
        ],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-i",
            "100",
            "--round-trip",
            "-c",
            "2",
        ],
    );
}

// --- TCP Async Tests ---

#[test]
fn standalone_tcp_async_round_trip() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--port",
            &port,
            "-i",
            "100",
            "--round-trip",
        ],
    );
}

#[test]
fn standalone_tcp_async_one_way() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--port",
            &port,
            "-i",
            "100",
            "--one-way",
        ],
    );
}

// --- UDS Tests (Unix only) ---

#[cfg(unix)]
#[test]
fn standalone_uds_blocking_round_trip() {
    let sock = format!("/tmp/ipc_test_standalone_{}.sock", std::process::id());
    run_standalone_pair(
        &[
            "--server",
            "-m",
            "uds",
            "--blocking",
            "--socket-path",
            &sock,
        ],
        &[
            "--client",
            "-m",
            "uds",
            "--blocking",
            "--socket-path",
            &sock,
            "-i",
            "100",
            "--round-trip",
        ],
    );
    let _ = std::fs::remove_file(&sock);
}

#[cfg(unix)]
#[test]
fn standalone_uds_async_round_trip() {
    let sock = format!("/tmp/ipc_test_standalone_async_{}.sock", std::process::id());
    run_standalone_pair(
        &["--server", "-m", "uds", "--socket-path", &sock],
        &[
            "--client",
            "-m",
            "uds",
            "--socket-path",
            &sock,
            "-i",
            "100",
            "--round-trip",
        ],
    );
    let _ = std::fs::remove_file(&sock);
}

#[cfg(unix)]
#[test]
fn standalone_uds_blocking_concurrent() {
    let sock = format!("/tmp/ipc_test_standalone_conc_{}.sock", std::process::id());
    run_standalone_pair(
        &[
            "--server",
            "-m",
            "uds",
            "--blocking",
            "--socket-path",
            &sock,
            "-c",
            "2",
        ],
        &[
            "--client",
            "-m",
            "uds",
            "--blocking",
            "--socket-path",
            &sock,
            "-i",
            "100",
            "--round-trip",
            "-c",
            "2",
        ],
    );
    let _ = std::fs::remove_file(&sock);
}

// --- Duration mode ---

#[test]
fn standalone_tcp_blocking_duration_mode() {
    let port = get_free_port().to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--blocking", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-d",
            "1s",
            "--round-trip",
        ],
    );
}

// --- Output file ---

#[test]
fn standalone_tcp_blocking_output_file() {
    let port = get_free_port().to_string();
    let output = std::env::temp_dir()
        .join(format!("ipc_test_output_{}.json", std::process::id()))
        .to_string_lossy()
        .to_string();
    run_standalone_pair(
        &["--server", "-m", "tcp", "--blocking", "--port", &port],
        &[
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "--port",
            &port,
            "-i",
            "50",
            "--round-trip",
            "--output-file",
            &output,
        ],
    );
    assert!(
        std::path::Path::new(&output).exists(),
        "Output file should be created"
    );
    let contents = std::fs::read_to_string(&output).unwrap();
    assert!(
        contents.contains("TCP Socket"),
        "Output should contain mechanism name"
    );
    let _ = std::fs::remove_file(&output);
}
