# IPC Benchmark Suite

A comprehensive interprocess communication (IPC) benchmark suite implemented in Rust, designed to measure performance characteristics of different IPC mechanisms with a focus on latency and throughput.

## Overview

This benchmark suite provides a systematic way to evaluate the performance of various IPC mechanisms commonly used in systems programming. It's designed to be:

- **Comprehensive**: Measures both latency and throughput for one-way and round-trip communication patterns
- **Configurable**: Supports various message sizes, concurrency levels, and test durations
- **Cross-Environment**: Supports host-to-container IPC testing for safety-critical applications
- **Reproducible**: Generates detailed JSON output suitable for automated analysis
- **Production-ready**: Optimized for performance measurement with minimal overhead

## Execution Modes

The benchmark suite supports three distinct execution modes:

### 1. **Standalone Mode** (Default)
Traditional single-process benchmarking where both client and server run within the same process.
```bash
./target/release/ipc-benchmark -m uds --message-size 1024 --msg-count 1000
```

### 2. **Host-to-Container Mode** (Cross-Environment)
Real-world testing between host environment (safety domain) and containerized environment (non-safety domain).
```bash
# Host side (acts as client)
./run_host_container.sh uds 1000 1024 1

# Container side (acts as server) - automatically managed
```

### 3. **Manual Client/Server Mode**
Manual control over client and server processes for custom testing scenarios.
```bash
# Server process
./target/release/ipc-benchmark --mode client -m uds --ipc-path ./sockets/test.sock

# Client process  
./target/release/ipc-benchmark --mode host -m uds --ipc-path ./sockets/test.sock
```

## Features

### Supported IPC Mechanisms

1. **Unix Domain Sockets** (`uds`) - Low-latency local communication
2. **Shared Memory** (`shm`) - Highest throughput for large data transfers
3. **TCP Sockets** (`tcp`) - Network-capable, standardized communication
4. **POSIX Message Queues** (`pmq`) - Kernel-managed, priority-based messaging

### Measurement Capabilities

- **Latency Metrics**: One-way and round-trip latency measurements
- **Throughput Metrics**: Message rate and bandwidth measurements
- **Statistical Analysis**: Percentiles (P50, P95, P99, P99.9), min/max, standard deviation
- **Concurrency Testing**: Multi-threaded/multi-process performance evaluation
- **Warmup Support**: Stabilizes performance measurements

### Output Formats

- **JSON**: Optional, machine-readable structured output for final, aggregated results. Generated only when the `--output-file` flag is used.
- **Streaming JSON**: Real-time, per-message latency data written to a file in a columnar JSON format. This allows for efficient, live monitoring of long-running tests. The format consists of a `headings` array and a `data` array containing value arrays for each message.
- **Streaming CSV**: Real-time, per-message latency data written to a standard CSV file. This format is ideal for easy import into spreadsheets and data analysis tools.
- **Console Output**: User-friendly, color-coded summaries on `stdout`. Includes a configuration summary at startup and a detailed results summary upon completion.
- **Detailed Logs**: Structured, timestamped logs written to a file or `stderr` for diagnostics.

## Installation

### Prerequisites

- **Rust**: 1.75.0 or later
- **Operating System**: Linux (tested on RHEL 9.6)
- **Dependencies**: All handled by Cargo

### Building from Source

```bash
git clone https://github.com/your-org/ipc-benchmark.git
cd ipc-benchmark
cargo build --release
```

The optimized binary will be available at `target/release/ipc-benchmark`.

### Quick Start

#### Standalone Testing (Single Process)
```bash
# Run all IPC mechanisms with default settings
./target/release/ipc-benchmark -m all

# Run specific mechanisms with custom configuration
./target/release/ipc-benchmark \
  -m uds shm tcp \
  --message-size 4096 \
  --msg-count 50000 \
  --concurrency 4
```

#### Host-to-Container Testing (Cross-Environment)
```bash
# Prerequisites: Install Podman (see PODMAN_SETUP.md)
# Start container server for UDS testing
./start_uds_container_server.sh &

# Run host-to-container benchmark
./run_host_container.sh uds 1000 1024 1

# Stop container when done
podman rm -f rusty-comms-uds-server

# For all mechanisms and syntax examples, see:
# - README_HOST_TO_CONTAINER.md
# - HOST_TO_CONTAINER_SYNTAX_EXAMPLES.txt
```

## Usage

### Basic Usage

