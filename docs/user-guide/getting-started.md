# Getting Started

This guide will help you install, build, and run your first IPC benchmark with rusty-comms.

## What is rusty-comms?

Rusty-comms is a comprehensive **Inter-Process Communication (IPC) benchmark suite** written in Rust. It measures the performance of different IPC mechanisms, helping you:

- Compare latency and throughput across IPC types
- Evaluate async vs blocking I/O performance
- Generate data for performance analysis
- Make informed decisions about IPC mechanism selection

## System Requirements

### Minimum Requirements

| Requirement | Specification |
|-------------|---------------|
| **Operating System** | Linux (tested on RHEL 9.6) |
| **Rust Version** | 1.70.0 or later (MSRV) |
| **CPU** | x86_64 architecture |
| **RAM** | 4GB minimum |
| **Disk** | 1GB free space |

### Platform Notes

- **Unix Domain Sockets (UDS)**: Available on all Unix-like systems
- **POSIX Message Queues (PMQ)**: Linux only, requires `/dev/mqueue` mounted
- **Shared Memory**: Ring-buffer mode is intended to be cross-platform, but rusty-comms
  is primarily tested on Linux; direct mode (`--shm-direct`) is Unix-only
- **TCP Sockets**: Cross-platform

## Installation

### Step 1: Clone the Repository

```bash
git clone https://github.com/redhat-performance/rusty-comms.git
cd rusty-comms
```

### Step 2: Install Rust

If Rust is not already installed, use [rustup](https://rustup.rs/) -- the
official Rust toolchain installer:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the on-screen prompts (the defaults are fine for most users). Once
the installer finishes, load the environment into your current shell:

```bash
source "$HOME/.cargo/env"
```

Verify the installation and confirm the version meets the MSRV (1.70+):

```bash
rustc --version
cargo --version
```

> **Tip:** If you already have Rust installed but need to update, run
> `rustup update stable`.

### Step 3: Build the Project

```bash
# Release build (recommended for benchmarking)
cargo build --release

# The binary will be at:
# target/release/ipc-benchmark
```

### Step 4: Verify the Installation

```bash
./target/release/ipc-benchmark --version
./target/release/ipc-benchmark --help
```

You should see version information and the help text with all available options.

## Your First Benchmark

### Quick Test

Run a simple benchmark using Unix Domain Sockets:

```bash
./target/release/ipc-benchmark -m uds -i 1000
```

This command:
- Uses Unix Domain Sockets (`-m uds`)
- Sends 1,000 messages (`-i 1000`)
- Runs both one-way and round-trip tests (default)
- Prints results to the console

### Expected Output

You should see output similar to:

```
Benchmark Configuration:
-----------------------------------------------------------------
  Mechanisms:         Unix Domain Socket
  Message Size:       1024 bytes
  Iterations:         1000
  Warmup Iterations:  1000
  Test Types:         One-Way, Round-Trip
-----------------------------------------------------------------

Running Unix Domain Socket benchmark...

Benchmark Results:
-----------------------------------------------------------------
Mechanism: Unix Domain Socket
  Message Size: 1024 bytes
  One-Way Latency:
      Mean: 3.15 us, P95: 5.21 us, P99: 8.43 us
      Min:  1.50 us, Max: 45.12 us
  Round-Trip Latency:
      Mean: 5.82 us, P95: 9.11 us, P99: 14.50 us
      Min:  4.20 us, Max: 88.30 us
  Throughput:
      Average: 155.30 MB/s, Peak: 156.40 MB/s
-----------------------------------------------------------------

Benchmark Results:
-----------------------------------------------------------------
  Output Files Written:
    Log File:             ipc_benchmark.log.2026-02-16

```

### Save Results to a File

To save results for later analysis:

```bash
./target/release/ipc-benchmark -m uds -i 10000 -o results.json
```

This creates `results.json` with detailed benchmark data.

### Enable Streaming Output

For real-time data capture:

```bash
./target/release/ipc-benchmark -m uds -i 10000 \
  -o results.json \
  --streaming-output-json streaming.json
```

This creates both summary and per-message latency data.

## Test All Mechanisms

To benchmark all supported IPC mechanisms:

```bash
./target/release/ipc-benchmark -m all -i 10000 -o comparison.json
```

This runs tests for:
- Unix Domain Sockets (uds)
- Shared Memory (shm)
- TCP Sockets (tcp)
- POSIX Message Queues (pmq)

## Common First-Time Issues

### "Could not resolve 'ipc-benchmark' binary"

**Solution**: Build the project first:
```bash
cargo build --release
```

### Permission Errors

**Solution**: Ensure you have write permissions to the current directory and `/tmp`:
```bash
ls -la /tmp/ipc_benchmark_*  # Check for stale files
rm -f /tmp/ipc_benchmark_*   # Clean up if needed
```

### PMQ Not Available

**Solution**: POSIX Message Queues require Linux with `/dev/mqueue` mounted:
```bash
# Check if mounted
mount | grep mqueue

# Mount if needed (requires root)
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue
```

## Next Steps

Now that you've run your first benchmark:

1. **[Basic Usage](basic-usage.md)** - Learn common command patterns
2. **[IPC Mechanisms](ipc-mechanisms.md)** - Understand each mechanism
3. **[Output Formats](output-formats.md)** - Work with results data
4. **[Performance Tuning](performance-tuning.md)** - Get accurate measurements

## Quick Reference

| Task | Command |
|------|---------|
| Test UDS | `ipc-benchmark -m uds -i 10000` |
| Test all mechanisms | `ipc-benchmark -m all -i 10000` |
| Save JSON results | `ipc-benchmark -m uds -i 10000 -o results.json` |
| Blocking mode | `ipc-benchmark -m uds -i 10000 --blocking` |
| Duration-based test | `ipc-benchmark -m uds -d 30s` |
| Verbose output | `ipc-benchmark -m uds -i 1000 -v` |
