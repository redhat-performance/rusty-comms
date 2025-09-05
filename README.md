# CI TEST 3
# IPC Benchmark Suite

A comprehensive interprocess communication (IPC) benchmark suite implemented in Rust, designed to measure performance characteristics of different IPC mechanisms with a focus on latency and throughput.

## Overview

This benchmark suite provides a systematic way to evaluate the performance of various IPC mechanisms commonly used in systems programming. It's designed to be:

- **Comprehensive**: Measures both latency and throughput for one-way and round-trip communication patterns
- **Configurable**: Supports various message sizes, concurrency levels, and test durations
- **Reproducible**: Generates detailed JSON output suitable for automated analysis
- **Production-ready**: Optimized for performance measurement with minimal overhead

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

## CI/CD

This project uses GitHub Actions for continuous integration. The CI pipeline is defined in `.github/workflows/` and includes the following checks:

- **Linting and Formatting**: Ensures code style and quality using `cargo fmt` and `cargo clippy`.
- **Testing**: Runs the full test suite on stable, beta, and MSRV Rust across Linux, Windows, and macOS.
- **Code Coverage**: Generates a code coverage report using `cargo-tarpaulin` and uploads it to Codecov.
- **Security Audit**: Scans for vulnerabilities using `cargo audit`.
- **Docker Build**: Validates that the Docker image can be built and run successfully.

### Pull Request Automation

To help maintain branch quality and streamline development, the CI includes automation for pull requests:

- **Stale PR Notifier**: If a pull request becomes out-of-date with the `main` branch, a bot will post a comment to notify the author. The comment will include a list of recent commits to `main` to provide context.

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