```bash
# Run benchmark with default settings (prints summary to console, no file output)
ipc-benchmark

# Run with specific mechanisms
ipc-benchmark -m uds shm pmq

# Run all mechanisms (including PMQ)
ipc-benchmark -m all

# Test POSIX message queue specifically
ipc-benchmark -m pmq --message-size 1024 --msg-count 10000

# Run with custom message size and message count
ipc-benchmark --message-size 1024 --msg-count 10000

# Run for a specific duration
ipc-benchmark --duration 30s

# Run with multiple concurrent workers
ipc-benchmark --concurrency 8
```

### Advanced Configuration

```bash
# Run with a larger message size and for a fixed duration
ipc-benchmark --message-size 65536 --duration 30s

# Create an output file with a custom name
ipc-benchmark --output-file my_results.json

# Create the default output file (benchmark_results.json)
ipc-benchmark --output-file

# Enable JSON streaming output to a custom file
ipc-benchmark --streaming-output-json my_stream.json

# Enable JSON streaming output to the default file (benchmark_streaming_output.json)
ipc-benchmark --streaming-output-json

# Enable CSV streaming output to a custom file
ipc-benchmark --streaming-output-csv my_stream.csv

# Enable CSV streaming output to the default file (benchmark_streaming_output.csv)
ipc-benchmark --streaming-output-csv

# Save detailed logs to a custom file
ipc-benchmark --log-file /var/log/ipc-benchmark.log

# Send detailed logs to stderr instead of a file
ipc-benchmark --log-file stderr

# Continue running tests even if one mechanism fails
ipc-benchmark -m all --continue-on-error

# Run only round-trip tests
ipc-benchmark --round-trip --no-one-way

# Custom percentiles for latency analysis
ipc-benchmark --percentiles 50 90 95 99 99.9 99.99

# TCP-specific configuration
ipc-benchmark -m tcp --host 127.0.0.1 --port 9090

# Shared memory configuration
ipc-benchmark -m shm --buffer-size 16384
```

### Test Configuration Examples

#### High-Throughput Testing
```bash
ipc-benchmark \
  -m shm \
  --message-size 65536 \
  --concurrency 8 \
  --duration 60s \
  --buffer-size 1048576
```

#### Low-Latency Testing
```bash
ipc-benchmark \
  -m uds \
  --message-size 64 \
  --msg-count 100000 \
  --warmup-iterations 10000 \
  --percentiles 50 95 99 99.9 99.99
```

#### Comparative Analysis
```bash
ipc-benchmark \
  -m uds shm tcp pmq \
  --message-size 1024 \
  --msg-count 50000 \
  --concurrency 4 \
  --output-file comparison.json
```

#### Test All Mechanisms
```bash
ipc-benchmark \
  -m all \
  --message-size 1024 \
  --msg-count 50000 \
  --concurrency 1 \
  --output-file complete_comparison.json
```

#### POSIX Message Queue Testing
```bash
# Test PMQ with different message sizes
ipc-benchmark \
  -m pmq \
  --message-size 1024 \
  --msg-count 10000 \
  --percentiles 50 95 99

# PMQ with concurrency testing
ipc-benchmark \
  -m pmq \
  --message-size 512 \
  --msg-count 5000 \
  --concurrency 2 \
  --duration 30s
```

## Host-to-Container Cross-Environment Testing

For safety-critical applications that need to test IPC performance between host and container environments, this benchmark suite provides comprehensive cross-environment testing capabilities.

### Prerequisites

1. **Container Runtime**: Podman is required (Docker can be used with modifications)
   ```bash
   # See PODMAN_SETUP.md for detailed installation instructions
   sudo dnf install podman
   ```

2. **Build the Project**: 
   ```bash
   cargo build --release
   ```

### Quick Start - Host-to-Container

#### 1. Using the Unified Script (Recommended)

The `run_host_container.sh` script provides the easiest way to run cross-environment tests:

```bash
# Basic UDS test: 1000 messages, 1024 bytes each, 1 worker
./run_host_container.sh uds 1000 1024 1

# Duration-based test: UDS for 30 seconds with 4KB messages
DURATION=30s ./run_host_container.sh uds 0 4096 1

# Shared Memory test with custom output
OUTPUT_FILE=./output/shm_results.json ./run_host_container.sh shm 5000 2048 1

# POSIX Message Queue test with streaming CSV output
STREAM_CSV=./output/pmq_stream.csv ./run_host_container.sh pmq 1000 512 1
```

#### 2. Manual Setup (Advanced)

For more control, you can manually start containers and run benchmarks:

