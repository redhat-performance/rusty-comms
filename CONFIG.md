# Configuration Guide

This document provides detailed information about configuring the IPC Benchmark Suite for various testing scenarios and environments.

## Table of Contents

- [Command Line Options](#command-line-options)
- [Shell Aliases for Common Configurations](#shell-aliases-for-common-configurations)
- [Environment Variables](#environment-variables)
- [IPC Mechanism Settings](#ipc-mechanism-settings)
- [Performance Tuning](#performance-tuning)
- [Test Scenarios](#test-scenarios)
- [System Requirements](#system-requirements)

## Command Line Options

### Basic Options

| Option | Short | Type | Default | Description |
|--------|-------|------|---------|-------------|
| `-m` | | String[] | `[uds]` | IPC mechanisms to benchmark (uds, shm, tcp, pmq, all) |
| `--message-size` | `-s` | Number | `1024` | Message size in bytes |
| `--msg-count` | `-i` | Number | `10000` | Number of messages to send |
| `--duration` | `-d` | String | - | Duration to run (e.g., "30s", "5m") |
| `--concurrency` | `-c` | Number | `1` | Number of concurrent workers |
| `--output-file` | `-o` | String | *(none; optional flag)* | JSON output file path |

### Test Configuration

One-way and round-trip tests run **sequentially**, not
simultaneously. Each test spawns its own server process, runs to
completion, and tears down before the next test starts. By default
both are enabled. For duration-based tests (`-d`), each test gets
its own full time window.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `--one-way` | Boolean | `true` | Enable one-way latency tests |
| `--round-trip` | Boolean | `true` | Enable round-trip latency tests |
| `--warmup-iterations` | `-w` | Number | `1000` | Number of warmup iterations |
| `--percentiles` | Number[] | `[50.0, 95.0, 99.0, 99.9]` | Percentiles to calculate |
| `--buffer-size` | Number | *auto* | Override buffer size (auto-sized per mechanism when omitted) |

### Advanced Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `--streaming-output-json` | String | - | JSON file for streaming per-message results |
| `--streaming-output-csv` | String | - | CSV file for streaming per-message results |
| `--continue-on-error` | Boolean | `false` | Continue running if one test fails |
| `--verbose` | `-v` | Flag | *(repeatable)* | Increase log verbosity (-v=DEBUG, -vv=TRACE) |
| `--quiet` | `-q` | Boolean | `false` | Suppress stdout output |
| `--log-file` | String | `ipc_benchmark.log` | Log file path (or "stderr") |
| `--host` | String | `"127.0.0.1"` | Host address for TCP sockets |
| `--port` | Number | `8080` | Port for TCP sockets |
| `--pmq-priority` | Number | `0` | Message priority for PMQ |
| `--send-delay` | String | - | Delay between messages (e.g., "10ms") |
| `--include-first-message` | Boolean | `false` | Include first message in results |
| `--blocking` | Boolean | `false` | Use blocking I/O instead of async |
| `--shm-direct` | Boolean | `false` | Use direct memory SHM (auto-enables blocking) |
| `--server-affinity` | Number | - | Pin the server process (message receiver) to a CPU core |
| `--client-affinity` | Number | - | Pin the client workload (message sender) to a CPU core |
| `--server` | Boolean | `false` | Run in standalone server mode |
| `--client` | Boolean | `false` | Run in standalone client mode |

### Examples

```bash
# Basic usage
ipc-benchmark -m uds shm pmq --message-size 4096 --msg-count 50000

# Test all mechanisms (including PMQ)
ipc-benchmark -m all --message-size 1024 --msg-count 10000

# Test POSIX message queues specifically
ipc-benchmark -m pmq --message-size 2048 --msg-count 5000

# Duration-based testing
ipc-benchmark --duration 60s --concurrency 8

# Comprehensive latency analysis
ipc-benchmark --percentiles 50 90 95 99 99.9 99.99 --warmup-iterations 10000

# High-throughput testing
ipc-benchmark -m shm --message-size 65536 --buffer-size 1048576
```

## Shell Aliases for Common Configurations

Configuration files are not currently supported. Instead, use shell
aliases or scripts for frequently used parameter sets:

```bash
# Latency-focused alias
alias ipc-latency='ipc-benchmark -m uds --message-size 64 \
  --msg-count 100000 --warmup-iterations 10000 --send-delay 10ms'

# Throughput-focused alias
alias ipc-throughput='ipc-benchmark -m shm --message-size 65536 \
  --duration 60s'

# Full comparison alias
alias ipc-compare='ipc-benchmark -m all --message-size 1024 \
  --msg-count 50000 --output-file comparison.json'
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `CARGO_BIN_EXE_ipc-benchmark` | Path hint for the test runner to spawn the server binary | Auto-detected |
| `CARGO_BIN_EXE_ipc_benchmark` | Alternate env var name used in some setups | Auto-detected |

Note: Logging verbosity is controlled via the `-v` CLI flag (not
`RUST_LOG`). Use `-v` for DEBUG, `-vv` for TRACE level output.

## IPC Mechanism Settings

### Unix Domain Sockets (UDS)

| Setting | Description | Default | Range |
|---------|-------------|---------|-------|
| `socket_path` | Path to socket file | `/tmp/ipc_benchmark_<uuid>.sock` | Valid file path |
| `buffer_size` | Socket buffer size | `8192` | 1024 - 1MB |

**Optimal Settings:**
- Message size: 64 - 4096 bytes for lowest latency
- Concurrency: 1-4 for latency testing, up to CPU cores for throughput
- Buffer size: 8192 - 65536 bytes

### Shared Memory (SHM)

| Setting | Description | Default | Range |
|---------|-------------|---------|-------|
| `shared_memory_name` | Shared memory segment name | `ipc_benchmark_<uuid>` | Valid identifier |
| `buffer_size` | Ring buffer size | 64 KB (auto) | 4096 - 1GB |

**Automatic Buffer Sizing:** When `--buffer-size` is omitted,
SHM uses a fixed 64 KB ring buffer (or 2x message size for
large messages). This enables proper streaming where the writer
blocks when the buffer is full, rather than dumping all data at
once into an oversized buffer.

**Optimal Settings:**
- Message size: 1024 - 65536 bytes for highest throughput
- Concurrency: 1 (single producer, single consumer)
- Buffer size: Leave at automatic (64 KB) for streaming;
  override with `--buffer-size` only to test specific
  backpressure scenarios

### TCP Sockets

| Setting | Description | Default | Range |
|---------|-------------|---------|-------|
| `host` | Server host address | `127.0.0.1` | Valid IP address |
| `port` | Server port | `8080` | 1024 - 65535 |
| `buffer_size` | TCP buffer size | `8192` | 1024 - 1MB |

**Optimal Settings:**
- Message size: 1024 - 8192 bytes for balanced performance
- Concurrency: 1-16 depending on system capacity
- Buffer size: 32KB - 256KB

## Dashboard Output Requirements

The Rusty-Comms Dashboard requires specific output formats and parameters to provide full functionality. Missing required outputs will result in limited dashboard features.

### Required Output Parameters

For full dashboard compatibility, you **must** use both output parameters:

| Parameter | Purpose | Dashboard Impact |
|-----------|---------|------------------|
| `-o <file>` | Generate summary JSON file | Enables Summary Analysis tab |
| `--streaming-output-json` | Generate streaming data file | Enables Time Series Analysis tab |

### Example Dashboard-Ready Commands

```bash
# Basic dashboard-compatible benchmark
ipc-benchmark -m shm --message-size 1024 \
               -o ./shm_results.json \
               --streaming-output-json

# Comprehensive comparison for dashboard
ipc-benchmark -m uds shm tcp pmq \
               --message-size 1024 \
               --msg-count 50000 \
               -o ./results.json \
               --streaming-output-json \
               --continue-on-error

# Multi-size analysis
for size in 64 256 1024 4096; do
  ipc-benchmark -m shm \
                 --message-size $size \
                 -o ./shm_${size}.json \
                 --streaming-output-json \
                 --duration 10s
done
```

### Output File Structure

Dashboard-compatible runs will generate:

```
results/
├── sharedmemory_1024_summary.json     # Summary statistics and throughput
└── sharedmemory_1024_streaming.json   # Per-message latency data
```

### Dashboard Limitation Matrix

| ipc-benchmark Parameters | Summary Tab | Time Series Tab | Impact |
|---------------------------|-------------|-----------------|--------|
| `-o` only | ✅ Available | ❌ Missing | Limited functionality |
| `--streaming-output-json` only | ❌ Missing | ✅ Available | No statistical summaries |
| Both `-o` and `--streaming-output-json` | ✅ Available | ✅ Available | Full functionality |
| Neither parameter | ❌ Missing | ❌ Missing | Dashboard incompatible |

### Troubleshooting Dashboard Data Issues

#### Missing Time Series Data
```bash
# Problem: Only used -o parameter
ipc-benchmark -m shm -o results.json

# Solution: Add streaming output
ipc-benchmark -m shm -o results.json --streaming-output-json
```

#### Missing Summary Data
```bash
# Problem: Only used streaming output
ipc-benchmark -m shm --streaming-output-json

# Solution: Add summary output
ipc-benchmark -m shm -o results.json --streaming-output-json
```

#### No Dashboard Data
```bash
# Problem: No output parameters specified
ipc-benchmark -m shm

# Solution: Use both required parameters
ipc-benchmark -m shm -o results.json --streaming-output-json
```

For dashboard setup and usage instructions, see [`utils/dashboard/README.md`](utils/dashboard/README.md).

## Performance Tuning

### System-Level Optimizations

#### CPU Settings
```bash
# Disable frequency scaling
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Set CPU affinity
taskset -c 0-3 ipc-benchmark --concurrency 4

# Disable turbo boost for consistency
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
```

#### Memory Settings
```bash
# Increase shared memory limits
echo 2147483648 | sudo tee /proc/sys/kernel/shmmax  # 2GB
echo 4096 | sudo tee /proc/sys/kernel/shmmni        # 4096 segments

# Optimize virtual memory
echo 1 | sudo tee /proc/sys/vm/overcommit_memory
echo 90 | sudo tee /proc/sys/vm/dirty_ratio
```

#### Network Settings (TCP)
```bash
# Increase TCP buffer sizes
echo 'net.core.rmem_max = 16777216' | sudo tee -a /etc/sysctl.conf
echo 'net.core.wmem_max = 16777216' | sudo tee -a /etc/sysctl.conf
echo 'net.ipv4.tcp_rmem = 4096 87380 16777216' | sudo tee -a /etc/sysctl.conf
echo 'net.ipv4.tcp_wmem = 4096 65536 16777216' | sudo tee -a /etc/sysctl.conf

# Apply settings
sudo sysctl -p
```

### Application-Level Tuning

#### Rust Compiler Optimizations

> **Important:** This repo does **not** ship a `.cargo/config.toml` with
> `target-cpu=native`. That setting would apply to every build —
> including CI — producing non-portable binaries. When CI
> cross-compiles for ARM on x86 runners, `target-cpu=native`
> silently optimizes for the build host, not the target, and may
> emit instructions the deployment CPU does not support (e.g.,
> ARMv8.2+ on a Cortex-A53). Apply CPU tuning explicitly at build
> time instead. See the README's
> [CPU-Optimized Builds](README.md#cpu-optimized-builds) section
> for per-platform examples.

```bash
# On-target build (uses every instruction the local CPU supports)
RUSTFLAGS="-C target-cpu=native -C opt-level=3" cargo build --release

# Cross-compile for a specific ARM CPU
RUSTFLAGS="-C target-cpu=cortex-a53 -C opt-level=3" cargo build \
  --release --target aarch64-unknown-linux-gnu

# Link-time optimization
RUSTFLAGS="-C lto=fat" cargo build --release

# Profile-guided optimization
RUSTFLAGS="-C profile-generate=/tmp/pgo-data" cargo build --release
# Run benchmark to generate profile data
RUSTFLAGS="-C profile-use=/tmp/pgo-data" cargo build --release
```

#### Runtime Configuration
```bash
# Increase stack size
ulimit -s 16384

# Set process priority
nice -n -20 ipc-benchmark

# Use huge pages
echo always | sudo tee /sys/kernel/mm/transparent_hugepage/enabled
```

## Test Scenarios

### Latency-Focused Testing

**Goal**: Measure minimum latency with high precision

```bash
ipc-benchmark \
  -m uds \
  --message-size 64 \
  --msg-count 100000 \
  --concurrency 1 \
  --warmup-iterations 10000 \
  --percentiles 50 90 95 99 99.9 99.99 \
  --round-trip
```

**Configuration:**
- Single-threaded to eliminate contention
- Small message sizes to minimize serialization overhead
- Many messages for statistical significance
- Focus on round-trip measurements

### Throughput-Focused Testing

**Goal**: Measure maximum data transfer rate

```bash
ipc-benchmark \
  -m shm \
  --message-size 65536 \
  --duration 60s \
  --concurrency 8 \
  --buffer-size 1048576 \
  --one-way
```

**Configuration:**
- Multiple concurrent workers
- Large message sizes
- Large buffer sizes
- One-way communication for maximum throughput

### Scalability Testing

**Goal**: Evaluate performance under increasing load

```bash
# Test with increasing concurrency
for concurrency in 1 2 4 8 16; do
  ipc-benchmark \
    -m uds shm tcp pmq \
    --concurrency $concurrency \
    --message-size 1024 \
    --msg-count 10000 \
    --output-file "results_c${concurrency}.json"
done

# Test all mechanisms with scalability
for concurrency in 1 2 4 8; do
  ipc-benchmark \
    -m all \
    --concurrency $concurrency \
    --message-size 1024 \
    --msg-count 10000 \
    --output-file "results_all_c${concurrency}.json"
done

# PMQ-specific scalability testing (Note: PMQ has limited multi-connection support)
for msg_size in 64 512 2048 8192; do
  ipc-benchmark \
    -m pmq \
    --message-size $msg_size \
    --msg-count 5000 \
    --concurrency 1 \
    --output-file "results_pmq_${msg_size}.json"
done
```

### Comparative Analysis

**Goal**: Compare different IPC mechanisms

```bash
ipc-benchmark \
  -m uds shm tcp \
  --message-size 1024 \
  --msg-count 50000 \
  --concurrency 4 \
  --one-way \
  --round-trip \
  --output-file comparison.json

# Or simply test all available mechanisms
ipc-benchmark \
  -m all \
  --message-size 1024 \
  --msg-count 50000 \
  --concurrency 4 \
  --one-way \
  --round-trip \
  --output-file complete_comparison.json
```

## System Requirements

### Minimum Requirements

- **OS**: Linux kernel 3.10+ (RHEL 7+, Ubuntu 16.04+)
- **CPU**: x86_64 architecture, 2 cores
- **RAM**: 4GB
- **Disk**: 1GB free space
- **Rust**: 1.70.0+

### Recommended Configuration

- **OS**: Linux kernel 5.0+ (RHEL 9, Ubuntu 20.04+)
- **CPU**: x86_64, 8+ cores, 3.0+ GHz
- **RAM**: 16GB+
- **Disk**: SSD with 10GB+ free space
- **Network**: Gigabit Ethernet (for TCP tests)

### Resource Requirements by Test Type

#### Latency Testing
- **CPU**: 2-4 cores, high frequency
- **RAM**: 4GB minimum
- **Disk**: Any (minimal I/O)

#### Throughput Testing
- **CPU**: 8+ cores
- **RAM**: 16GB+ (for shared memory)
- **Disk**: SSD recommended

#### Scalability Testing
- **CPU**: 16+ cores
- **RAM**: 32GB+
- **Disk**: High-performance SSD

### Container Environments

#### Docker Configuration
```dockerfile
FROM rust:1.75-slim

RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libc6-dev

WORKDIR /app
COPY . .
RUN cargo build --release

# Increase shared memory limit
RUN echo 'tmpfs /dev/shm tmpfs defaults,size=2g 0 0' >> /etc/fstab

CMD ["./target/release/ipc-benchmark"]
```

#### Kubernetes Configuration
```yaml
apiVersion: v1
kind: Pod
spec:
  containers:
  - name: ipc-benchmark
    image: ipc-benchmark:latest
    resources:
      requests:
        memory: "4Gi"
        cpu: "2"
      limits:
        memory: "16Gi"
        cpu: "8"
    volumeMounts:
    - name: shm
      mountPath: /dev/shm
  volumes:
  - name: shm
    emptyDir:
      medium: Memory
      sizeLimit: 2Gi
```

## Troubleshooting Configuration

### Common Configuration Issues

1. **Buffer Size Too Small**: Increase buffer_size for large messages
2. **Insufficient Shared Memory**: Adjust kernel limits
3. **Port Conflicts**: Use different ports for multiple TCP tests
4. **Permission Errors**: Ensure write access to temp directories

### Mechanism-Specific Configuration

#### POSIX Message Queues (PMQ)
```bash
# Check PMQ limits
cat /proc/sys/fs/mqueue/msg_max       # Max messages per queue
cat /proc/sys/fs/mqueue/msgsize_max   # Max message size

# Mount message queue filesystem (if not mounted)
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue

# Increase PMQ limits if needed
echo 100 | sudo tee /proc/sys/fs/mqueue/msg_max
echo 16384 | sudo tee /proc/sys/fs/mqueue/msgsize_max
```

**PMQ Limitations:**
- Limited multi-connection support compared to other mechanisms
- Message size and queue depth are system-limited
- Queue names must start with '/' and follow POSIX naming rules
- Persistent queues may need manual cleanup after crashes

#### Shared Memory (SHM)
```bash
# Check current limits
ipcs -lm

# Increase shared memory limits
echo 2147483648 | sudo tee /proc/sys/kernel/shmmax  # 2GB max segment
echo 4096 | sudo tee /proc/sys/kernel/shmmni        # 4096 segments
```

#### TCP Sockets
```bash
# Increase socket buffer sizes
echo 'net.core.rmem_max = 16777216' | sudo tee -a /etc/sysctl.conf
echo 'net.core.wmem_max = 16777216' | sudo tee -a /etc/sysctl.conf
sudo sysctl -p
```

### Validation Commands

```bash
# Check shared memory limits
ipcs -lm

# Check available ports
ss -tuln | grep :8080

# Check file permissions
ls -la /tmp/ipc_benchmark_*

# Verify CLI options
ipc-benchmark --help
```

### Performance Debugging

```bash
# Enable detailed logging (DEBUG level)
ipc-benchmark -v -m uds -i 1000

# Enable TRACE level logging
ipc-benchmark -vv -m uds -i 100

# Profile with perf
perf record -g ./target/release/ipc-benchmark -m uds -i 50000
perf report

# Memory usage analysis
valgrind --tool=massif ./target/release/ipc-benchmark -m uds -i 1000
```

---

For additional configuration assistance, see the [troubleshooting section](README.md#troubleshooting) in the main README. 