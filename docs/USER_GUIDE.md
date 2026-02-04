# Rusty-Comms User Guide

A comprehensive guide to using the IPC Benchmark Suite.

---

## Table of Contents

1. [Getting Started](#getting-started)
2. [Basic Usage](#basic-usage)
3. [Advanced Usage](#advanced-usage)
4. [IPC Mechanisms](#ipc-mechanisms)
5. [Output Formats](#output-formats)
6. [Performance Tuning](#performance-tuning)
7. [Troubleshooting](#troubleshooting)

---

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

### Step 2: Build the Project

```bash
# Release build (recommended for benchmarking)
cargo build --release

# The binary will be at:
# target/release/ipc-benchmark
```

### Step 3: Verify the Installation

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

## Quick Reference

| Task | Command |
|------|---------|
| Test UDS | `ipc-benchmark -m uds -i 10000` |
| Test all mechanisms | `ipc-benchmark -m all -i 10000` |
| Save JSON results | `ipc-benchmark -m uds -i 10000 -o results.json` |
| Blocking mode | `ipc-benchmark -m uds -i 10000 --blocking` |
| Duration-based test | `ipc-benchmark -m uds -d 30s` |
| Verbose output | `ipc-benchmark -m uds -i 1000 -v` |

---

# Basic Usage

This section covers the most common command patterns for running IPC benchmarks.

## Command Structure

The basic command structure is:

```bash
ipc-benchmark [OPTIONS]
```

Or with the full path:

```bash
./target/release/ipc-benchmark [OPTIONS]
```

## Selecting IPC Mechanisms

Use the `-m` flag to select which mechanisms to test:

```bash
# Single mechanism
ipc-benchmark -m uds

# Multiple mechanisms
ipc-benchmark -m uds shm tcp

# All available mechanisms
ipc-benchmark -m all
```

### Available Mechanisms

| Short | Full Name | Best For |
|-------|-----------|----------|
| `uds` | Unix Domain Sockets | General local IPC |
| `shm` | Shared Memory | Maximum throughput |
| `tcp` | TCP Sockets | Network-capable testing |
| `pmq` | POSIX Message Queues | Kernel-managed messaging |
| `all` | All of the above | Comparative analysis |

## Controlling Test Parameters

### Message Count vs Duration

**Message count** (default mode):
```bash
# Send exactly 10,000 messages
ipc-benchmark -m uds -i 10000
```

**Duration-based** (takes precedence):
```bash
# Run for 30 seconds
ipc-benchmark -m uds -d 30s

# Run for 5 minutes
ipc-benchmark -m uds -d 5m
```

### Message Size

```bash
# Small messages (64 bytes)
ipc-benchmark -m uds -s 64

# Large messages (64 KB)
ipc-benchmark -m uds -s 65536
```

### Warmup Iterations

Warmup helps stabilize measurements by "warming up" caches and buffers:

```bash
# Default is 1000 warmup iterations
ipc-benchmark -m uds -i 10000

# Increase warmup for more stable results
ipc-benchmark -m uds -i 10000 -w 5000
```

## Test Types

By default, both one-way and round-trip tests run. You can select specific tests:

```bash
# Only one-way latency tests
ipc-benchmark -m uds --one-way

# Only round-trip latency tests
ipc-benchmark -m uds --round-trip

# Both (default)
ipc-benchmark -m uds --one-way --round-trip
```

### One-Way vs Round-Trip

| Test Type | Measures | Use Case |
|-----------|----------|----------|
| One-Way | Send time only | Streaming, fire-and-forget |
| Round-Trip | Send + receive | Request-response patterns |

## Output Options

### Console Output (Default)

Without any output flags, results print to the console:

```bash
ipc-benchmark -m uds -i 10000
```

### JSON Summary File

Save aggregated results to a JSON file:

```bash
# Explicit filename
ipc-benchmark -m uds -i 10000 -o results.json

# Default filename (benchmark_results.json)
ipc-benchmark -m uds -i 10000 -o
```

### Streaming Output

Capture per-message latency data in real-time:

```bash
# JSON streaming (columnar format)
ipc-benchmark -m uds -i 10000 --streaming-output-json stream.json

# CSV streaming
ipc-benchmark -m uds -i 10000 --streaming-output-csv stream.csv

# Both
ipc-benchmark -m uds -i 10000 \
  --streaming-output-json stream.json \
  --streaming-output-csv stream.csv
```

### Complete Output Example

For full analysis, use all output options:

```bash
ipc-benchmark -m uds -i 10000 \
  -o summary.json \
  --streaming-output-json streaming.json \
  --streaming-output-csv streaming.csv \
  --log-file benchmark.log
```

## Verbosity and Logging

### Console Verbosity

```bash
# Default: warnings and errors only
ipc-benchmark -m uds

# Info level
ipc-benchmark -m uds -v

# Debug level
ipc-benchmark -m uds -vv

# Trace level (maximum detail)
ipc-benchmark -m uds -vvv
```

### Log File

```bash
# Log to file (default: ipc_benchmark.log)
ipc-benchmark -m uds --log-file benchmark.log

# Log to stderr
ipc-benchmark -m uds --log-file stderr
```

### Quiet Mode

Suppress all console output except errors:

```bash
ipc-benchmark -m uds -i 10000 -o results.json -q
```

## Handling Failures

### Continue on Error

By default, the benchmark stops if any mechanism fails. Use `--continue-on-error` to test all mechanisms even if some fail:

```bash
ipc-benchmark -m all --continue-on-error -o results.json
```

## Common Command Patterns

### Quick Comparison

Compare all mechanisms with default settings:

```bash
ipc-benchmark -m all -i 10000 -o comparison.json
```

### Latency-Focused Test

Small messages, many iterations, detailed percentiles:

```bash
ipc-benchmark -m uds \
  -s 64 \
  -i 100000 \
  -w 10000 \
  --percentiles 50 90 95 99 99.9 99.99
```

### Throughput-Focused Test

Large messages, high concurrency:

```bash
ipc-benchmark -m shm \
  -s 65536 \
  -d 60s \
  --buffer-size 1048576
```

### Dashboard-Ready Output

Generate all files needed for the performance dashboard:

```bash
ipc-benchmark -m uds shm tcp pmq \
  -i 50000 \
  -o ./results/summary.json \
  --streaming-output-json ./results/streaming.json \
  --continue-on-error
```

---

# Advanced Usage

This section covers advanced features including execution modes, CPU affinity, cross-process testing, and performance optimization.

## Execution Modes: Async vs Blocking

The benchmark supports two execution modes for different use cases.

### Async Mode (Default)

Uses the Tokio runtime with async/await:

```bash
# Async mode (default)
ipc-benchmark -m uds -i 10000
```

**Characteristics:**
- Uses `tokio::spawn` for concurrency
- Non-blocking I/O operations
- Matches modern async Rust applications
- Efficient for high-concurrency workloads

### Blocking Mode

Uses pure standard library with blocking I/O:

```bash
# Blocking mode
ipc-benchmark -m uds -i 10000 --blocking
```

**Characteristics:**
- Uses `std::thread` for concurrency
- Blocking I/O operations
- No Tokio runtime overhead
- Simpler execution model

### Comparing Modes

Run the same test in both modes for direct comparison:

```bash
# Async test
ipc-benchmark -m uds -i 50000 -o async_results.json

# Blocking test
ipc-benchmark -m uds -i 50000 --blocking -o blocking_results.json
```

## Shared Memory Direct Mode

For maximum shared memory performance, use the `--shm-direct` flag:

```bash
# Direct memory mode (auto-enables blocking)
ipc-benchmark -m shm --shm-direct -i 10000
```

**Benefits:**
- 3x faster average latency (~7μs vs ~20μs)
- 450x better tail latency (~22μs vs ~10ms)
- No serialization overhead

**Limitations:**
- Unix-only (not Windows)
- Fixed message size (8KB max)

## CPU Affinity

Pin server and client to specific CPU cores for reproducible results:

```bash
# Pin server to core 0, client to core 1
ipc-benchmark -m uds -i 10000 \
  --server-affinity 0 \
  --client-affinity 1
```

### Terminology

| Flag | Role | Description |
|------|------|-------------|
| `--server-affinity` | Receiver | The process receiving messages |
| `--client-affinity` | Sender | The process sending messages |

### Best Practices

- Use different physical cores (not hyperthreads)
- Avoid cores running system services (core 0 on some systems)
- Check your CPU topology with `lscpu` or `lstopo`

```bash
# Example: Use cores 2 and 4
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 2 \
  --client-affinity 4
```

## Cross-Process Testing

Run benchmarks between separate processes, including across container boundaries.

### Run Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| `standalone` | Both processes on same host (default) | Normal benchmarking |
| `client` | Acts as IPC server (receiver) | Inside containers |
| `sender` | Acts as IPC client (sender) | From host to container |

### Basic Cross-Process Setup

**Terminal 1: Start the receiver (client mode)**
```bash
./ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Terminal 2: Start the sender**
```bash
./ipc-benchmark -m tcp --run-mode sender --blocking \
  --host 127.0.0.1 \
  -i 10000 \
  -o results.json
```

### Container Testing

**Step 1: Copy binary to container**
```bash
podman cp ./target/release/ipc-benchmark mycontainer:/tmp/
```

**Step 2: Start receiver in container**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Step 3: Run sender from host**
```bash
./ipc-benchmark -m tcp --run-mode sender --blocking \
  --host <container-ip> \
  -i 10000 \
  -o results.json
```

### Mechanism-Specific Container Setup

| Mechanism | Container Configuration |
|-----------|------------------------|
| TCP | Network accessible (`--network host` or port forwarding) |
| UDS | Shared volume for socket file |
| SHM | Shared `/dev/shm` mount |
| PMQ | `--ipc=host` + mount `/dev/mqueue` |

### Cross-Container Flag

For shared memory across containers, the `--cross-container` flag enables container-safe synchronization:

```bash
# Usually auto-detected, but can be forced
ipc-benchmark -m shm --run-mode client --blocking --cross-container
```

## Latency-Focused Testing

Use `--send-delay` to measure latency under controlled load instead of maximum throughput:

```bash
# Send a message every 10ms
ipc-benchmark -m uds -i 10000 --send-delay 10ms

# Send a message every 50 microseconds
ipc-benchmark -m uds -i 10000 --send-delay 50us
```

**When to use:**
- Testing latency under realistic workloads
- Avoiding backpressure effects
- Simulating periodic message patterns

## Custom Percentiles

Request specific percentile calculations:

```bash
# Standard set
ipc-benchmark -m uds -i 10000 --percentiles 50 95 99 99.9

# Extended set for latency analysis
ipc-benchmark -m uds -i 100000 --percentiles 50 75 90 95 99 99.5 99.9 99.99
```

## Buffer Size Tuning

Control internal buffer sizes for different testing scenarios:

```bash
# Large buffer for high throughput
ipc-benchmark -m shm -i 100000 --buffer-size 1048576

# Small buffer to test backpressure
ipc-benchmark -m shm -i 10000 --buffer-size 8192
```

**Note:** If omitted, buffer size is auto-calculated based on message count and size.

## First Message Handling

By default, the first message is discarded to avoid cold-start effects:

```bash
# Include first message in results
ipc-benchmark -m uds -i 10000 --include-first-message
```

## Complete Advanced Example

```bash
# High-precision latency benchmark
./target/release/ipc-benchmark \
  -m shm \
  --shm-direct \
  -s 64 \
  -i 100000 \
  -w 10000 \
  --server-affinity 2 \
  --client-affinity 4 \
  --percentiles 50 90 95 99 99.9 99.99 \
  -o ./results/shm_latency.json \
  --streaming-output-json ./results/shm_streaming.json \
  -v
```

---

# IPC Mechanisms

This section provides detailed information about each Inter-Process Communication mechanism supported by rusty-comms.

## Overview

| Mechanism | Short | Latency | Throughput | Platform | Best For |
|-----------|-------|---------|------------|----------|----------|
| Unix Domain Sockets | `uds` | Low (2-10μs) | High | Unix | General local IPC |
| Shared Memory | `shm` | Very Low (1-7μs) | Very High | All | Maximum performance |
| TCP Sockets | `tcp` | Medium (5-20μs) | High | All | Network capability |
| POSIX Message Queues | `pmq` | Low (5-15μs) | Medium | Linux | Kernel-managed messaging |

## Unix Domain Sockets (UDS)

### Description

Unix Domain Sockets provide reliable, ordered communication between processes on the same machine. They use the filesystem namespace for addressing.

### Usage

```bash
# Basic UDS test
ipc-benchmark -m uds -i 10000

# With blocking mode
ipc-benchmark -m uds -i 10000 --blocking
```

### Characteristics

- **Latency**: 2-10 μs typical
- **Throughput**: High
- **Platform**: Unix-like systems only (Linux, macOS, BSD)
- **Connection**: Stream-oriented (like TCP)
- **Reliability**: Guaranteed delivery, ordered

### When to Use

- General-purpose local IPC
- Client-server architectures
- When you need reliable, ordered delivery
- Cross-container communication with shared volumes

### Platform Notes

- Not available on Windows
- Socket files created in `/tmp/` by default
- Clean up socket files automatically on shutdown

## Shared Memory (SHM)

### Description

Shared Memory provides the fastest possible IPC by allowing processes to access the same memory region. rusty-comms offers two implementations.

### Ring Buffer (Default)

Uses a ring buffer with bincode serialization:

```bash
# Ring buffer with async
ipc-benchmark -m shm -i 10000

# Ring buffer with blocking
ipc-benchmark -m shm -i 10000 --blocking
```

**Characteristics:**
- Latency: ~20 μs average
- Variable message sizes
- Cross-platform (Linux, macOS, Windows, BSD)
- Uses bincode serialization

### Direct Memory Mode

Uses direct memory access without serialization:

```bash
# Direct memory (auto-enables blocking)
ipc-benchmark -m shm --shm-direct -i 10000
```

**Characteristics:**
- Latency: ~7 μs average (3x faster)
- Maximum latency: ~22 μs (vs ~10ms for ring buffer)
- Fixed message size (8KB max payload)
- Unix-only (no Windows)
- No serialization overhead

### Comparison

| Feature | Ring Buffer | Direct Memory |
|---------|-------------|---------------|
| Average Latency | ~20 μs | ~7 μs |
| Max Latency | ~10 ms | ~22 μs |
| Message Size | Variable | Fixed (8KB) |
| Serialization | bincode | None |
| Platform | All | Unix only |

### When to Use

**Ring Buffer:**
- Cross-platform support needed
- Variable message sizes required
- Flexibility over raw performance

**Direct Memory:**
- Maximum performance critical
- Fixed message sizes acceptable
- Unix/Linux deployment only

### Configuration

```bash
# Custom buffer size
ipc-benchmark -m shm --buffer-size 1048576 -i 10000

# With CPU affinity for best results
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 0 --client-affinity 1
```

## TCP Sockets

### Description

TCP Sockets provide network-capable communication. When used for localhost testing, they measure the overhead of the TCP/IP stack.

### Usage

```bash
# Basic TCP test
ipc-benchmark -m tcp -i 10000

# Custom host and port
ipc-benchmark -m tcp -i 10000 --host 127.0.0.1 --port 9090
```

### Characteristics

- **Latency**: 5-20 μs (localhost)
- **Throughput**: High
- **Platform**: All platforms
- **Connection**: Stream-oriented, reliable
- **Network**: Can test across network boundaries

### When to Use

- Baseline for network-capable applications
- Cross-machine testing
- Container networking tests
- When UDS is not available

### Configuration Options

| Option | Default | Description |
|--------|---------|-------------|
| `--host` | 127.0.0.1 | Server bind address |
| `--port` | 8080 | Server port |

### Cross-Process Example

```bash
# Terminal 1: Server
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0

# Terminal 2: Client
ipc-benchmark -m tcp --run-mode sender --blocking \
  --host 127.0.0.1 -i 10000
```

## POSIX Message Queues (PMQ)

### Description

POSIX Message Queues are kernel-managed message passing queues. They preserve message boundaries and support priority-based delivery.

### Usage

```bash
# Basic PMQ test
ipc-benchmark -m pmq -i 10000

# With message priority
ipc-benchmark -m pmq -i 10000 --pmq-priority 1
```

### Characteristics

- **Latency**: 5-15 μs typical
- **Throughput**: Medium (kernel-limited)
- **Platform**: Linux only
- **Message Boundaries**: Preserved
- **Priority**: Supports message priority levels

### System Limits

PMQ has kernel-imposed limits:

```bash
# Check current limits
cat /proc/sys/fs/mqueue/msg_max       # Max messages per queue
cat /proc/sys/fs/mqueue/msgsize_max   # Max message size (default: 8192)
```

### When to Use

- Priority-based message delivery needed
- Message boundaries must be preserved
- Kernel-managed queues preferred
- Linux-only deployment

### Configuration

| Option | Default | Description |
|--------|---------|-------------|
| `--pmq-priority` | 0 | Message priority (higher = delivered first) |
| `--buffer-size` | 8192 | Message size (limited by kernel) |

### Increasing Limits

```bash
# Increase message size limit (requires root)
sudo sysctl -w fs.mqueue.msgsize_max=16384

# Increase queue depth
sudo sysctl -w fs.mqueue.msg_max=100

# Mount mqueue filesystem if not mounted
sudo mount -t mqueue none /dev/mqueue
```

## Mechanism Selection Guide

### By Use Case

| Use Case | Recommended | Reason |
|----------|-------------|--------|
| General benchmarking | UDS | Good balance, widely applicable |
| Maximum performance | SHM (direct) | Lowest latency |
| Network simulation | TCP | Includes TCP/IP overhead |
| Priority messaging | PMQ | Built-in priority support |
| Cross-platform | TCP or SHM (ring) | Windows compatible |

### By Latency Requirements

| Requirement | Mechanism | Expected Latency |
|-------------|-----------|------------------|
| Sub-10μs | SHM (direct) | ~7 μs |
| 10-20μs | UDS, SHM (ring) | 2-20 μs |
| 20-50μs | TCP, PMQ | 5-20 μs |

### By Throughput Requirements

| Requirement | Mechanism | Configuration |
|-------------|-----------|---------------|
| Maximum | SHM (direct) | Large messages, CPU affinity |
| High | SHM (ring), UDS | Default settings |
| Moderate | TCP, PMQ | Standard configuration |

---

# Output Formats

This section explains the various output formats generated by the IPC benchmark suite.

## Overview

| Format | Flag | Purpose |
|--------|------|---------|
| Console | (default) | Human-readable summary |
| JSON Summary | `-o` | Machine-readable aggregated results |
| Streaming JSON | `--streaming-output-json` | Per-message latency data |
| Streaming CSV | `--streaming-output-csv` | Spreadsheet-compatible data |
| Log File | `--log-file` | Diagnostic information |

## Console Output

Without any output flags, results print to the console in a human-readable format.

### Configuration Summary

Displayed at startup:

```
Benchmark Configuration:
-----------------------------------------------------------------
  Mechanisms:         Unix Domain Socket
  Message Size:       1024 bytes
  Iterations:         10000
  Warmup Iterations:  1000
  Test Types:         One-Way, Round-Trip
-----------------------------------------------------------------
```

### Results Summary

Displayed after completion:

```
Benchmark Results:
-----------------------------------------------------------------
Mechanism: Unix Domain Socket
  Message Size: 1024 bytes
  One-Way Latency:
      Mean: 3.15 us, P95: 5.21 us, P99: 8.43 us
      Min:  1.50 us, Max: 45.12 us
  Round-Trip Latency:
      Mean: 5.82 us, P95: 9.11 us, P99: 14.50 us
  Throughput:
      Average: 155.30 MB/s, Peak: 156.40 MB/s
-----------------------------------------------------------------
```

## JSON Summary Output

Use `-o` to generate a JSON file with aggregated results.

```bash
ipc-benchmark -m uds -i 10000 -o results.json
```

### JSON Structure

```json
{
  "metadata": {
    "version": "0.1.0",
    "timestamp": "2024-01-01T00:00:00Z",
    "system_info": {
      "os": "linux",
      "architecture": "x86_64",
      "cpu_cores": 8
    }
  },
  "results": [
    {
      "mechanism": "UnixDomainSocket",
      "test_config": {
        "message_size": 1024,
        "concurrency": 1,
        "msg_count": 10000
      },
      "one_way_results": {
        "latency": {
          "min_ns": 1500,
          "max_ns": 45000,
          "mean_ns": 3200.5,
          "percentiles": [
            {"percentile": 95.0, "value_ns": 5200},
            {"percentile": 99.0, "value_ns": 8500}
          ]
        },
        "throughput": {
          "messages_per_second": 312500.0,
          "bytes_per_second": 320000000.0
        }
      },
      "round_trip_results": { ... }
    }
  ]
}
```

### Key Fields

| Field | Description |
|-------|-------------|
| `metadata.timestamp` | Test execution time |
| `metadata.system_info` | System configuration |
| `results[].mechanism` | IPC mechanism tested |
| `results[].one_way_results` | One-way latency and throughput |
| `results[].round_trip_results` | Round-trip latency and throughput |

## Streaming JSON Output

Use `--streaming-output-json` for per-message latency data.

```bash
ipc-benchmark -m uds -i 10000 --streaming-output-json stream.json
```

### Columnar Format

The streaming JSON uses a columnar format for efficiency:

```json
{
  "headings": [
    "timestamp_ns",
    "message_id",
    "mechanism",
    "message_size",
    "one_way_latency_ns",
    "round_trip_latency_ns"
  ],
  "data": [
    [1737417600000000000, 1, "UnixDomainSocket", 1024, 50000, null],
    [1737417600000100000, 2, "UnixDomainSocket", 1024, 51000, null]
  ]
}
```

### Timestamp Semantics

- **`timestamp_ns`**: Unix epoch timestamp in nanoseconds for when the record was
  captured (useful for correlating with external monitoring).
- **Latency values** (`one_way_latency_ns`, `round_trip_latency_ns`): measured
  durations derived from a **monotonic clock** (immune to NTP/manual clock
  adjustments).

### Processing Streaming JSON

```python
import json

with open('stream.json') as f:
    data = json.load(f)

headings = data['headings']
for row in data['data']:
    record = dict(zip(headings, row))
    ow = record["one_way_latency_ns"]
    rt = record["round_trip_latency_ns"]
    print(
        f"{record['mechanism']} msg {record['message_id']}: "
        f"one_way={ow} ns, round_trip={rt} ns"
    )
```

## Streaming CSV Output

Use `--streaming-output-csv` for spreadsheet-compatible data.

```bash
ipc-benchmark -m uds -i 10000 --streaming-output-csv stream.csv
```

### CSV Format

```csv
timestamp_ns,message_id,mechanism,message_size,one_way_latency_ns,round_trip_latency_ns
1737417600000000000,1,UnixDomainSocket,1024,50000,
1737417600000100000,2,UnixDomainSocket,1024,51000,
```

### Importing to Spreadsheets

The CSV format works directly with:
- Microsoft Excel
- Google Sheets
- LibreOffice Calc
- Python pandas

```python
import pandas as pd

df = pd.read_csv('stream.csv')
print(df['one_way_latency_ns'].describe())
```

## Log File Output

Use `--log-file` for diagnostic information.

```bash
ipc-benchmark -m uds -i 10000 --log-file benchmark.log
```

### Log Levels

Control verbosity with `-v`:

| Flag | Level | Information |
|------|-------|-------------|
| (none) | WARN | Warnings and errors only |
| `-v` | INFO | Configuration and progress |
| `-vv` | DEBUG | Detailed operation info |
| `-vvv` | TRACE | Maximum detail |

### Log to stderr

```bash
ipc-benchmark -m uds --log-file stderr -vv
```

## Complete Output Example

Generate all output formats:

```bash
ipc-benchmark -m uds -i 10000 \
  -o summary.json \
  --streaming-output-json streaming.json \
  --streaming-output-csv streaming.csv \
  --log-file benchmark.log \
  -v
```

## Dashboard Integration

For the performance dashboard, you need both summary and streaming output:

```bash
mkdir -p ./results
ipc-benchmark -m uds shm tcp pmq \
  -i 50000 \
  -o ./results/summary.json \
  --streaming-output-json ./results/streaming.json \
  --continue-on-error
```

---

# Performance Tuning

This section covers system configuration and best practices for accurate, reproducible benchmark results.

## Why Tuning Matters

Without proper tuning, benchmark results can vary significantly due to:
- CPU frequency scaling
- Process scheduling
- Cache effects
- Background system activity

## CPU Configuration

### Disable Frequency Scaling

CPU frequency scaling causes measurement variability. Set to performance mode:

```bash
# Check current governor
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Set to performance (requires root)
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

### Disable Turbo Boost

Turbo boost can cause inconsistent results:

```bash
# Intel CPUs
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# AMD CPUs
echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost
```

### CPU Affinity

Pin processes to specific cores to avoid migration:

```bash
ipc-benchmark -m uds -i 10000 \
  --server-affinity 2 \
  --client-affinity 4
```

**Best practices:**
- Use different physical cores (not hyperthreads)
- Avoid core 0 (often handles system interrupts)
- Check topology with `lscpu` or `lstopo`

## Memory Configuration

### Shared Memory Limits

Check and increase shared memory limits:

```bash
# Check current limits
ipcs -lm

# Increase maximum segment size (2GB)
echo 2147483648 | sudo tee /proc/sys/kernel/shmmax

# Increase maximum segments
echo 4096 | sudo tee /proc/sys/kernel/shmmni
```

### Huge Pages

Enable huge pages for better memory performance:

```bash
# Enable transparent huge pages
echo always | sudo tee /sys/kernel/mm/transparent_hugepage/enabled

# Or allocate explicit huge pages
echo 1024 | sudo tee /proc/sys/vm/nr_hugepages
```

## Network Configuration (TCP)

### Increase Buffer Sizes

```bash
# Maximum receive buffer
echo 16777216 | sudo tee /proc/sys/net/core/rmem_max

# Maximum send buffer
echo 16777216 | sudo tee /proc/sys/net/core/wmem_max

# TCP buffer range
echo "4096 87380 16777216" | sudo tee /proc/sys/net/ipv4/tcp_rmem
echo "4096 65536 16777216" | sudo tee /proc/sys/net/ipv4/tcp_wmem
```

## POSIX Message Queue Configuration

### Check Limits

```bash
cat /proc/sys/fs/mqueue/msg_max      # Max messages per queue
cat /proc/sys/fs/mqueue/msgsize_max  # Max message size
```

### Increase Limits

```bash
# Increase message size limit
echo 16384 | sudo tee /proc/sys/fs/mqueue/msgsize_max

# Increase queue depth
echo 100 | sudo tee /proc/sys/fs/mqueue/msg_max
```

### Mount Message Queue Filesystem

```bash
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue
```

## Benchmark Configuration

### Warmup Iterations

Use warmup to stabilize measurements:

```bash
# Default is 1000, increase for more stability
ipc-benchmark -m uds -i 100000 -w 10000
```

### Message Count vs Duration

**Message count** for consistent comparisons:
```bash
ipc-benchmark -m uds -i 100000
```

**Duration** for time-consistent tests:
```bash
ipc-benchmark -m uds -d 60s
```

### Buffer Size

For throughput tests, ensure buffers are large enough:

```bash
ipc-benchmark -m shm -i 100000 --buffer-size 1048576
```

### Send Delay for Latency Testing

Avoid backpressure effects when measuring latency:

```bash
ipc-benchmark -m uds -i 10000 --send-delay 10ms
```

## System Preparation

### Before Benchmarking

1. **Close unnecessary applications**
2. **Stop background services** (if possible)
3. **Check system load**: `uptime`
4. **Check for I/O activity**: `iostat -x 1`
5. **Check memory usage**: `free -h`

### Reproducibility Checklist

```bash
# 1. Set CPU governor
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# 2. Disable turbo
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null || true

# 3. Check load
uptime

# 4. Run benchmark with affinity
ipc-benchmark -m shm --shm-direct -i 100000 \
  --server-affinity 2 --client-affinity 4 \
  -w 10000 \
  -o results.json
```

## Compiler Optimizations

### Release Build

Always use release builds for benchmarking:

```bash
cargo build --release
```

### Native CPU Optimization

Build for your specific CPU:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

### Link-Time Optimization

Enable LTO for maximum performance:

```bash
RUSTFLAGS="-C lto=fat" cargo build --release
```

## Interpreting Results

### Expected Variability

| Metric | Acceptable Variance |
|--------|---------------------|
| Mean latency | < 5% |
| P99 latency | < 10% |
| Throughput | < 5% |

### Signs of Problems

- **High max latency**: System interference
- **Bimodal distribution**: CPU frequency changes
- **Increasing latency over time**: Memory pressure
- **Inconsistent runs**: Insufficient warmup

### Multiple Runs

Run benchmarks multiple times and average:

```bash
for i in 1 2 3 4 5; do
  ipc-benchmark -m uds -i 100000 -o "run_${i}.json"
done
```

---

# Troubleshooting

This section covers common issues and their solutions when using the IPC Benchmark Suite.

## Quick Diagnostics

### Enable Verbose Logging

The first step for any issue is to enable verbose logging:

```bash
# Debug level
ipc-benchmark -m uds -i 1000 -vv

# Trace level (maximum detail)
ipc-benchmark -m uds -i 1000 -vvv

# Save to log file
ipc-benchmark -m uds -i 1000 -vv --log-file debug.log
```

## Common Issues

### Binary Not Found

**Error:**
```
Could not resolve 'ipc-benchmark' binary for server mode
```

**Solution:**
Build the project first:
```bash
cargo build --release
```

Or run with the full path:
```bash
./target/release/ipc-benchmark -m uds
```

### Permission Denied

**Error:**
```
Permission denied (os error 13)
```

**Solutions:**

1. Check directory permissions:
```bash
# Ensure write access to current directory
ls -la .

# Ensure write access to /tmp
ls -la /tmp
```

2. Clean up stale files:
```bash
rm -f /tmp/ipc_benchmark_*
```

3. Run with appropriate permissions (not typically needed):
```bash
sudo ./target/release/ipc-benchmark -m uds
```

### Address Already in Use

**Error:**
```
Address already in use (os error 98)
```

**Solutions:**

1. Wait for previous test to complete
2. Use a different port:
```bash
ipc-benchmark -m tcp --port 9090
```
3. Check for orphan processes:
```bash
ps aux | grep ipc-benchmark
kill <pid>
```

## Mechanism-Specific Issues

### Unix Domain Sockets (UDS)

#### Socket File Remains After Crash

**Problem:** Stale socket files prevent new tests.

**Solution:**
```bash
rm -f /tmp/ipc_benchmark_*.sock
```

#### Not Available on Windows

**Error:**
```
Unix domain sockets are not available on this platform
```

**Solution:** Use TCP sockets instead:
```bash
ipc-benchmark -m tcp -i 10000
```

### Shared Memory (SHM)

#### Unexpected End of File

**Error:**
```
unexpected end of file
```

**Causes:**
- Buffer size too small
- Concurrency race conditions

**Solution:**
```bash
# Increase buffer size
ipc-benchmark -m shm -i 10000 --buffer-size 1048576
```

#### Timeout Sending Message

**Error:**
```
Timeout sending message
```

**Solution:** Increase buffer size or reduce concurrency:
```bash
ipc-benchmark -m shm -i 10000 --buffer-size 2097152
```

#### Backpressure Warnings

**Warning:**
```
WARN rusty_comms::ipc::shared_memory: Shared memory buffer is full; backpressure is occurring.
```

**This is informational.** It means the sender is faster than the receiver. Options:

1. Increase buffer size (if testing throughput):
```bash
ipc-benchmark -m shm -i 10000 --buffer-size 1048576
```

2. Add send delay (if testing latency):
```bash
ipc-benchmark -m shm -i 10000 --send-delay 10ms
```

3. Accept backpressure as a valid test scenario.

### TCP Sockets

#### Connection Refused

**Error:**
```
Connection refused (os error 111)
```

**Causes:**
- Server not started (in cross-process mode)
- Wrong host/port

**Solutions:**

1. Start the server first:
```bash
# Terminal 1
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

2. Verify host/port:
```bash
# Terminal 2
ipc-benchmark -m tcp --run-mode sender --blocking --host 127.0.0.1 --port 8080
```

### POSIX Message Queues (PMQ)

#### PMQ Not Available

**Error:**
```
POSIX Message Queues are not available on this platform
```

**Solution:** PMQ is Linux-only. Use another mechanism on other platforms.

#### Message Too Long

**Error:**
```
Message too long (os error 90)
```

**Cause:** Message size exceeds kernel limit.

**Solution:**

1. Check current limits:
```bash
cat /proc/sys/fs/mqueue/msgsize_max
```

2. Increase limit (requires root):
```bash
sudo sysctl -w fs.mqueue.msgsize_max=16384
```

3. Or use smaller messages:
```bash
ipc-benchmark -m pmq -s 8000 -i 10000
```

#### /dev/mqueue Not Mounted

**Error:**
```
No such file or directory (os error 2)
```

**Solution:**
```bash
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue
```

#### Queue Already Exists

**Error:**
```
File exists (os error 17)
```

**Solution:** Clean up stale queues:
```bash
ls /dev/mqueue/
rm /dev/mqueue/ipc_benchmark_*
```

## Performance Issues

### Inconsistent Results

**Problem:** Latency measurements vary between runs.

**Solutions:**

1. Use CPU affinity:
```bash
ipc-benchmark -m uds -i 10000 \
  --server-affinity 0 --client-affinity 1
```

2. Increase warmup iterations:
```bash
ipc-benchmark -m uds -i 10000 -w 5000
```

3. Disable CPU frequency scaling:
```bash
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

4. Run on idle system

### High Maximum Latency

**Problem:** P99.9 or max latency is much higher than expected.

**Causes:**
- OS scheduling jitter
- CPU frequency changes
- System interrupts

**Solutions:**

1. Use CPU affinity (most effective)
2. Disable turbo boost:
```bash
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
```
3. Use `--shm-direct` for shared memory (450x better tail latency)

### First Message Spike

**Problem:** First message has very high latency.

**This is expected.** Cold-start effects include:
- CPU cache misses
- Memory allocation
- Branch prediction misses

**Solution:** The tool automatically discards the first message. To include it:
```bash
ipc-benchmark -m uds -i 10000 --include-first-message
```

## Cross-Process Issues

### Container Can't Connect

**Problem:** Sender can't reach receiver in container.

**Solutions:**

1. Check container IP:
```bash
podman inspect <container> | grep IPAddress
```

2. Use correct host binding in container:
```bash
podman exec <container> /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

3. For SHM, share `/dev/shm`:
```bash
podman run --volume /dev/shm:/dev/shm ...
```

4. For PMQ, share IPC namespace:
```bash
podman run --ipc=host --volume /dev/mqueue:/dev/mqueue ...
```

## Debugging Commands

### Check System Limits

```bash
# Shared memory
ipcs -lm

# Message queues
cat /proc/sys/fs/mqueue/msg_max
cat /proc/sys/fs/mqueue/msgsize_max

# Open files
ulimit -n
```

### Check for Stale Resources

```bash
# Stale socket files
ls -la /tmp/ipc_benchmark_*

# Stale message queues
ls /dev/mqueue/

# Stale shared memory segments
ipcs -m | grep ipc_benchmark
```

### Clean Up Resources

```bash
# Remove socket files
rm -f /tmp/ipc_benchmark_*

# Remove message queues
rm /dev/mqueue/ipc_benchmark_* 2>/dev/null

# Remove shared memory (find IPC key first)
ipcs -m | grep ipc_benchmark
ipcrm -m <shmid>
```

## Getting Help

If issues persist:

1. Run with maximum verbosity:
```bash
ipc-benchmark -m uds -i 100 -vvv --log-file detailed.log 2>&1 | tee output.txt
```

2. Capture system info:
```bash
uname -a
cat /etc/os-release
rustc --version
cargo --version
```

3. Check the [GitHub Issues](https://github.com/redhat-performance/rusty-comms/issues)

4. Open a new issue with:
   - Command used
   - Error message
   - Log file contents
   - System information

---

*This user guide is also available as separate chapter files in `docs/user-guide/`.*
