#!/usr/bin/env python3
"""
Comprehensive IPC Benchmark Suite Runner

Runs all mechanisms and configurations:
- Mechanisms: TCP, UDS, SHM, PMQ
- Modes: 
  - Standalone (async/blocking/shm-direct)
  - Host-to-QM (async/blocking)
  - Container-to-Container (async/blocking)
- Test types: Iteration-based (50k) and Duration-based (10s)

Message sizes:
- PMQ: 512, 4096, 8100
- Others: 64, 512, 4096, 8192, 65536

Host-to-QM tests require:
- QM container running with proper configuration
- Binary copied to QM container at /tmp/ipc-benchmark
- See /root/scripts/qm/HOST_TO_QM_IPC_SETUP.md for setup details

Container-to-Container tests require:
- Podman installed and working
- Uses fedora:35 as base image (glibc 2.34 matches host)
"""

import subprocess
import sys
import os
import json
import time
import signal
from pathlib import Path
from typing import List, Dict, Optional
from dataclasses import dataclass
from enum import Enum

# SELinux helpers
def get_selinux_mode() -> str:
    """Get current SELinux mode (Enforcing, Permissive, or Disabled)."""
    try:
        result = subprocess.run(["getenforce"], capture_output=True, text=True, timeout=5)
        if result.returncode == 0:
            return result.stdout.strip()
        return "Unknown"
    except (subprocess.TimeoutExpired, FileNotFoundError):
        return "N/A (getenforce not found)"


def set_selinux_permissive() -> bool:
    """Set SELinux to Permissive mode. Returns True on success."""
    try:
        result = subprocess.run(["setenforce", "0"], capture_output=True, text=True, timeout=5)
        return result.returncode == 0
    except (subprocess.TimeoutExpired, FileNotFoundError, PermissionError):
        return False


def check_selinux_for_tests() -> None:
    """Check SELinux status and warn/set if needed for H2QM/C2C tests."""
    mode = get_selinux_mode()
    print(f"SELinux mode: {mode}")
    
    if mode == "Enforcing":
        print("  WARNING: SELinux is Enforcing. H2QM PMQ tests may fail.")
        print("  Attempting to set Permissive mode...")
        if set_selinux_permissive():
            print("  SELinux set to Permissive successfully.")
        else:
            print("  Failed to set Permissive mode. Run as root or use: sudo setenforce 0")


# Configuration
BINARY = "/root/hostcont/rusty-comms/target/release/ipc-benchmark"
QM_BINARY = "/tmp/ipc-benchmark"
QM_CONTAINER = "qm"
OUTPUT_DIR = Path("/root/scripts/fullrun/out")
RUSTY_COMMS_DIR = Path("/root/hostcont/rusty-comms")

# C2C container configuration
C2C_SERVER_CONTAINER = "c2c-server"
C2C_SENDER_CONTAINER = "c2c-sender"
C2C_SHARED_DIR = Path("/tmp/c2c_benchmark")
C2C_CONTAINER_BINARY = "/app/ipc-benchmark"
# Use fedora:35 for glibc 2.34 compatibility with host-built binary
C2C_BASE_IMAGE = "fedora:35"

# Test parameters
ITERATIONS = 50000
DURATION_SECS = 10
WARMUP = 100

# Message sizes
PMQ_SIZES = [512, 1024, 4096, 8100]
GENERAL_SIZES = [64, 512, 1024, 4096, 8192, 65536]

# Runtime settings (set by main())
STREAM_OUTPUT = False


def check_and_update_qm_binary() -> bool:
    """Check if QM binary is up-to-date with host binary, update if needed.
    
    Returns True if binary is ready (updated or already current), False on error.
    """
    if not Path(BINARY).exists():
        print(f"ERROR: Host binary not found: {BINARY}")
        print("  Run: cargo build --release")
        return False
    
    # Get host binary modification time
    host_mtime = Path(BINARY).stat().st_mtime
    
    # Check if QM container is running
    result = subprocess.run(
        ["podman", "inspect", "-f", "{{.State.Running}}", QM_CONTAINER],
        capture_output=True, text=True, timeout=10
    )
    if result.returncode != 0 or result.stdout.strip() != "true":
        print(f"WARNING: QM container '{QM_CONTAINER}' not running, skipping H2QM tests")
        return True  # Not an error, just skip H2QM tests
    
    # Get QM binary modification time
    result = subprocess.run(
        ["podman", "exec", QM_CONTAINER, "stat", "-c", "%Y", QM_BINARY],
        capture_output=True, text=True, timeout=10
    )
    
    qm_mtime = 0
    if result.returncode == 0:
        try:
            qm_mtime = float(result.stdout.strip())
        except ValueError:
            qm_mtime = 0
    
    # Compare times (use integer seconds since QM stat doesn't report sub-second precision)
    if int(qm_mtime) >= int(host_mtime):
        print(f"QM binary is up-to-date")
        return True
    
    # Need to update
    if qm_mtime == 0:
        print(f"QM binary not found, copying...")
    else:
        from datetime import datetime
        host_time = datetime.fromtimestamp(host_mtime).strftime('%Y-%m-%d %H:%M:%S')
        qm_time = datetime.fromtimestamp(qm_mtime).strftime('%Y-%m-%d %H:%M:%S')
        print(f"QM binary is stale:")
        print(f"  Host binary: {host_time}")
        print(f"  QM binary:   {qm_time}")
        print(f"Updating QM binary...")
    
    # Copy binary to QM
    result = subprocess.run(
        ["podman", "cp", BINARY, f"{QM_CONTAINER}:{QM_BINARY}"],
        capture_output=True, text=True, timeout=30
    )
    if result.returncode != 0:
        print(f"ERROR: Failed to copy binary to QM: {result.stderr}")
        return False
    
    # Make executable
    result = subprocess.run(
        ["podman", "exec", QM_CONTAINER, "chmod", "+x", QM_BINARY],
        capture_output=True, text=True, timeout=10
    )
    if result.returncode != 0:
        print(f"ERROR: Failed to make binary executable: {result.stderr}")
        return False
    
    print(f"QM binary updated successfully")
    return True


