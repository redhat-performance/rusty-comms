# CLI Reference

Complete command-line reference for `ipc-benchmark`.

## Synopsis

```
ipc-benchmark [OPTIONS]
```

## Description

A comprehensive interprocess communication benchmark suite for measuring latency and throughput across different IPC mechanisms.

## Quick Reference

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `-m` | | `uds` | IPC mechanism(s) to benchmark |
| `--message-size` | `-s` | `1024` | Message size in bytes |
| `--msg-count` | `-i` | `10000` | Number of messages |
| `--duration` | `-d` | - | Test duration |
| `--concurrency` | `-c` | `1` | Concurrent workers |
| `--blocking` | | `false` | Use blocking I/O |
| `--output-file` | `-o` | - | JSON output file |
| `--verbose` | `-v` | - | Increase verbosity |

## Options

### Basic Options

#### `-m <MECHANISMS>...`

IPC mechanisms to benchmark. Multiple mechanisms can be specified.

**Values:** `uds`, `shm`, `tcp`, `pmq`, `all`  
**Default:** `uds`

```bash
# Single mechanism
ipc-benchmark -m uds

# Multiple mechanisms
ipc-benchmark -m uds shm tcp

# All mechanisms
ipc-benchmark -m all
```

#### `-s, --message-size <BYTES>`

Message payload size in bytes.

**Default:** `1024`  
**Range:** 1 to 16MB

```bash
ipc-benchmark -m uds -s 4096
```

#### `--one-way`

Include one-way latency measurements. Measures time from send to receive.

If neither `--one-way` nor `--round-trip` is specified, both tests run.

```bash
ipc-benchmark -m uds --one-way
```

#### `--round-trip`

Include round-trip latency measurements. Measures request-response cycle time.

If neither `--one-way` nor `--round-trip` is specified, both tests run.

```bash
ipc-benchmark -m uds --round-trip
```

---

### Timing Options

#### `-i, --msg-count <COUNT>`

Number of messages to send. Ignored if `--duration` is specified.

**Default:** `10000`

```bash
ipc-benchmark -m uds -i 50000
```

#### `-d, --duration <DURATION>`

Run benchmark for a fixed duration. Takes precedence over `--msg-count`.

**Format:** Human-readable (e.g., `30s`, `5m`, `1h`)

```bash
ipc-benchmark -m uds -d 30s
ipc-benchmark -m uds -d 5m
```

#### `--send-delay <DELAY>`

Pause between sending messages. Useful for latency-focused testing.

**Format:** Human-readable (e.g., `10ms`, `50us`)

```bash
ipc-benchmark -m uds -i 1000 --send-delay 10ms
```

#### `-w, --warmup-iterations <COUNT>`

Number of warmup messages before measurement begins.

**Default:** `1000`

```bash
ipc-benchmark -m uds -i 10000 -w 5000
```

---

### Concurrency Options

#### `-c, --concurrency <COUNT>`

Number of concurrent workers.

**Default:** `1`

Note: Shared memory forces `concurrency = 1` to avoid race conditions.

```bash
ipc-benchmark -m tcp -c 4
```

#### `--server-affinity <CORE>`

Pin the server (message receiver) process to a specific CPU core.

```bash
ipc-benchmark -m uds --server-affinity 0
```

#### `--client-affinity <CORE>`

Pin the client (message sender) process to a specific CPU core.

```bash
ipc-benchmark -m uds --client-affinity 1
```

---

### Output and Logging Options

#### `-o, --output-file [<FILE>]`

Path to JSON output file for aggregated results.

**Default filename:** `benchmark_results.json` (when flag used without path)

```bash
# Explicit filename
ipc-benchmark -m uds -o results.json

# Default filename
ipc-benchmark -m uds -o
```

#### `-q, --quiet`

Suppress all informational console output. Only shows errors.

```bash
ipc-benchmark -m uds -i 10000 -o results.json -q
```

#### `-v, --verbose`

Increase diagnostic log verbosity. Can be used multiple times.

| Level | Flag | Verbosity |
|-------|------|-----------|
| Default | (none) | Warnings and errors |
| Info | `-v` | Info messages |
| Debug | `-vv` | Debug messages |
| Trace | `-vvv` | Maximum detail |

```bash
ipc-benchmark -m uds -v    # Info
ipc-benchmark -m uds -vv   # Debug
ipc-benchmark -m uds -vvv  # Trace
```

#### `--log-file <PATH | stderr>`

Path to log file for detailed diagnostics.