```bash
# 1. Start the container server (UDS example)
./start_uds_container_server.sh &

# 2. Wait for container to be ready (check logs)
podman logs rusty-comms-uds-server

# 3. Run the host-side benchmark
./target/release/ipc-benchmark \
  --mode host \
  -m uds \
  --ipc-path ./sockets/ipc_benchmark.sock \
  --message-size 1024 \
  --msg-count 1000 \
  --output-file ./output/host_uds_results.json

# 4. Stop the container
podman rm -f rusty-comms-uds-server
```

### Supported Cross-Environment Mechanisms

| Mechanism | Script | Container Script | Status |
|-----------|--------|------------------|--------|
| **Unix Domain Sockets** | `run_host_container.sh uds` | `start_uds_container_server.sh` | ✅ Fully Supported |
| **Shared Memory** | `run_host_container.sh shm` | `start_shm_container_server.sh` | ✅ Fully Supported |
| **POSIX Message Queues** | `run_host_container.sh pmq` | `start_pmq_container_server.sh` | ✅ Fully Supported |

### Configuration Options

The unified script supports all benchmark parameters:

```bash
# Environment Variables
export MECHANISM=uds           # uds, shm, pmq
export DURATION=30s            # Duration instead of message count
export OUTPUT_FILE=./output/results.json
export STREAM_JSON=./output/stream.json
export STREAM_CSV=./output/stream.csv
export SOCKET_PATH=./sockets/custom.sock
export SHM_NAME=custom_shm
export BUFFER_SIZE=1048576
export ROUND_TRIP=true         # Enable round-trip testing
export VERBOSE=true            # Enable verbose logging

# Run with environment variables
./run_host_container.sh $MECHANISM $MESSAGES $MESSAGE_SIZE $WORKERS
```

### Example Test Scenarios

#### Performance Comparison
```bash
# Test all mechanisms with the same parameters
./run_host_container.sh uds 10000 1024 1
./run_host_container.sh shm 10000 1024 1  
./run_host_container.sh pmq 10000 1024 1
```

#### Latency Analysis
```bash
# Small messages for latency measurement
ROUND_TRIP=true STREAM_CSV=./output/uds_latency.csv \
./run_host_container.sh uds 5000 64 1
```

#### Throughput Testing
```bash
# Large messages for throughput measurement
DURATION=60s OUTPUT_FILE=./output/shm_throughput.json \
./run_host_container.sh shm 0 65536 1
```

#### Long-Running Tests
```bash
# 5-minute duration test with streaming output
DURATION=5m STREAM_JSON=./output/long_test.json VERBOSE=true \
./run_host_container.sh uds 0 1024 1
```

### Container Management

#### Starting Individual Container Servers
```bash
# Unix Domain Sockets
./start_uds_container_server.sh &

# Shared Memory  
./start_shm_container_server.sh &

# POSIX Message Queues
./start_pmq_container_server.sh &
```

#### Monitoring Container Status
```bash
# List running containers
podman ps

# View container logs
podman logs -f rusty-comms-uds-server

# Check container health
podman exec rusty-comms-uds-server ipc-benchmark --help
```

#### Cleanup
```bash
# Stop specific containers
podman rm -f rusty-comms-uds-server
podman rm -f rusty-comms-shm-server  
podman rm -f rusty-comms-pmq-server

# Stop all benchmark containers
podman rm -f $(podman ps -q --filter "name=rusty-comms-")
```

### Troubleshooting Cross-Environment Issues

#### Common Problems and Solutions

1. **Container Not Starting**
   ```bash
   # Check Podman installation
   podman --version
   
   # Verify image build
   podman images | grep rusty-comms
   
   # Check for port conflicts
   podman ps -a
   ```

2. **Connection Timeouts**
   ```bash
   # Verify socket file exists
   ls -la ./sockets/
   
   # Check container logs for errors
   podman logs rusty-comms-uds-server
   
   # Increase connection timeout
   CONNECTION_TIMEOUT=30 ./run_host_container.sh uds 1000 1024 1
   ```

3. **Permission Issues**
   ```bash
   # Ensure socket directory permissions
   chmod 755 ./sockets/
   
   # Check SELinux context (if applicable)
   ls -Z ./sockets/
   ```

4. **Performance Issues**
   ```bash
   # Monitor system resources
   htop
   
   # Check container resource usage
   podman stats rusty-comms-uds-server
   
   # Reduce message size or count
   ./run_host_container.sh uds 1000 512 1
   ```

### Best Practices

1. **Always start containers before running benchmarks**
2. **Wait for containers to be fully ready** (check logs)
3. **Use appropriate timeouts** for your test duration
4. **Clean up containers** after testing
5. **Monitor system resources** during long-running tests
6. **Use streaming output** for real-time monitoring

