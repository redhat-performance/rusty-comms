# Basic Usage

This guide covers the most common command patterns for running IPC benchmarks.

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

## Next Steps

- **[Advanced Usage](advanced-usage.md)** - CPU affinity, cross-process testing
- **[IPC Mechanisms](ipc-mechanisms.md)** - Deep dive into each mechanism
- **[Output Formats](output-formats.md)** - Understanding result files