**Default:** `ipc_benchmark.log`  
**Special value:** `stderr` sends logs to standard error

```bash
ipc-benchmark -m uds --log-file debug.log
ipc-benchmark -m uds --log-file stderr
```

#### `--streaming-output-json [<FILE>]`

JSON file for real-time per-message latency data.

**Default filename:** `benchmark_streaming_output.json`

```bash
ipc-benchmark -m uds --streaming-output-json stream.json
```

#### `--streaming-output-csv [<FILE>]`

CSV file for real-time per-message latency data.

**Default filename:** `benchmark_streaming_output.csv`

```bash
ipc-benchmark -m uds --streaming-output-csv stream.csv
```

---

### Advanced Options

#### `--continue-on-error`

Continue running remaining benchmarks even if one mechanism fails.

```bash
ipc-benchmark -m all --continue-on-error
```

#### `--percentiles <VALUES>`

Percentile values to calculate and report.

**Default:** `50 95 99 99.9`

```bash
ipc-benchmark -m uds --percentiles 50 75 90 95 99 99.5 99.9 99.99
```

#### `--buffer-size <BYTES>`

Buffer size for message queues and shared memory.

**Default:** Auto-calculated (PMQ defaults to 8192 for OS compatibility)

```bash
ipc-benchmark -m shm --buffer-size 1048576
```

#### `--host <ADDRESS>`

Host address for TCP sockets.

**Default:** `127.0.0.1`

```bash
ipc-benchmark -m tcp --host 0.0.0.0
```

#### `--port <PORT>`

Port number for TCP sockets.

**Default:** `8080`

```bash
ipc-benchmark -m tcp --port 9090
```

#### `--pmq-priority <PRIORITY>`

Message priority for POSIX Message Queues.

**Default:** `0`

```bash
ipc-benchmark -m pmq --pmq-priority 5
```

#### `--include-first-message`

Include the first message in results. By default, the first message is discarded to avoid cold-start latency spikes.

```bash
ipc-benchmark -m uds --include-first-message
```

#### `--blocking`

Use synchronous/blocking I/O instead of async (Tokio).

**Default:** `false` (async mode)

```bash
ipc-benchmark -m uds --blocking
```

#### `--shm-direct`

Use high-performance direct memory shared memory. Auto-enables `--blocking`.

**Platform:** Unix only  
**Message size:** Fixed (8KB max)

```bash
ipc-benchmark -m shm --shm-direct
```

#### `--cross-container`

Force cross-container mode for shared memory. Usually auto-detected.

```bash
ipc-benchmark -m shm --run-mode client --blocking --cross-container
```

#### `--run-mode <MODE>`

Process architecture mode.

**Values:**
- `standalone` - Both processes on same host (default)
- `client` - Act as IPC server (receiver)
- `sender` - Act as IPC client (sender)

**Default:** `standalone`

```bash
# Server (in container)
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0

# Client (on host)
ipc-benchmark -m tcp --run-mode sender --blocking --host <ip>
```

---

### Help and Version

#### `-h, --help`

Print help information.

```bash
ipc-benchmark --help
ipc-benchmark -h  # Summary
```

#### `-V, --version`

Print version information.

```bash
ipc-benchmark --version
```

---

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Logging level filter | `info` |

---

## Examples

### Basic Benchmark

```bash
ipc-benchmark -m uds -i 10000
```

### Full Comparison

```bash
ipc-benchmark -m all -i 50000 \
  -o comparison.json \
  --streaming-output-json streaming.json \
  --continue-on-error
```

### High-Precision Latency

```bash
ipc-benchmark -m shm --shm-direct \
  -s 64 -i 100000 -w 10000 \
  --server-affinity 0 --client-affinity 1 \
  --percentiles 50 90 95 99 99.9 99.99 \
  -o latency.json
```

### Duration-Based Test

```bash
ipc-benchmark -m uds -d 5m \
  -o results.json \
  --streaming-output-csv stream.csv
```

### Cross-Process Testing

```bash
# Terminal 1: Receiver
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0

# Terminal 2: Sender
ipc-benchmark -m tcp --run-mode sender --blocking \
  --host 127.0.0.1 -i 10000 -o results.json
```

---

## See Also

- [Getting Started](../user-guide/getting-started.md)
- [Basic Usage](../user-guide/basic-usage.md)
- [Advanced Usage](../user-guide/advanced-usage.md)
- [Troubleshooting](../user-guide/troubleshooting.md)
