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
- **Streaming JSON**: Real-time, per-message latency data written
  to a file in a columnar JSON format. This allows for efficient,
  live monitoring of long-running tests. The format consists of a
  `headings` array and a `data` array containing value arrays for
  each message. See [Streaming Output Columns](#streaming-output-columns)
  for the column definitions.
- **Streaming CSV**: Real-time, per-message latency data written
  to a standard CSV file. The columns match the streaming JSON
  headings. This format is ideal for easy import into spreadsheets
  and data analysis tools.
- **Console Output**: User-friendly, color-coded summaries on `stdout`. Includes a configuration summary at startup and a detailed results summary upon completion.
- **Detailed Logs**: Structured, timestamped logs written to a file or `stderr` for diagnostics.

## Execution Modes: Async vs. Blocking

This benchmark suite supports **two distinct execution modes**: **async** (default) and **blocking**. Both modes coexist in the same binary, enabling direct performance comparison between asynchronous and synchronous I/O approaches.

### Async Mode (Default)

**Technology:** Tokio runtime with async/await  
**Best For:** Production-grade async systems, high-concurrency scenarios

Async mode uses Rust's async/await syntax with the Tokio runtime. This is the default behavior and matches most modern Rust networking applications.

```bash
# Run in async mode (default)
./target/release/ipc-benchmark -m uds --message-size 1024 --msg-count 10000
```

**Characteristics:**
- Uses `tokio::spawn` for tasks
- Non-blocking I/O operations
- Efficient for high-concurrency workloads
- Matches real-world async application behavior

### Blocking Mode

**Technology:** Pure standard library with blocking I/O  
**Best For:** Synchronous systems, baseline performance comparison

Blocking mode uses only the Rust standard library (`std::net`, `std::thread`, `std::fs`) without any async runtime overhead. Enable it with the `--blocking` flag.

```bash
# Run in blocking mode
./target/release/ipc-benchmark -m uds --message-size 1024 --msg-count 10000 --blocking
```

**Characteristics:**
- Uses `std::thread` for concurrency
- Blocking I/O operations
- No Tokio runtime overhead
- Simpler execution model

### When to Use Each Mode

| Scenario | Recommended Mode | Reason |
|----------|------------------|--------|
| **Comparing I/O models** | Both | Direct performance comparison |
| **Testing async systems** | Async | Matches your production code |
| **Testing blocking systems** | Blocking | Matches your production code |
| **Minimal overhead baseline** | Blocking | No runtime complexity |
| **High concurrency (>100 connections)** | Async | More efficient resource usage |
| **Simple request-response** | Either | Similar performance characteristics |

## Shared Memory Implementations

The benchmark suite provides **two shared memory implementations** for blocking mode, each optimized for different use cases:

### Ring Buffer (Default)

The default implementation uses a ring buffer with bincode serialization. It offers flexibility and cross-platform support.

```bash
# Ring buffer with blocking mode
ipc-benchmark -m shm --blocking -i 10000

# Ring buffer with async mode (default)
ipc-benchmark -m shm -i 10000
```

**Characteristics:**
- Works on all platforms (Linux, macOS, Windows, BSD)
- Supports variable message sizes
- Uses bincode serialization (~15-30 μs overhead)
- Average latency: ~20 μs

### Direct Memory (`--shm-direct`)

The high-performance implementation uses direct memory access with no serialization overhead. The `--shm-direct` flag automatically enables blocking mode.

```bash
# Direct memory (auto-enables blocking mode)
ipc-benchmark -m shm --shm-direct -i 10000

# With CPU affinity for best results
ipc-benchmark -m shm --shm-direct -i 10000 --server-affinity 0 --client-affinity 1
```

**Characteristics:**
- Unix-only (Linux, macOS, BSD - not Windows)
- Fixed message size (8KB maximum payload)
- No serialization (direct memcpy)
- Average latency: ~7 μs (3× faster)
- Maximum latency: ~22 μs (450× better than ring buffer)

### Implementation Comparison

| Feature | Ring Buffer (Default) | Direct Memory (`--shm-direct`) |
|---------|----------------------|-------------------------------|
| **Average Latency** | ~20 μs | ~7 μs (3× faster) |
| **Max Latency** | ~10 ms | ~22 μs (450× better) |
| **Serialization** | bincode | None (memcpy) |
| **Message Size** | Variable | Fixed (8KB max) |
| **Platform Support** | All | Unix only |
| **Best For** | Flexibility, cross-platform | Maximum performance |

### When to Use Each Implementation

**Use Direct Memory (`--shm-direct`) when:**
- Maximum performance is critical
- Fixed message sizes are acceptable
- Running on Unix/Linux systems

**Use Ring Buffer (default) when:**
- Cross-platform support is needed
- Variable message sizes are required
- Flexibility is more important than raw speed
- Windows support is required

For detailed technical comparison, see [SHM_COMPARISON.md](SHM_COMPARISON.md).

## Latency Measurement Methodology

This benchmark suite uses **high-precision monotonic clocks** to measure true IPC latency, matching the methodology used in C-based benchmark programs. This ensures accurate, reproducible measurements that are immune to system clock adjustments.

### Timing Architecture

#### Clock Source

- **Unix/Linux**: Uses `CLOCK_MONOTONIC` via the nix crate
- **Windows**: Falls back to system time (less precise)
- **Characteristics**: Monotonic clocks measure time from system boot and are unaffected by NTP adjustments, daylight saving time, or manual clock changes

#### Test Execution Order

One-way and round-trip tests run **sequentially**, never
simultaneously. Each test spawns its own server process, runs to
completion, and tears down before the next test starts:

1. **Warmup** (if configured)
2. **One-way test** — client sends messages; server measures
   receive latency
3. **Round-trip test** — client sends a message, waits for the
   server's response, then sends the next

By default both tests are enabled. Use `--one-way` or
`--round-trip` to run only one. For duration-based tests (`-d`),
each test gets its own full time window, so one-way typically
produces far more records than round-trip (fire-and-forget vs
send-wait-receive per message).

#### Timestamp Capture Points

**For One-Way Latency Tests:**
1. **Send Side**: Timestamp captured **immediately before serialization** in `send_blocking()`
2. **Receive Side**: Timestamp captured **immediately after receiving** the message
3. **Latency Calculation**: `receive_time - send_time` (both in nanoseconds)

**For Round-Trip Latency Tests:**
1. **Client Side**: Timestamp captured before `send_blocking()`
2. **Server Side**: Receives request, sends response immediately
3. **Client Side**: Timestamp captured after receiving response
4. **Latency Calculation**: Total elapsed time from send to receive

#### Streaming Output Columns

The per-message streaming output (JSON and CSV) contains the
following columns:

| Column | Type | Description |
|--------|------|-------------|
| `timestamp_ns` | `u64` | Approximate wall-clock time (nanoseconds since Unix epoch) when the message was sent. For round-trip and combined tests this is captured on the client immediately before `send()`. For one-way tests the server approximates it as `wall_clock_now - latency`. |
| `message_id` | `u64` | Zero-based sequential message identifier. |
| `mechanism` | `string` | IPC mechanism name (e.g. `"TcpSocket"`, `"UnixDomainSocket"`). |
| `message_size` | `u64` | Payload size in bytes. |
| `one_way_latency_ns` | `u64` or `null` | One-way latency in nanoseconds, or `null` if this record is round-trip only. |
| `round_trip_latency_ns` | `u64` or `null` | Round-trip latency in nanoseconds, or `null` if this record is one-way only. |

> **Note on `timestamp_ns` accuracy:** For one-way tests the
> server computes the send timestamp by subtracting the measured
> monotonic latency from its current wall-clock time. This mixes
> two clock domains (wall-clock and monotonic) and is subject to
> minor drift from NTP adjustments, but it is the best
> approximation available without clock synchronization between
> the client and server processes.

### What's Measured

The latency measurements include:
- ✅ **Pure IPC transit time** (message traveling through the IPC mechanism)
- ✅ **Serialization overhead** (bincode encoding/decoding)
- ✅ **Kernel context switches** (for mechanisms like Unix domain sockets)
- ✅ **Queue/buffer operations** (for message queues and shared memory)

The latency measurements exclude:
- ❌ Message construction time (before timestamp capture)
- ❌ Application processing logic
- ❌ Memory allocation for payloads

### Timing Methodology

The benchmark captures timestamps immediately before the IPC syscall:

```rust
// Create message with monotonic timestamp right before serialization
let mut message = message.clone();
message.set_timestamp_now();  // Uses get_monotonic_time_ns()
let serialized = bincode::serialize(&message)?;
stream.write_all(&serialized)?;

// On receive side
let receive_time_ns = get_monotonic_time_ns();
let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
```

### Accuracy Considerations

**Why This Matters:**
- Previous implementations that captured timestamps during `Message::new()` included unnecessary overhead before IPC operations
- Using monotonic clocks prevents anomalies from system time adjustments during long-running benchmarks
- Capturing timestamps right before serialization ensures measurements reflect actual IPC performance

**Expected Latency Ranges** (on modern hardware):
- Unix Domain Sockets: 2-10 µs
- TCP Localhost: 5-20 µs  
- Shared Memory: 1-5 µs
- POSIX Message Queues: 5-15 µs

### Performance Comparison Methodology

To fairly compare async vs. blocking performance:

1. **Use identical test parameters:**
```bash
# Async test
./target/release/ipc-benchmark -m tcp --message-size 1024 --msg-count 50000 \
  --output-file async_results.json

# Blocking test
./target/release/ipc-benchmark -m tcp --message-size 1024 --msg-count 50000 \
  --blocking --output-file blocking_results.json
```

2. **Control for system variability:**
   - Run tests on an idle system
   - Use CPU affinity (`--server-affinity` for receiver, `--client-affinity` for sender)
   - Run multiple iterations and average results
   - Use warmup iterations (`--warmup-iterations 1000`)

3. **Analyze the results:**
```bash
# Compare latency distributions
cat async_results.json | jq '.results[0].one_way_results.latency'
cat blocking_results.json | jq '.results[0].one_way_results.latency'

# Compare throughput
cat async_results.json | jq '.results[0].one_way_results.throughput'
cat blocking_results.json | jq '.results[0].one_way_results.throughput'
```

### Example: Side-by-Side Comparison

```bash
# Test all mechanisms in both modes
for mode in "" "--blocking"; do
  mode_name=$([ -z "$mode" ] && echo "async" || echo "blocking")
  
  ./target/release/ipc-benchmark \
    -m uds tcp shm \
    --message-size 1024 \
    --msg-count 50000 \
    --warmup-iterations 1000 \
    --percentiles 50 95 99 99.9 \
    $mode \
    --output-file ${mode_name}_comparison.json
done

# Results in: async_comparison.json and blocking_comparison.json
```

### Implementation Details

**Async Mode (`src/benchmark.rs`):**
- Uses `tokio::spawn` for concurrent tasks
- `async fn` with `.await` points
- `tokio::net::TcpListener`, `tokio::fs::File`
- Non-blocking operations throughout

**Blocking Mode (`src/benchmark_blocking.rs`):**
- Uses `std::thread::spawn` for concurrency
- Pure synchronous functions (no `async`)
- `std::net::TcpListener`, `std::fs::File`
- Blocking operations throughout
- No Tokio dependency in execution path

### Backward Compatibility

**The `--blocking` flag is completely optional.** If omitted, the benchmark runs in async mode exactly as before. This ensures no breaking changes for existing users and scripts.

## Installation

### Prerequisites

- **Rust**: 1.70.0 or later (MSRV)
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
# Run benchmark with default settings (async mode, prints summary to console)
ipc-benchmark

# Run in blocking mode
ipc-benchmark --blocking

# Run with specific mechanisms
ipc-benchmark -m uds shm pmq

# Run in blocking mode with specific mechanisms
ipc-benchmark -m tcp uds --blocking

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

# Run only round-trip tests (one-way and round-trip run
# sequentially by default; use these flags to select one)
ipc-benchmark --round-trip --no-one-way

# Custom percentiles for latency analysis
ipc-benchmark --percentiles 50 90 95 99 99.9 99.99

# TCP-specific configuration
ipc-benchmark -m tcp --host 127.0.0.1 --port 9090

# POSIX Message Queue-specific configuration
ipc-benchmark -m pmq --pmq-priority 1

# Shared memory configuration (demonstrating a user-provided buffer size)
ipc-benchmark -m shm --buffer-size 16384
```

### Process-based client/server model

This benchmark runs the server as a separate child process for each test to ensure strong isolation and realistic IPC behavior.

- The parent process spawns the same binary in a special "server-only" mode and waits for a readiness byte via a pipe connected to the child's stdout.
- On Unix, the readiness signal is a single byte `0x01` written to stdout. On Windows, tests use a simple `echo` to emit a single character (e.g., `R`).
- The child process is terminated at the end of each test; resources are cleaned up by the transport implementation.

Binary resolution strategy used by the spawner:
- If the current executable already matches the binary name, it is used directly.
- If `CARGO_BIN_EXE_ipc-benchmark` (or `CARGO_BIN_EXE_ipc_benchmark`) is set, that path is used.
- Otherwise it falls back to `target/debug/ipc-benchmark` on Unix, and `target/debug/ipc-benchmark.exe` on Windows.

Tip: If you see "Could not resolve 'ipc-benchmark' binary for server mode", run:

```bash
cargo build --bin ipc-benchmark
# or simply
cargo test --all-features
```

### CPU affinity controls

You can pin the server (message receiver) and/or client (message sender) workload to specific CPU cores to reduce jitter and improve reproducibility:

```bash
# Pin server (receiver) to core 2 and client (sender) to core 5
ipc-benchmark --server-affinity 2 --client-affinity 5 -m uds --msg-count 20000
```

Notes:
- `--server-affinity`: Pins the message **receiver** process to the specified CPU core
- `--client-affinity`: Pins the message **sender** process to the specified CPU core
- Affinity is implemented via the `core_affinity` crate. The semantics are best-effort and depend on OS support.
- On multi-core systems, pinning can reduce cross-core migration and improve latency consistency.

### Platform notes

- Unix Domain Sockets (UDS) are available only on Unix-like systems.
- POSIX Message Queues (PMQ) are available only on Linux and may require `/dev/mqueue` to be mounted and proper limits.
- Windows: the spawned server binary must be discoverable; the code handles `.exe` resolution and environment-path hints as described above.
- macOS: time-based latency tests may see higher scheduler jitter; certain tests relax strict upper bounds to prevent flakiness.

### First-Message Latency (Canary)

The first message in any benchmark often has a higher latency than subsequent messages due to "cold start" effects like CPU cache misses, memory allocation, and branch prediction misses. To provide more stable and representative results, this tool automatically sends one "canary" message before starting the measurement loop. This message and its latency are discarded by default.

If you need to analyze the raw performance data, including the first-message spike, you can use the `--include-first-message` flag to disable this behavior.

```bash
# Include the first message in the final results
ipc-benchmark --include-first-message
### Understanding Test Types: Throughput vs. Latency

This benchmark suite can be used to measure two primary aspects of IPC performance: **throughput** and **latency**. The configuration you choose will determine which of these you are primarily testing.

#### Throughput-Focused Testing (Default Behavior)

By default, this tool is a **throughput benchmark**. It attempts to send messages as fast as the system will allow, with no artificial delays. This is ideal for answering questions like:

-   "What is the maximum message rate this IPC mechanism can handle?"
-   "How many megabytes per second can I transfer?"

This mode is most useful for stress-testing the system and finding its performance limits under heavy load.

```bash
# Throughput test: Send 100,000 messages as fast as possible
./target/release/ipc-benchmark -m uds -i 100000 -s 4096
```

#### Latency-Focused Testing (Using `--send-delay`)

Sometimes, you are more interested in the latency of individual messages under a controlled, predictable load, rather than the maximum possible throughput. The `--send-delay` flag allows you to introduce a fixed pause between each message sent.

This is a **latency-focused test**. It is ideal for answering questions like:

-   "When my application sends a message every 10 milliseconds, what is the typical time it takes for that message to be delivered?"
-   "Does latency remain stable or does it spike under a consistent, non-maximal load?"

This mode simulates applications that are not constantly sending data at maximum speed, which is a very common real-world scenario.

```bash
# Latency test: Send messages with a 10 millisecond delay between each one
./target/release/ipc-benchmark \
  -m pmq \
  -i 10000 \
  -s 116 \
  --send-delay 10ms
```

By using `--send-delay`, you can more accurately measure the base "travel time" of a message without the confounding factor of queue backpressure that occurs during high-throughput tests.

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
  --percentiles 50 95 99 99.9 99.99 \
  --send-delay 10ms
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
        "average_throughput_megabytes_per_sec": 305.17,
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
  Buffer Size:  8192 bytes
  One-Way Latency:
      Mean: 3.15 us, P95: 5.21 us, P99: 8.43 us
      Min:  1.50 us, Max: 45.12 us
  Round-Trip Latency:
      Mean: 5.82 us, P95: 9.11 us, P99: 14.50 us
      Min:  4.20 us, Max: 88.30 us
  Throughput:
      Average: 155.30 MB/s, Peak: 156.40 MB/s
  Totals:
      Messages: 20000, Data: 19.53 MB
-----------------------------------------------------------------
Mechanism: SharedMemory
  Message Size: 1024 bytes
  Buffer Size:  65536 bytes
  Status: FAILED
    Error: Timed out waiting for client to connect
-----------------------------------------------------------------
```
*Note: The `Final JSON Results` line will appear in the "Output Files Written" section if the `--output-file` flag was used.*

## Result Analysis

### Interactive Dashboard

For comprehensive analysis and visualization of benchmark results, use the included **Performance Dashboard** - a web application designed for IPC performance analysis:

```bash
# Start the interactive dashboard
python utils/dashboard/dashboard.py --dir /path/to/results --host 0.0.0.0 --port 8050
```
**Analysis Workflows:**
- **Summary Analysis**: Performance overview cards, head-to-head comparisons, and statistical breakdowns
- **Time Series Analysis**: Temporal patterns, anomaly detection, and moving averages with 5 preset configurations
- **Interactive Exploration**: Filter by mechanism/message size, drill down into specific test runs

**Key Features:**
- **Advanced Visualizations**: Interactive time-series plots, statistical overlays, and comparative analysis
- **Intelligent Insights**: AI-powered performance recommendations and mechanism comparisons  
- **Modern UI**: interface with preset configurations for different analysis scenarios
- **High Performance**: Handles millions of data points with threaded processing and smart caching
- **Export Capabilities**: Generate reports and capture insights for documentation

The dashboard automatically discovers all JSON and CSV output files in your results directory and provides powerful tools for understanding IPC performance characteristics across different scenarios.

For detailed dashboard documentation and setup instructions, see [`utils/dashboard/README.md`](utils/dashboard/README.md).

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

### Backpressure Warnings

During high-throughput tests, it's possible for the sender to produce data faster than the receiver can consume it. When the underlying OS buffers or message queues become full, the send operation will slow down or block. This condition is known as **backpressure**.

This benchmark tool automatically detects backpressure and will issue a warning message the first time it occurs for a given transport, for example:

```
WARN rusty_comms::ipc::shared_memory: Shared memory buffer is full; backpressure is occurring. 
This may impact latency and throughput measurements. 
Consider increasing the buffer size if this is not the desired scenario.
```

When you see this warning, it means your benchmark may be measuring a backpressure-limited scenario rather than the pure, unconstrained performance of the IPC mechanism. This is a valid and important scenario to test, but it's crucial for interpreting the results correctly. If your goal is to measure maximum throughput, you may need to increase the buffer sizes (`--buffer-size`) or investigate the receiver's performance.

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

### Local Validation

Before submitting contributions, it's important to run the same checks that our CI system uses. This ensures your changes will be accepted smoothly. For detailed instructions, please see the [CONTRIBUTING.md](CONTRIBUTING.md) file.

The primary checks are:
- **Formatting**: `cargo fmt --all`
- **Linting**: `cargo clippy --all-targets --all-features -- -D warnings`
- **Testing**: `cargo test --verbose --all-features`

### Repo-managed pre-commit hooks

This repository maintains its pre-commit checks in `.githooks/pre-commit` so everyone runs the same gates locally.

Enable them once per clone:

```bash
# Option A: one-liner
git config core.hooksPath .githooks && chmod +x .githooks/pre-commit

# Option B: helper script
bash scripts/install-git-hooks.sh
```

What runs on commit:

- `cargo check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo fmt -- --check`
- `cargo test`
- `scripts/msrv-check.sh` — builds/tests with Rust 1.70 in a container when `podman` or `docker` is available; otherwise it is skipped.

You can run these manually anytime to reproduce the exact gate.

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
- **Code Coverage**: Generates a code coverage report using `cargo-tarpaulin`.
- **Security Audit**: Scans for vulnerabilities using `cargo audit`.
- **Docker Build**: Validates that the Docker image can be built and run successfully.

### Pull Request Automation

To help maintain branch quality and streamline development, the CI includes automation for pull requests:

- **Stale PR Notifier**: If a pull request becomes out-of-date with the `main` branch, a bot will post a comment to notify the author. The comment will include a list of recent commits to `main` to provide context.

## Dashboard Integration

The Rusty-Comms Dashboard provides powerful visualization and analysis capabilities for benchmark results. However, it requires specific output files to function properly.

### Prerequisites for Dashboard Usage

The dashboard requires **both** summary and streaming data files from ipc-benchmark:

- **Summary JSON**: Contains aggregated performance metrics and statistical analysis
- **Streaming JSON**: Contains individual message latency data for time series analysis

### Required ipc-benchmark Parameters

To generate dashboard-compatible output, you **must** include both output parameters:

```bash
# Minimum command for dashboard compatibility
./ipc-benchmark --mechanism <MECHANISM> --message-size <SIZE> \
                 -o results/ \
                 --streaming-output-json \
                 --continue-on-error

# Example with specific values
./ipc-benchmark --mechanism SharedMemory --message-size 1024 \
                 -o ./benchmark_results/ \
                 --streaming-output-json \
                 --duration 30s
```

### File Output Expectations

After running with the required parameters, you should see these files:

```
results/
├── sharedmemory_1024_summary.json     # Enables Summary Analysis
└── sharedmemory_1024_streaming.json   # Enables Time Series Analysis
```

### Dashboard Parameter Reference

| Parameter | Required | Purpose | Dashboard Impact |
|-----------|----------|---------|------------------|
| `-o <dir>` | **Yes** | Output directory | Summary data location |
| `--streaming-output-json` | **Yes** | Enable streaming data | Time series analysis |
| `--mechanism <type>` | **Yes** | IPC mechanism | Data categorization |
| `--message-size <bytes>` | **Yes** | Message size | Performance comparison |
| `--duration <time>` | Recommended | Test duration | Data volume |
| `--continue-on-error` | Recommended | Continue if one test fails | Complete dataset |

### Quick Start: Dashboard-Ready Benchmarks

#### Single Mechanism Test
```bash
./ipc-benchmark --mechanism SharedMemory \
                 --message-size 1024 \
                 -o ./dashboard_data/ \
                 --streaming-output-json \
                 --duration 30s
```

#### Multi-Size Comparison Test
```bash
for size in 64 256 1024 4096; do
  ./ipc-benchmark --mechanism SharedMemory \
                   --message-size $size \
                   -o ./dashboard_data/ \
                   --streaming-output-json \
                   --duration 10s
done
```

#### Multi-Mechanism Comparison
```bash
for mechanism in uds shm tcp pmq; do
  ./ipc-benchmark --mechanism $mechanism \
                   --message-size 1024 \
                   -o ./dashboard_data/ \
                   --streaming-output-json \
                   --duration 15s
done
```

### Launch Dashboard
```bash
cd utils/dashboard
python3 dashboard.py --dir ../../dashboard_data/
```

### Troubleshooting Dashboard Issues

#### "No Data Available" Error
**Cause**: Missing streaming data files  
**Solution**: Ensure ipc-benchmark runs with `--streaming-output-json`

#### Dashboard Shows Only Summary Data  
**Cause**: Missing streaming JSON files  
**Impact**: Time Series analysis will be unavailable  
**Solution**: Re-run benchmarks with streaming output enabled

#### Empty Directory on Startup
**Cause**: No benchmark result files in the specified directory  
**Solution**: Run ipc-benchmark with the required parameters first, then start dashboard

For detailed dashboard documentation and advanced features, see [`utils/dashboard/README.md`](utils/dashboard/README.md).

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

### Buffer Size Configuration

The `--buffer-size` flag controls the size of internal buffers.
When omitted, each mechanism receives an appropriate automatic
default. When provided, the user-specified value applies to all
mechanisms.

**Automatic Sizing (Default):**

| Mechanism | Auto Buffer Size | Rationale |
|-----------|-----------------|-----------|
| **SHM** | 64 KB (or 2x message size) | Fixed buffer enables streaming; writer blocks when full |
| **PMQ** | 8,192 bytes | Safe default within common OS limits |
| **TCP/UDS** (duration mode) | 1 GB | Avoids backpressure during timed runs |
| **TCP/UDS** (msg-count mode) | `msg_count * (msg_size + 64)` | Sized to fit all messages |

**User-Provided Size:** You can override the automatic default
with `--buffer-size <bytes>` to test specific backpressure
scenarios. This is a valid testing approach — the tool will log
a message when the buffer is smaller than the total data volume.

```bash
# Override buffer to test backpressure on TCP:
ipc-benchmark -m tcp -i 10000 -s 1024 --buffer-size 8192
```

### Error Prevention

Common issues and solutions:

1. **"Timeout sending message"**: Buffer too small for the
   mechanism; increase `--buffer-size` or let automatic sizing
   handle it
2. **"Timeout waiting for buffer space"**: SHM reader fell too
   far behind the writer; this can occur with very large message
   sizes or slow consumers
3. **Hanging with concurrency**: Fixed — now uses safe fallbacks

### Performance Notes

- **Single-threaded** (`-c 1`): Most accurate latency measurements
- **Simulated concurrency** (`-c 2+`): Good for throughput scaling analysis
- **Shared memory**: Always single-threaded for reliability