class Mode(Enum):
    STANDALONE_ASYNC = "standalone_async"
    STANDALONE_BLOCKING = "standalone_blocking"
    STANDALONE_SHM_DIRECT = "standalone_shm_direct"
    # Note: H2QM, C2C, and QM_C2C only support blocking mode because
    # async client mode is not implemented in the Rust code
    H2QM_BLOCKING = "h2qm_blocking"      # Host-to-QM blocking mode
    C2C_BLOCKING = "c2c_blocking"        # Container-to-Container (generic) blocking mode
    QM_C2C_BLOCKING = "qm_c2c_blocking"  # C2C inside QM partition (nested containers)


@dataclass
class TestConfig:
    mechanism: str
    mode: Mode
    size: int
    test_type: str  # "iter" or "dur"
    
    @property
    def output_name(self) -> str:
        return f"{self.mode.value}_{self.mechanism}_{self.size}_{self.test_type}_summary.json"
    
    @property
    def streaming_name(self) -> str:
        return f"{self.mode.value}_{self.mechanism}_{self.size}_{self.test_type}_streaming.csv"


def run_command(cmd: List[str], timeout: int = 300, cwd: Optional[Path] = None, 
                stream: bool = False, stream_file: Optional[Path] = None) -> subprocess.CompletedProcess:
    """Run a command and return the result.
    
    Args:
        cmd: Command to run as list of strings
        timeout: Timeout in seconds
        cwd: Working directory
        stream: If True, stream stdout/stderr in real-time to console
        stream_file: Deprecated - streaming CSV is now handled via --streaming-output-csv flag
    """
    # Show full command for transparency
    cmd_str = ' '.join(cmd)
    print(f"  Command: {cmd_str}", flush=True)
    try:
        if stream:
            # Stream output in real-time to console
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                cwd=cwd
            )
            stdout_lines = []
            try:
                for line in process.stdout:
                    print(f"    {line.rstrip()}", flush=True)
                    stdout_lines.append(line)
                process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                print(f"  TIMEOUT after {timeout}s")
                return subprocess.CompletedProcess(cmd, 124, "".join(stdout_lines), "Timeout")
            
            return subprocess.CompletedProcess(cmd, process.returncode, "".join(stdout_lines), "")
        else:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout,
                cwd=cwd
            )
            return result
    except subprocess.TimeoutExpired:
        print(f"  TIMEOUT after {timeout}s")
        return subprocess.CompletedProcess(cmd, 124, "", "Timeout")


def run_standalone_test(config: TestConfig, stream_file: Optional[Path] = None) -> bool:
    """Run a standalone test (async, blocking, or shm-direct)."""
    output_file = OUTPUT_DIR / config.output_name
    
    cmd = [
        BINARY,
        "-m", config.mechanism,
        "-s", str(config.size),
        "-w", str(WARMUP),
        "--one-way",
        "-o", str(output_file),
    ]
    
    # Add streaming CSV output if requested
    if stream_file:
        cmd.extend(["--streaming-output-csv", str(stream_file)])
    
    # Async PMQ and async SHM have timing issues with round-trip
    # Only run one-way for these; other modes get both
    skip_roundtrip = (
        config.mode == Mode.STANDALONE_ASYNC and config.mechanism in ["pmq", "shm"]
    )
    if not skip_roundtrip:
        cmd.append("--round-trip")
    
    # Add iteration or duration
    if config.test_type == "iter":
        cmd.extend(["-i", str(ITERATIONS)])
    else:
        cmd.extend(["-d", f"{DURATION_SECS}s"])
    
    # Add mode-specific flags
    if config.mode == Mode.STANDALONE_BLOCKING:
        cmd.append("--blocking")
    elif config.mode == Mode.STANDALONE_SHM_DIRECT:
        cmd.append("--shm-direct")
    # async is the default
    
    result = run_command(cmd, stream=STREAM_OUTPUT)
    
    if result.returncode != 0:
        print(f"  FAILED: {result.stderr[:200] if result.stderr else 'No error output'}")
        return False
    
    # Check if streaming CSV was created
    if stream_file and stream_file.exists():
        print(f"  Streaming CSV: {stream_file.name}", flush=True)
    
    return output_file.exists()