For complete syntax examples and advanced usage, see:
- **[README_HOST_TO_CONTAINER.md](README_HOST_TO_CONTAINER.md)** - Detailed documentation
- **[HOST_TO_CONTAINER_SYNTAX_EXAMPLES.txt](HOST_TO_CONTAINER_SYNTAX_EXAMPLES.txt)** - 100+ syntax examples
- **[HOST_CONTAINER_IPC_SETUP.md](HOST_CONTAINER_IPC_SETUP.md)** - Technical setup details
- **[PODMAN_SETUP.md](PODMAN_SETUP.md)** - Container runtime setup

## Output Format

The benchmark generates comprehensive JSON output with the following structure:

```json
{
  "metadata": {
    "version": "0.1.0",
    "timestamp": "2024-01-01T00:00:00Z",
    "total_tests": 3,
    "system_info": {
      "os": "linux",
      "architecture": "x86_64",
      "cpu_cores": 8,
      "memory_gb": 16.0,
      "rust_version": "1.75.0",
      "benchmark_version": "0.1.0"
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
          "latency_type": "OneWay",
          "min_ns": 1500,
          "max_ns": 45000,
          "mean_ns": 3200.5,
          "median_ns": 3100,
          "percentiles": [
            {"percentile": 95.0, "value_ns": 5200},
            {"percentile": 99.0, "value_ns": 8500}
          ]
        },
        "throughput": {
          "messages_per_second": 312500.0,
          "bytes_per_second": 320000000.0,
          "total_messages": 10000,
          "total_bytes": 10240000
        }
      },
      "summary": {
        "total_messages_sent": 10000,
        "total_bytes_transferred": 10240000,
        "average_throughput_mbps": 305.17,
        "p95_latency_ns": 5200,
        "p99_latency_ns": 8500
      }
    }
  ],
  "summary": {
    "fastest_mechanism": "SharedMemory",
    "lowest_latency_mechanism": "UnixDomainSocket"
  }
}
```

### Console Output

The benchmark provides a human-readable summary directly in your terminal.

**Configuration Summary (at startup):**

This example shows a run where no final output file is being generated.

```
Benchmark Configuration:
-----------------------------------------------------------------
  Mechanisms:         UnixDomainSocket, SharedMemory
  Message Size:       1024 bytes
  Iterations:         10000
  Warmup Iterations:  1000
  Test Types:         One-Way, Round-Trip
  Log File:             ipc_benchmark.log.2025-08-05
  Streaming CSV Output:    stream.csv
  Continue on Error:  true
-----------------------------------------------------------------
```
*Note: The `Output File` line will appear in the summary if the `--output-file` flag is used.*

**Results Summary (at completion):**

This summary shows the performance metrics for each successful test and clearly marks any tests that failed.

```
Benchmark Results:
-----------------------------------------------------------------
  Output Files Written:
    Streaming CSV:        stream.csv
    Log File:             ipc_benchmark.log.2025-08-05
-----------------------------------------------------------------
Mechanism: UnixDomainSocket
  Message Size: 1024 bytes
  Latency:
      Mean: 3.15 us, P95: 5.21 us, P99: 8.43 us
      Min:  1.50 us, Max: 45.12 us
  Throughput:
      Average: 310.50 MB/s, Peak: 312.80 MB/s
  Totals:
      Messages: 20000, Data: 19.53 MB
-----------------------------------------------------------------
Mechanism: SharedMemory
  Message Size: 1024 bytes
  Status: FAILED
    Error: Timed out waiting for client to connect
-----------------------------------------------------------------
```
*Note: The `Final JSON Results` line will appear in the "Output Files Written" section if the `--output-file` flag was used.*

## Performance Considerations

### System Configuration

For optimal performance measurement:

1. **CPU Frequency Scaling**: Disable frequency scaling for consistent results
```bash
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

2. **Process Isolation**: Use CPU affinity to isolate benchmark processes
```bash
taskset -c 0-3 ipc-benchmark --concurrency 4
```

3. **Memory**: Ensure sufficient RAM for shared memory tests
4. **Disk I/O**: Results may be affected by disk I/O for file operations

### Measurement Accuracy

- **Warmup**: Use warmup iterations to stabilize performance
- **Duration**: Longer test durations provide more stable results
- **Noise**: Run tests on idle systems for best accuracy
- **Repetition**: Run multiple test executions and average results

## Troubleshooting

### Common Issues

1. **Permission Errors**: Ensure write permissions for output files and temp directories
2. **Port Conflicts**: Use different ports for TCP tests if default ports are occupied
3. **Memory Issues**: Reduce buffer sizes or concurrency for memory-constrained systems
4. **Compilation Issues**: Ensure Rust toolchain is up to date

### Logging and Debugging

The benchmark provides two log streams: a user-friendly console output and a detailed diagnostic log.

- **Console Verbosity**: Control the level of detail on `stdout` with the `-v` flag.
  - `-v`: Show `DEBUG` level messages.
  - `-vv`: Show `TRACE` level messages for maximum detail.

- **Diagnostic Logs**: By default, detailed logs are saved to `ipc_benchmark.log`. Use the `--log-file` flag to customize this.

```bash
# Run with DEBUG level console output and default log file
./target/release/ipc-benchmark -v

