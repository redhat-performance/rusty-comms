#!/usr/bin/env python3
"""
Container-to-Container IPC Benchmark Orchestrator

This script launches two containers and benchmarks IPC performance between them.
Container A runs as "client" mode (IPC server/receiver).
Container B runs as "sender" mode (IPC client/sender).

Supported IPC mechanisms:
- SharedMemory (shm): Requires shared /dev/shm mount
- SharedMemory_Direct (shm --shm-direct): Requires shared /dev/shm mount  
- TCP: Uses container networking (host network mode)
- UDS: Requires shared volume for socket file

Usage:
    python3 run_container_to_container_tests.py [--mechanism shm|tcp|uds] [--all]

AI-assisted-by: Claude Opus 4.5
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path


# Default Configuration (can be overridden via environment variables)
IMAGE_NAME = os.environ.get("C2C_IMAGE", "localhost/ipc-benchmark:latest")
SHARED_SHM_DIR = os.environ.get("C2C_SHM_DIR", "/tmp/c2c_shm")
SHARED_UDS_DIR = os.environ.get("C2C_UDS_DIR", "/tmp/c2c_uds")
SHARED_OUTPUT_DIR = os.environ.get("C2C_OUTPUT_DIR", "/tmp/c2c_output")
CONTAINER_PREFIX = os.environ.get("C2C_PREFIX", "ipc-c2c")

# Default test parameters
DEFAULT_ITERATIONS = 10000
DEFAULT_SIZES = [64, 512, 1024, 4096, 8192, 65536]


def run_cmd(cmd, check=True, capture=True, timeout=None):
    """Run a command and return output."""
    print(f"  $ {' '.join(cmd)}")
    result = subprocess.run(
        cmd,
        capture_output=capture,
        text=True,
        check=check,
        timeout=timeout
    )
    return result


def setup_shared_directories():
    """Create shared directories for IPC."""
    for d in [SHARED_SHM_DIR, SHARED_UDS_DIR, SHARED_OUTPUT_DIR]:
        os.makedirs(d, exist_ok=True)
        os.chmod(d, 0o1777)  # World writable with sticky bit
    print("✓ Created shared directories")


def cleanup_containers():
    """Stop and remove any existing benchmark containers."""
    for suffix in ["server", "sender"]:
        name = f"{CONTAINER_PREFIX}-{suffix}"
        subprocess.run(
            ["podman", "rm", "-f", name],
            capture_output=True,
            check=False
        )


def get_container_mounts(mechanism):
    """Get volume mounts based on mechanism."""
    mounts = [
        f"{SHARED_OUTPUT_DIR}:/app/output:z"
    ]
    
    if mechanism in ["shm", "shm-direct"]:
        # Share /dev/shm between containers
        mounts.append(f"{SHARED_SHM_DIR}:/dev/shm:z")
    
    if mechanism == "uds":
        # Share directory for Unix domain socket
        mounts.append(f"{SHARED_UDS_DIR}:/tmp/rusty-comms:z")
    
    # PMQ uses host IPC namespace, no additional mounts needed
    
    return mounts


def get_container_ipc_mode(mechanism):
    """Get IPC mode based on mechanism."""
    # PMQ requires host IPC namespace to share message queues
    if mechanism == "pmq":
        return "host"
    return None  # Use container's own IPC namespace


def launch_server_container(mechanism, size, iterations, shm_direct=False):
    """
    Launch the server (receiver) container.
    
    Uses --run-mode client which starts the IPC server and waits for connections.
    """
    name = f"{CONTAINER_PREFIX}-server"
    mounts = get_container_mounts(mechanism)
    ipc_mode = get_container_ipc_mode(mechanism)
    
    # Build the command
    cmd = [
        "podman", "run", "-d",
        "--name", name,
        "--network", "host",  # Use host network for TCP
    ]
    
    # Add IPC mode if needed (for PMQ)
    if ipc_mode:
        cmd.extend(["--ipc", ipc_mode])
    
    # Add volume mounts
    for mount in mounts:
        cmd.extend(["-v", mount])
    
    # Add the image and benchmark command
    cmd.append(IMAGE_NAME)
    
    # Server uses --run-mode client (acts as IPC server/receiver)
    mech = mechanism.replace("-direct", "")
    cmd.extend([
        "-m", mech,
        "--run-mode", "client",
        "--blocking",
        "-s", str(size),
    ])
    
    # For UDS, specify socket path in shared volume
    if mechanism == "uds":
        cmd.extend(["--socket-path", "/tmp/rusty-comms/c2c.sock"])
    
    if shm_direct:
        cmd.append("--shm-direct")
    
    result = run_cmd(cmd)
    container_id = result.stdout.strip()
    print(f"✓ Server container started: {container_id[:12]}")
    return container_id


def launch_sender_container(
    mechanism, size, iterations, shm_direct=False, duration=None, streaming=False
):
    """
    Launch the sender container.
    
    Uses --run-mode sender which connects to server and runs benchmark.
    """
    name = f"{CONTAINER_PREFIX}-sender"
    mounts = get_container_mounts(mechanism)
    ipc_mode = get_container_ipc_mode(mechanism)
    
    # Build the command
    cmd = [
        "podman", "run", "-d",
        "--name", name,
        "--network", "host",
    ]
    
    # Add IPC mode if needed (for PMQ)
    if ipc_mode:
        cmd.extend(["--ipc", ipc_mode])
    
    # Add volume mounts
    for mount in mounts:
        cmd.extend(["-v", mount])
    
    cmd.append(IMAGE_NAME)
    
    # Sender uses --run-mode sender (acts as IPC client/sender)
    mech = mechanism.replace("-direct", "")
    output_suffix = "_shmdirect" if shm_direct else ""
    output_file = f"/app/output/{mech}_{size}{output_suffix}.json"
    
    # Include test mode in filename for distinction
    mode_suffix = "_dur" if duration else "_iter"
    
    # Summary output file (aggregated results) - c2c prefix for container-to-container
    summary_file = f"/app/output/c2c_{mech}_{size}{output_suffix}{mode_suffix}_summary.json"
    
    cmd.extend([
        "-m", mech,
        "--run-mode", "sender",
        "--blocking",
        "-s", str(size),
        "--output-file", summary_file,
        "--log-file", "stderr",
        "--quiet",
    ])
    
    # Add streaming output if requested (generates large per-message data files)
    if streaming:
        streaming_file = f"/app/output/c2c_{mech}_{size}{output_suffix}{mode_suffix}_streaming.json"
        cmd.extend(["--streaming-output-json", streaming_file])
    
    if duration:
        cmd.extend(["--duration", duration])
    else:
        cmd.extend(["-i", str(iterations)])
    
    # For SHM, use one-way mode (ring buffer doesn't support round-trip well)
    # TCP and UDS can do round-trip
    if mechanism in ["shm", "shm-direct"]:
        cmd.append("--one-way")
    
    # For UDS, specify socket path in shared volume
    if mechanism == "uds":
        cmd.extend(["--socket-path", "/tmp/rusty-comms/c2c.sock"])
    
    if shm_direct:
        cmd.append("--shm-direct")
    
    result = run_cmd(cmd)
    container_id = result.stdout.strip()
    print(f"✓ Sender container started: {container_id[:12]}")
    return container_id


def wait_for_server_ready(container_id, timeout=30):
    """Wait for server container to print SERVER_READY."""
    start = time.time()
    while time.time() - start < timeout:
        result = subprocess.run(
            ["podman", "logs", container_id],
            capture_output=True,
            text=True,
            check=False
        )
        if "SERVER_READY" in result.stdout:
            return True
        time.sleep(0.5)
    return False


def wait_for_container(container_id, timeout=120):
    """Wait for a container to complete."""
    start = time.time()
    while time.time() - start < timeout:
        result = subprocess.run(
            ["podman", "inspect", "-f", "{{.State.Status}}", container_id],
            capture_output=True,
            text=True,
            check=False
        )
        status = result.stdout.strip()
        if status == "exited":
            # Get exit code
            result = subprocess.run(
                ["podman", "inspect", "-f", "{{.State.ExitCode}}", container_id],
                capture_output=True,
                text=True,
                check=False
            )
            exit_code = int(result.stdout.strip())
            return exit_code
        time.sleep(1)
    
    return -1  # Timeout


def get_container_logs(container_id, tail=50):
    """Get logs from a container."""
    result = subprocess.run(
        ["podman", "logs", "--tail", str(tail), container_id],
        capture_output=True,
        text=True,
        check=False
    )
    return result.stdout + result.stderr


def run_benchmark(mechanism, size, iterations, shm_direct=False, duration=None, streaming=False):
    """Run a single benchmark test."""
    mech_name = f"{mechanism}-direct" if shm_direct else mechanism
    print(f"\n{'='*60}")
    print(f"  Benchmarking: {mech_name} size={size} iter={iterations}")
    print(f"{'='*60}")
    
    cleanup_containers()
    
    try:
        # Launch server first
        server_id = launch_server_container(
            mechanism, size, iterations, shm_direct
        )
        
        # Wait for server to be ready
        if not wait_for_server_ready(server_id):
            print("✗ Server failed to start")
            print(get_container_logs(server_id))
            return False
        
        print("✓ Server is ready")
        
        # Launch sender
        sender_id = launch_sender_container(
            mechanism, size, iterations, shm_direct, duration, streaming
        )
        
        # Wait for sender to complete
        print("Waiting for benchmark to complete...")
        sender_exit = wait_for_container(sender_id, timeout=180)
        
        # Server should exit after receiving shutdown message
        server_exit = wait_for_container(server_id, timeout=10)
        
        if sender_exit == 0:
            print(f"✓ Benchmark completed successfully")
            return True
        else:
            print(f"✗ Benchmark failed (sender={sender_exit}, server={server_exit})")
            print("\n--- Server Logs ---")
            print(get_container_logs(server_id, 100))
            print("\n--- Sender Logs ---")
            print(get_container_logs(sender_id, 100))
            return False
            
    except Exception as e:
        print(f"✗ Error: {e}")
        return False
    finally:
        cleanup_containers()


def main():
    global SHARED_OUTPUT_DIR  # Allow override via -o flag
    
    parser = argparse.ArgumentParser(
        description="Container-to-Container IPC Benchmark"
    )
    parser.add_argument(
        "-m", "--mechanism",
        choices=["shm", "shm-direct", "tcp", "uds", "pmq"],
        default="tcp",
        help="IPC mechanism to test"
    )
    parser.add_argument(
        "-s", "--sizes",
        default=",".join(str(s) for s in DEFAULT_SIZES),
        help="Comma-separated message sizes to test"
    )
    parser.add_argument(
        "-i", "--iterations",
        type=int,
        default=DEFAULT_ITERATIONS,
        help="Number of messages per test"
    )
    parser.add_argument(
        "--duration",
        help="Duration for each test (e.g., '10s')"
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Run all mechanisms"
    )
    parser.add_argument(
        "-o", "--output-dir",
        default=None,
        help=f"Output directory for results (default: {SHARED_OUTPUT_DIR})"
    )
    parser.add_argument(
        "--streaming",
        action="store_true",
        help="Enable per-message streaming output (generates large files)"
    )
    
    args = parser.parse_args()
    
    # Override output directory if specified
    if args.output_dir:
        SHARED_OUTPUT_DIR = args.output_dir
    
    print("="*60)
    print("  Container-to-Container IPC Benchmark")
    print("="*60)
    print(f"  Timestamp: {datetime.now().isoformat()}")
    print(f"  Image: {IMAGE_NAME}")
    print()
    
    # Setup
    setup_shared_directories()
    cleanup_containers()
    
    sizes = [int(s) for s in args.sizes.split(",")]
    
    if args.all:
        mechanisms = ["tcp", "uds", "shm", "shm-direct", "pmq"]
    else:
        mechanisms = [args.mechanism]
    
    results = []
    
    for mechanism in mechanisms:
        shm_direct = mechanism == "shm-direct"
        mech = "shm" if shm_direct else mechanism
        
        for size in sizes:
            success = run_benchmark(
                mech, size, args.iterations, shm_direct, args.duration, args.streaming
            )
            results.append({
                "mechanism": mechanism,
                "size": size,
                "success": success
            })
    
    # Summary
    print("\n" + "="*60)
    print("  SUMMARY")
    print("="*60)
    for r in results:
        status = "✓" if r["success"] else "✗"
        print(f"  {status} {r['mechanism']:15} size={r['size']:>6}")
    
    # Show only files created in this run
    print("\n  Output files created:")
    for r in results:
        if r["success"]:
            mech = r["mechanism"].replace("-direct", "")
            suffix = "_shmdirect" if "direct" in r["mechanism"] else ""
            mode_suffix = "_dur" if args.duration else "_iter"
            fname = f"c2c_{mech}_{r['size']}{suffix}{mode_suffix}_summary.json"
            fpath = Path(SHARED_OUTPUT_DIR) / fname
            if fpath.exists():
                size_kb = fpath.stat().st_size / 1024
                print(f"    - {fname} ({size_kb:.1f} KB)")
    
    # Return non-zero if any test failed
    if not all(r["success"] for r in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