def get_qm_ip() -> Optional[str]:
    """Get the QM container's IP address."""
    try:
        result = subprocess.run(
            ["podman", "inspect", QM_CONTAINER, "--format", 
             "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}"],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except Exception as e:
        print(f"  Error getting QM IP: {e}")
    return None


def run_h2qm_test(config: TestConfig, stream_file: Optional[Path] = None) -> bool:
    """Run a Host-to-QM container test.
    
    Server runs inside QM container, sender runs on host.
    Note: Only blocking mode is supported (async client mode not implemented).
    """
    output_file = OUTPUT_DIR / config.output_name
    
    # Clean up any stale server processes in QM
    subprocess.run(
        ["podman", "exec", QM_CONTAINER, "pkill", "-f", "ipc-benchmark"],
        capture_output=True, timeout=10
    )
    time.sleep(0.5)
    
    # Build common args for both server and sender
    # Always use blocking mode (async client mode not implemented)
    common_args = [
        "-m", config.mechanism,
        "-s", str(config.size),
        "-w", str(WARMUP),
        "--blocking",
    ]
    
    if config.test_type == "iter":
        common_args.extend(["-i", str(ITERATIONS)])
    else:
        common_args.extend(["-d", f"{DURATION_SECS}s"])
    
    # Mechanism-specific configuration
    server_extra = []
    sender_extra = []
    
    if config.mechanism == "tcp":
        qm_ip = get_qm_ip()
        if not qm_ip:
            print("  FAILED: Could not get QM container IP")
            return False
        server_extra = ["--host", "0.0.0.0"]
        sender_extra = ["--host", qm_ip]
    elif config.mechanism == "uds":
        # /var/qm on host maps to /var inside QM
        # Server (QM) sees: /var/tmp/ipc_test/benchmark.sock
        # Sender (host) sees: /var/qm/tmp/ipc_test/benchmark.sock
        socket_dir_host = Path("/var/qm/tmp/ipc_test")
        socket_dir_host.mkdir(parents=True, exist_ok=True)
        socket_dir_host.chmod(0o777)
        socket_name = f"h2qm_{config.size}.sock"
        socket_path_host = socket_dir_host / socket_name
        # Clean up any stale socket
        if socket_path_host.exists():
            socket_path_host.unlink()
        server_extra = ["--socket-path", f"/var/tmp/ipc_test/{socket_name}"]
        sender_extra = ["--socket-path", str(socket_path_host)]
    elif config.mechanism == "shm":
        # Uses bind-mounted /dev/shm
        # Use default shared memory name (ipc_benchmark_shm) - works reliably
        # Clean up any stale shared memory from previous runs
        shm_path = Path("/dev/shm/ipc_benchmark_shm")
        if shm_path.exists():
            shm_path.unlink()
        server_extra = []  # Use default name
        sender_extra = []  # Use default name
    elif config.mechanism == "pmq":
        # Uses --ipc=host in QM container
        mq_name = f"ipc_h2qm_mq_{config.size}"
        server_extra = ["--message-queue-name", mq_name]
        sender_extra = ["--message-queue-name", mq_name]
    
    # Build server command (runs inside QM)
    server_cmd = [
        "podman", "exec", "-d", QM_CONTAINER,
        QM_BINARY,
    ] + common_args + [
        "--run-mode", "client",
        "--log-file", "stderr",
    ] + server_extra
    
    # Start server in QM container
    print(f"  Starting server in QM...")
    result = subprocess.run(server_cmd, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        print(f"  FAILED to start server: {result.stderr[:200] if result.stderr else 'No output'}")
        return False
    
    # Wait for server to be ready
    # SHM needs time for shared memory setup
    # PMQ needs time for message queue creation across container boundary
    if config.mechanism == "shm":
        wait_time = 3
    elif config.mechanism == "pmq":
        wait_time = 4  # PMQ queue creation is slower across QM boundary
    else:
        wait_time = 2
    time.sleep(wait_time)
    
    # Build sender command (runs on host)
    sender_cmd = [
        BINARY,
    ] + common_args + [
        "--run-mode", "sender",
        "--one-way",
        "-o", str(output_file),
    ] + sender_extra
    
    # Add streaming CSV output if requested
    if stream_file:
        sender_cmd.extend(["--streaming-output-csv", str(stream_file)])
    
    # SHM has timing issues with round-trip across host/QM boundary
    if config.mechanism != "shm":
        sender_cmd.append("--round-trip")
    
    # Run sender from host
    print(f"  Running sender from host...")
    result = run_command(sender_cmd, timeout=600, stream=STREAM_OUTPUT)
    
    # Clean up: kill server process in QM
    # Find and kill the ipc-benchmark process
    subprocess.run(
        ["podman", "exec", QM_CONTAINER, "pkill", "-f", "ipc-benchmark"],
        capture_output=True, timeout=10
    )
    
    if result.returncode != 0:
        print(f"  FAILED: {result.stderr[:200] if result.stderr else 'No error output'}")
        return False
    
    # Check if streaming CSV was created
    if stream_file and stream_file.exists():
        print(f"  Streaming CSV: {stream_file.name}", flush=True)
    
    return output_file.exists()


def cleanup_c2c_containers():
    """Stop and remove C2C test containers."""
    for container in [C2C_SERVER_CONTAINER, C2C_SENDER_CONTAINER]:
        subprocess.run(
            ["podman", "rm", "-f", container],
            capture_output=True, timeout=30
        )


def setup_c2c_shared_resources(mechanism: str, size: int) -> Dict[str, str]:
    """Set up shared resources for C2C testing. Returns paths/names for server and sender."""
    C2C_SHARED_DIR.mkdir(parents=True, exist_ok=True)
    C2C_SHARED_DIR.chmod(0o777)
    
    config = {"server": [], "sender": []}
    
    if mechanism == "tcp":
        # Server binds to 0.0.0.0, sender connects to server container
        config["server"] = ["--host", "0.0.0.0"]
        # Sender will need to get server IP after container starts
        config["sender"] = []  # Will be set after server starts
        config["needs_server_ip"] = True
    elif mechanism == "uds":
        socket_name = f"c2c_{size}.sock"
        socket_path = f"/shared/{socket_name}"
        # Clean up stale socket
        host_socket = C2C_SHARED_DIR / socket_name
        if host_socket.exists():
            host_socket.unlink()
        config["server"] = ["--socket-path", socket_path]
        config["sender"] = ["--socket-path", socket_path]
    elif mechanism == "shm":
        # SHM uses default name via --ipc=host sharing
        shm_path = Path("/dev/shm/ipc_benchmark_shm")
        if shm_path.exists():
            shm_path.unlink()
        config["server"] = []
        config["sender"] = []
    elif mechanism == "pmq":
        mq_name = f"c2c_mq_{size}"
        config["server"] = ["--message-queue-name", mq_name]
        config["sender"] = ["--message-queue-name", mq_name]
    
    return config


def get_container_ip(container_name: str) -> Optional[str]:
    """Get a container's IP address."""
    try:
        result = subprocess.run(
            ["podman", "inspect", container_name, "--format",
             "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}"],
            capture_output=True, text=True, timeout=10
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except Exception:
        pass
    return None


def run_c2c_test(config: TestConfig, stream_file: Optional[Path] = None) -> bool:
    """Run a Container-to-Container test.
    
    Server runs in one container, sender in another.
    Note: Only blocking mode is supported (async client mode not implemented).
    """
    output_file = OUTPUT_DIR / config.output_name
    
    # Clean up any existing containers
    cleanup_c2c_containers()
    time.sleep(0.5)
    
    # Set up shared resources
    mech_config = setup_c2c_shared_resources(config.mechanism, config.size)
    
    # Build common args - always blocking (async client mode not implemented)
    common_args = [
        "-m", config.mechanism,
        "-s", str(config.size),
        "-w", str(WARMUP),
        "--blocking",
    ]
    
    if config.test_type == "iter":
        common_args.extend(["-i", str(ITERATIONS)])
    else:
        common_args.extend(["-d", f"{DURATION_SECS}s"])
    
    # Build volume mounts based on mechanism
    # Use :z for SELinux relabeling (required for container access)
    volumes = [f"{RUSTY_COMMS_DIR}/target/release:/app:z"]
    podman_args = ["--network", "host"]
    
    if config.mechanism == "uds":
        volumes.append(f"{C2C_SHARED_DIR}:/shared:z")
    elif config.mechanism in ["shm", "pmq"]:
        podman_args.append("--ipc=host")
    
    volume_args = []
    for v in volumes:
        volume_args.extend(["-v", v])
    
    # Start server container
    server_cmd = [
        "podman", "run", "-d", "--name", C2C_SERVER_CONTAINER,
    ] + podman_args + volume_args + [
        C2C_BASE_IMAGE,
        C2C_CONTAINER_BINARY,
    ] + common_args + [
        "--run-mode", "client",
        "--log-file", "stderr",
    ] + mech_config.get("server", [])
    
    print(f"  Starting server container...")
    result = subprocess.run(server_cmd, capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        print(f"  FAILED to start server container: {result.stderr[:200] if result.stderr else 'No output'}")
        cleanup_c2c_containers()
        return False
    
    # Wait for server to be ready
    time.sleep(3)
    
    # Get server IP if needed (for TCP)
    sender_extra = mech_config.get("sender", [])
    if mech_config.get("needs_server_ip"):
        server_ip = get_container_ip(C2C_SERVER_CONTAINER)
        if not server_ip:
            # Fallback to localhost since we use --network host
            server_ip = "127.0.0.1"
        sender_extra = ["--host", server_ip]
    
    # SHM has timing issues with round-trip across container boundaries
    skip_roundtrip = config.mechanism == "shm"
    
    # Build sender command - runs sender container and waits for it
    sender_cmd = [
        "podman", "run", "--rm", "--name", C2C_SENDER_CONTAINER,
    ] + podman_args + volume_args + [
        "-v", f"{OUTPUT_DIR}:/output:z",
        C2C_BASE_IMAGE,
        C2C_CONTAINER_BINARY,
    ] + common_args + [
        "--run-mode", "sender",
        "--one-way",
        "-o", f"/output/{config.output_name}",
    ] + sender_extra
    
    # Add streaming CSV output if requested (write to mounted /output directory)
    if stream_file:
        sender_cmd.extend(["--streaming-output-csv", f"/output/{config.streaming_name}"])
    
    if not skip_roundtrip:
        sender_cmd.append("--round-trip")
    
    print(f"  Running sender container...")
    result = run_command(sender_cmd, timeout=600, stream=STREAM_OUTPUT)
    
    # Clean up
    cleanup_c2c_containers()
    
    if result.returncode != 0:
        print(f"  FAILED: {result.stderr[:200] if result.stderr else 'No error output'}")
        return False
    
    # Check if streaming CSV was created
    if stream_file and stream_file.exists():
        print(f"  Streaming CSV: {stream_file.name}", flush=True)
    
    return output_file.exists()


def cleanup_qm_c2c_containers():
    """Stop and remove C2C test containers inside QM."""
    for container in ["qm-c2c-server", "qm-c2c-sender"]:
        subprocess.run(
            ["podman", "exec", QM_CONTAINER, "podman", "rm", "-f", container],
            capture_output=True, timeout=30
        )


def run_qm_c2c_test(config: TestConfig, stream_file: Optional[Path] = None) -> bool:
    """Run a Container-to-Container test inside the QM partition.
    
    Both server and sender run as nested containers inside QM.
    """
    output_file = OUTPUT_DIR / config.output_name
    
    # Clean up any existing containers inside QM
    cleanup_qm_c2c_containers()
    # Also kill any stale direct processes
    subprocess.run(
        ["podman", "exec", QM_CONTAINER, "pkill", "-f", "ipc-benchmark"],
        capture_output=True, timeout=10
    )
    time.sleep(0.5)
    
    # Build common args
    common_args = [
        "-m", config.mechanism,
        "-s", str(config.size),
        "-w", str(WARMUP),
        "--blocking",
    ]
    
    if config.test_type == "iter":
        common_args.extend(["-i", str(ITERATIONS)])
    else:
        common_args.extend(["-d", f"{DURATION_SECS}s"])
    
    # Mechanism-specific configuration
    server_extra = []
    sender_extra = []
    
    if config.mechanism == "tcp":
        server_extra = ["--host", "0.0.0.0"]
        sender_extra = ["--host", "127.0.0.1"]  # localhost inside QM
    elif config.mechanism == "uds":
        socket_path = f"/tmp/qm_c2c_{config.size}.sock"
        # Clean up stale socket
        subprocess.run(
            ["podman", "exec", QM_CONTAINER, "rm", "-f", socket_path],
            capture_output=True, timeout=10
        )
        server_extra = ["--socket-path", socket_path]
        sender_extra = ["--socket-path", socket_path]
    elif config.mechanism == "shm":
        # Clean up stale SHM
        subprocess.run(
            ["podman", "exec", QM_CONTAINER, "rm", "-f", "/dev/shm/ipc_benchmark_shm"],
            capture_output=True, timeout=10
        )
        server_extra = []
        sender_extra = []
    elif config.mechanism == "pmq":
        mq_name = f"qm_c2c_mq_{config.size}"
        server_extra = ["--message-queue-name", mq_name]
        sender_extra = ["--message-queue-name", mq_name]
    
    # Start server directly inside QM (not nested container - simpler)
    server_cmd = [
        "podman", "exec", "-d", QM_CONTAINER,
        QM_BINARY,
    ] + common_args + [
        "--run-mode", "client",
        "--log-file", "stderr",
    ] + server_extra
    
    print(f"  Starting server in QM...")
    result = subprocess.run(server_cmd, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        print(f"  FAILED to start server: {result.stderr[:200] if result.stderr else 'No output'}")
        return False
    
    # Wait for server to be ready
    time.sleep(3)
    
    # Run sender inside QM - output to a temp file inside QM, then copy out
    qm_output_path = f"/tmp/{config.output_name}"
    qm_streaming_path = f"/tmp/{config.streaming_name}" if stream_file else None
    
    sender_cmd = [
        "podman", "exec", QM_CONTAINER,
        QM_BINARY,
    ] + common_args + [
        "--run-mode", "sender",
        "--one-way",
        "--log-file", "stderr",
        "-o", qm_output_path,
    ] + sender_extra
    
    # Add streaming CSV output if requested
    if stream_file:
        sender_cmd.extend(["--streaming-output-csv", qm_streaming_path])
    
    # SHM has timing issues with round-trip
    if config.mechanism != "shm":
        sender_cmd.append("--round-trip")
    
    print(f"  Running sender in QM...")
    result = run_command(sender_cmd, timeout=600, stream=STREAM_OUTPUT)
    
    # Copy output file from QM to host
    if result.returncode == 0:
        copy_result = subprocess.run(
            ["podman", "cp", f"{QM_CONTAINER}:{qm_output_path}", str(output_file)],
            capture_output=True, timeout=30
        )
        if copy_result.returncode != 0:
            print(f"  FAILED to copy output file")
        
        # Copy streaming CSV from QM to host if requested
        if stream_file and qm_streaming_path:
            copy_result = subprocess.run(
                ["podman", "cp", f"{QM_CONTAINER}:{qm_streaming_path}", str(stream_file)],
                capture_output=True, timeout=30
            )
            if copy_result.returncode == 0:
                print(f"  Streaming CSV: {stream_file.name}", flush=True)
    
    # Clean up: kill server process in QM
    subprocess.run(
        ["podman", "exec", QM_CONTAINER, "pkill", "-f", "ipc-benchmark"],
        capture_output=True, timeout=10
    )
    # Clean up temp files in QM
    subprocess.run(
        ["podman", "exec", QM_CONTAINER, "rm", "-f", qm_output_path],
        capture_output=True, timeout=10
    )
    if qm_streaming_path:
        subprocess.run(
            ["podman", "exec", QM_CONTAINER, "rm", "-f", qm_streaming_path],
            capture_output=True, timeout=10
        )
    
    if result.returncode != 0:
        print(f"  FAILED: {result.stderr[:200] if result.stderr else 'No error output'}")
        return False
    
    return output_file.exists()


def run_test(config: TestConfig, stream_file: Optional[Path] = None) -> bool:
    """Run a single test based on configuration."""
    print(f"[{config.mode.value}] {config.mechanism} size={config.size} type={config.test_type}", flush=True)
    
    if config.mode in [Mode.STANDALONE_ASYNC, Mode.STANDALONE_BLOCKING, Mode.STANDALONE_SHM_DIRECT]:
        return run_standalone_test(config, stream_file)
    elif config.mode == Mode.H2QM_BLOCKING:
        return run_h2qm_test(config, stream_file)
    elif config.mode == Mode.C2C_BLOCKING:
        return run_c2c_test(config, stream_file)
    elif config.mode == Mode.QM_C2C_BLOCKING:
        return run_qm_c2c_test(config, stream_file)
    
    return False


def get_sizes_for_mechanism(mechanism: str, mode: Mode) -> List[int]:
    """Get appropriate message sizes for a mechanism and mode.
    
    Args:
        mechanism: The IPC mechanism (tcp, uds, shm, pmq)
        mode: The test mode
    """
    if mechanism == "pmq":
        return PMQ_SIZES
    
    # SHM-direct has 8KB payload limit - exclude 65536
    if mode == Mode.STANDALONE_SHM_DIRECT:
        return [s for s in GENERAL_SIZES if s <= 8192]
    
    return GENERAL_SIZES


def get_modes_for_mechanism(mechanism: str, include_h2qm: bool = True, include_c2c: bool = False, include_qm_c2c: bool = False) -> List[Mode]:
    """Get applicable modes for a mechanism."""
    modes = [
        Mode.STANDALONE_ASYNC,
        Mode.STANDALONE_BLOCKING,
    ]
    
    # SHM-direct only applies to SHM
    if mechanism == "shm":
        modes.append(Mode.STANDALONE_SHM_DIRECT)
    
    # H2QM mode for all mechanisms (blocking only - async client mode not implemented)
    # - UDS: Uses /var/qm (host) -> /var (QM) bind mount
    # - TCP: Uses QM container's IP address
    # - PMQ: Uses --ipc=host for shared IPC namespace
    # - SHM: Uses bind-mounted /dev/shm
    if include_h2qm:
        modes.append(Mode.H2QM_BLOCKING)
    
    # C2C mode for all mechanisms (blocking only - async client mode not implemented)
    if include_c2c:
        modes.append(Mode.C2C_BLOCKING)
    
    # QM C2C mode - both processes run inside QM partition
    if include_qm_c2c:
        modes.append(Mode.QM_C2C_BLOCKING)
    
    return modes


def check_qm_ready() -> bool:
    """Check if QM container is running and has the benchmark binary."""
    # Check if QM container is running
    result = subprocess.run(
        ["podman", "ps", "-q", "-f", f"name={QM_CONTAINER}"],
        capture_output=True, text=True, timeout=10
    )
    if not result.stdout.strip():
        print(f"ERROR: QM container '{QM_CONTAINER}' is not running")
        return False
    
    # Check if binary exists in QM
    result = subprocess.run(
        ["podman", "exec", QM_CONTAINER, "test", "-x", QM_BINARY],
        capture_output=True, timeout=10
    )
    if result.returncode != 0:
        print(f"ERROR: Binary not found in QM at {QM_BINARY}")
        print(f"Run: podman cp {BINARY} {QM_CONTAINER}:{QM_BINARY}")
        return False
    
    return True


def generate_csv_summary():
    """Generate CSV summary from JSON output files."""
    import csv
    csv_file = OUTPUT_DIR / "benchmark_results.csv"
    
    # Find all summary JSON files
    json_files = sorted(OUTPUT_DIR.glob("*_summary.json"))
    if not json_files:
        print("No summary JSON files found")
        return None
    
    rows = []
    for jf in json_files:
        try:
            with open(jf) as f:
                data = json.load(f)
            # Extract key metrics
            row = {
                "file": jf.name,
                "mechanism": data.get("config", {}).get("mechanism", ""),
                "size": data.get("config", {}).get("message_size", ""),
                "iterations": data.get("config", {}).get("iterations", ""),
                "one_way_mean_ns": data.get("one_way_latency", {}).get("mean_ns", ""),
                "round_trip_mean_ns": data.get("round_trip_latency", {}).get("mean_ns", ""),
                "throughput_mbps": data.get("throughput", {}).get("mbps", ""),
            }
            rows.append(row)
        except Exception as e:
            print(f"  Warning: Could not process {jf.name}: {e}")
    
    if not rows:
        return None
    
    # Write CSV
    fieldnames = ["file", "mechanism", "size", "iterations", "one_way_mean_ns", "round_trip_mean_ns", "throughput_mbps"]
    with open(csv_file, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    
    print(f"CSV written to: {csv_file}")
    return csv_file


def should_stream_test(config: TestConfig, stream_sizes: Optional[List[int]], 
                       stream_iter_only: bool, stream_dur_only: bool) -> bool:
    """Determine if a specific test should have streaming output."""
    # Check size filter
    if stream_sizes and config.size not in stream_sizes:
        return False
    
    # Check test type filter
    if stream_iter_only and config.test_type != "iter":
        return False
    if stream_dur_only and config.test_type != "dur":
        return False
    
    return True


def main():
    """Run all benchmarks."""
    global STREAM_OUTPUT
    
    import argparse
    parser = argparse.ArgumentParser(
        description="IPC Benchmark Suite Runner - Always runs ALL tests",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
All tests always run. The --stream option enables real-time output for 
selected tests based on filter criteria.

Examples:
  %(prog)s                                   # Run all tests (quiet mode)
  %(prog)s --output both                     # Run all, generate JSON + CSV
  %(prog)s --stream                          # Run all, stream ALL output
  %(prog)s --stream --sizes 1024,4096        # Run all, stream only 1024/4096 tests
  %(prog)s --stream --iter-only              # Run all, stream only iteration tests
  %(prog)s --stream --sizes 4096 --dur-only  # Run all, stream only 4096 duration tests
"""
    )
    
    # Mode selection - controls which test categories to include
    mode_group = parser.add_argument_group("Mode Selection (which tests to run)")
    mode_group.add_argument("--standalone-only", action="store_true",
                           help="Only run standalone tests (skip H2QM)")
    mode_group.add_argument("--h2qm-only", action="store_true",
                           help="Only run Host-to-QM tests")
    mode_group.add_argument("--c2c-only", action="store_true",
                           help="Only run Container-to-Container tests (generic)")
    mode_group.add_argument("--include-c2c", action="store_true",
                           help="(deprecated) C2C tests are now included by default")
    mode_group.add_argument("--qm-c2c-only", action="store_true",
                           help="Only run C2C tests inside QM partition")
    mode_group.add_argument("--include-qm-c2c", action="store_true",
                           help="Include C2C tests inside QM partition")
    mode_group.add_argument("--blocking-only", action="store_true",
                           help="Only run blocking mode tests")
    mode_group.add_argument("--async-only", action="store_true",
                           help="Only run async mode tests")
    
    # Streaming options - control which tests get verbose output
    stream_group = parser.add_argument_group("Streaming Options (which tests show real-time output)")
    stream_group.add_argument("--stream", action="store_true",
                             help="Enable streaming output (real-time, unbuffered)")
    stream_group.add_argument("--sizes", type=str, default=None,
                             help="Stream only these sizes (e.g., '1024,4096'). Default: all")
    stream_group.add_argument("--iter-only", action="store_true",
                             help="Stream only iteration-based tests")
    stream_group.add_argument("--dur-only", action="store_true",
                             help="Stream only duration-based tests")
    
    # Output format
    output_group = parser.add_argument_group("Output Format")
    output_group.add_argument("--output", choices=["json", "csv", "both"], default="json",
                             help="Output format: json (default), csv, or both")
    
    args = parser.parse_args()
    
    # Parse stream size filter
    stream_sizes = None
    if args.sizes:
        try:
            stream_sizes = [int(s.strip()) for s in args.sizes.split(",")]
        except ValueError:
            print(f"ERROR: Invalid size format: {args.sizes}")
            sys.exit(1)
    
    # Determine which test categories to include
    include_standalone = not (args.h2qm_only or args.c2c_only or args.qm_c2c_only)
    include_h2qm = not (args.standalone_only or args.c2c_only or args.qm_c2c_only)
    # C2C is now included by default (unless running a specific mode only)
    include_c2c = args.c2c_only or (not args.standalone_only and not args.h2qm_only and not args.qm_c2c_only)
    include_qm_c2c = args.qm_c2c_only or args.include_qm_c2c
    
    # Check host binary exists
    if not Path(BINARY).exists():
        print(f"ERROR: Binary not found: {BINARY}")
        print("  Run: cargo build --release")
        sys.exit(1)
    
    # Check SELinux status (important for H2QM PMQ and C2C tests)
    check_selinux_for_tests()
    
    # Check and update QM binary if H2QM or QM_C2C tests are included
    if include_h2qm or include_qm_c2c:
        if not check_and_update_qm_binary():
            print("ERROR: Failed to prepare QM binary")
            sys.exit(1)
    
    print("=" * 70)
    print("IPC Benchmark Full Suite")
    print(f"Output directory: {OUTPUT_DIR}")
    print(f"Binary: {BINARY}")
    print(f"Iterations: {ITERATIONS}, Duration: {DURATION_SECS}s")
    if args.standalone_only:
        print("Mode: Standalone only")
    elif args.h2qm_only:
        print("Mode: Host-to-QM only")
    elif args.c2c_only:
        print("Mode: Container-to-Container (generic) only")
    elif args.qm_c2c_only:
        print("Mode: C2C inside QM partition only")
    else:
        modes_list = ["Standalone", "H2QM", "C2C"]  # C2C now included by default
        if args.include_qm_c2c:
            modes_list.append("QM-C2C")
        print(f"Mode: {' + '.join(modes_list)}")
    
    # Show streaming settings
    if args.stream:
        stream_info = "Streaming: enabled"
        if stream_sizes:
            stream_info += f", sizes={stream_sizes}"
        if args.iter_only:
            stream_info += ", iter-only"
        if args.dur_only:
            stream_info += ", dur-only"
        print(stream_info)
    else:
        print("Streaming: disabled (quiet mode)")
    print(f"Output format: {args.output.upper()}")
    print("=" * 70)
    
    # Ensure output directory exists
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    
    # Verify binary exists
    if not os.path.exists(BINARY):
        print(f"ERROR: Binary not found: {BINARY}")
        sys.exit(1)
    
    # Check QM readiness if running H2QM tests
    if include_h2qm:
        if not check_qm_ready():
            if args.h2qm_only:
                sys.exit(1)
            else:
                print("WARNING: QM not ready, skipping H2QM tests")
                include_h2qm = False
    
    mechanisms = ["uds", "tcp", "shm", "pmq"]
    test_types = ["iter", "dur"]  # Always run both
    
    results = {"passed": 0, "failed": 0, "skipped": 0}
    failed_tests = []
    
    # Run iteration tests first, then duration tests
    for test_type in test_types:
        print(f"\n{'='*70}")
        print(f"Running {'ITERATION' if test_type == 'iter' else 'DURATION'} tests")
        print(f"{'='*70}")
        
        for mechanism in mechanisms:
            modes = get_modes_for_mechanism(mechanism, include_h2qm, include_c2c, include_qm_c2c)
            
            # Filter modes based on command line args
            if args.standalone_only:
                modes = [m for m in modes if m.value.startswith("standalone")]
            elif args.h2qm_only:
                modes = [m for m in modes if m.value.startswith("h2qm")]
            elif args.c2c_only:
                modes = [m for m in modes if m.value.startswith("c2c_")]
            elif args.qm_c2c_only:
                modes = [m for m in modes if m.value.startswith("qm_c2c")]
            
            # Filter by async/blocking preference
            if args.blocking_only:
                modes = [m for m in modes if "blocking" in m.value or "shm_direct" in m.value]
            elif args.async_only:
                modes = [m for m in modes if "async" in m.value]
            
            for mode in modes:
                sizes = get_sizes_for_mechanism(mechanism, mode)  # Always all sizes
                for size in sizes:
                    config = TestConfig(
                        mechanism=mechanism,
                        mode=mode,
                        size=size,
                        test_type=test_type
                    )
                    
                    # Determine if this specific test should stream
                    global STREAM_OUTPUT
                    use_stream = args.stream and should_stream_test(
                        config, stream_sizes, args.iter_only, args.dur_only
                    )
                    STREAM_OUTPUT = use_stream
                    
                    # Set up streaming file if streaming is enabled
                    stream_file = OUTPUT_DIR / config.streaming_name if use_stream else None
                    
                    try:
                        success = run_test(config, stream_file)
                        if success:
                            print(f"  ✓ PASSED")
                            results["passed"] += 1
                        else:
                            print(f"  ✗ FAILED")
                            results["failed"] += 1
                            failed_tests.append(config.output_name)
                    except Exception as e:
                        print(f"  ✗ ERROR: {e}")
                        results["failed"] += 1
                        failed_tests.append(config.output_name)
    
    # Print summary
    print("\n" + "=" * 70)
    print("SUMMARY")
    print("=" * 70)
    print(f"Passed:  {results['passed']}")
    print(f"Failed:  {results['failed']}")
    
    if failed_tests:
        print("\nFailed tests:")
        for t in failed_tests:
            print(f"  - {t}")
    
    print(f"\nOutput files in: {OUTPUT_DIR}")
    
    # Handle output format
    if args.output in ["csv", "both"]:
        print("\nGenerating CSV summary...")
        # Use the external generate_summary_csv.py for comprehensive parsing
        csv_script = Path(__file__).parent / "generate_summary_csv.py"
        if csv_script.exists():
            result = subprocess.run(
                [sys.executable, str(csv_script)],
                capture_output=True, text=True, timeout=60
            )
            if result.returncode == 0:
                print(result.stdout)
            else:
                print(f"CSV generation failed: {result.stderr}")
        else:
            # Fallback to built-in CSV generation
            generate_csv_summary()
        
        if args.output == "csv":
            print("\nCSV summary generated (JSON files preserved)")
    else:
        print("\nRun generate_summary_csv.py to create consolidated CSV")
    
    return 0 if results["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