# Run with TRACE level console output and log to a custom file
./target/release/ipc-benchmark -vv --log-file /tmp/ipc-debug.log

# Send detailed diagnostic logs to stderr instead of a file
./target/release/ipc-benchmark --log-file stderr
```

### Performance Issues

If you encounter performance issues:

1. Check system resource utilization
2. Verify no other processes are interfering
3. Consider reducing concurrency or message sizes
4. Ensure sufficient system resources

## Development

### Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# With debugging symbols
cargo build --release --debug
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test modules
cargo test metrics
cargo test ipc

# Run with output
cargo test -- --nocapture
```

### Benchmarking

```bash
# Run internal benchmarks
cargo bench

# Profile with perf
perf record --call-graph dwarf target/release/ipc-benchmark
perf report
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed contribution guidelines.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## References

- [Unix Domain Sockets](https://man7.org/linux/man-pages/man7/unix.7.html)
- [POSIX Shared Memory](https://man7.org/linux/man-pages/man7/shm_overview.7.html)
- [TCP Socket Programming](https://man7.org/linux/man-pages/man7/tcp.7.html)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for version history and changes.

---

**Note**: This benchmark suite is optimized for Linux systems. While it may work on other Unix-like systems, testing and validation have been performed primarily on RHEL 9.6. 

## Concurrency Support

**Important**: The benchmark has different concurrency behavior depending on the transport mechanism:

### Current Behavior by Transport

| Transport | Concurrency > 1 | Behavior |
|-----------|----------------|----------|
| **TCP** | ✅ Supported | Simulated concurrency (sequential tests) |
| **Unix Domain Sockets** | ✅ Supported | Simulated concurrency (sequential tests) |
| **Shared Memory** | ⚠️ **Forced to single-thread** | Automatically uses `concurrency = 1` |

### Shared Memory Limitations

**Race Condition Protection**: Shared memory automatically falls back to single-threaded mode when `--concurrency > 1` is specified to prevent race conditions and "unexpected end of file" errors.

```bash
# This command:
ipc-benchmark -m shm -c 4 -i 1000 -s 1024

# Automatically becomes:
# ipc-benchmark -m shm -c 1 -i 1000 -s 1024
# (with warning message)
```

### Why These Limitations?

- **Shared Memory**: The ring buffer implementation has inherent race conditions with multiple concurrent access
- **TCP/UDS**: True concurrent connections require complex server architecture beyond the current scope

### Buffer Size Validation

The benchmark now validates buffer sizes for shared memory to prevent buffer overflow errors:

```bash
# This will show a warning:
ipc-benchmark -m shm -i 10000 -s 1024 --buffer-size 8192

# Warning: Buffer size (8192 bytes) may be too small for 10000 messages 
# of 1024 byte messages. Consider using --buffer-size 20971520
```

### Recommended Usage

```bash
# ✅ Optimal for latency measurement
ipc-benchmark -m all -c 1 -i 10000 -s 1024

# ✅ Good for throughput analysis (TCP/UDS only)
ipc-benchmark -m tcp,uds -c 4 -i 10000 -s 1024

# ✅ Shared memory with adequate buffer
ipc-benchmark -m shm -i 10000 -s 1024 --buffer-size 50000000

# ⚠️ Will automatically use c=1 for shared memory
ipc-benchmark -m shm -c 4 -i 10000 -s 1024
```

### Error Prevention

Common issues and solutions:

1. **"Unexpected end of file"**: Increase `--buffer-size` for shared memory
2. **"Timeout sending message"**: Buffer too small, increase `--buffer-size`
3. **Hanging with concurrency**: Fixed - now uses safe fallbacks

### Performance Notes

- **Single-threaded** (`-c 1`): Most accurate latency measurements
- **Simulated concurrency** (`-c 2+`): Good for throughput scaling analysis
- **Shared memory**: Always single-threaded for reliability
