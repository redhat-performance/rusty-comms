# Configuration Guide

This document provides detailed information about configuring the IPC Benchmark Suite for various testing scenarios and environments.

## Table of Contents

- [Command Line Options](#command-line-options)
- [Configuration File](#configuration-file)
- [Environment Variables](#environment-variables)
- [IPC Mechanism Settings](#ipc-mechanism-settings)
- [Performance Tuning](#performance-tuning)
- [Test Scenarios](#test-scenarios)
- [System Requirements](#system-requirements)

## Command Line Options

### Basic Options

| Option | Short | Type | Default | Description |
|--------|-------|------|---------|-------------|
| `--mechanisms` | `-m` | String[] | `[uds]` | IPC mechanisms to benchmark |
| `--message-size` | `-s` | Number | `1024` | Message size in bytes |
| `--iterations` | `-i` | Number | `10000` | Number of iterations to run |
| `--duration` | `-d` | String | - | Duration to run (e.g., "30s", "5m") |
| `--concurrency` | `-c` | Number | `1` | Number of concurrent workers |
| `--output-file` | `-o` | String | `benchmark_results.json` | Output file path |

### Test Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `--one-way` | Boolean | `true` | Enable one-way latency tests |
| `--round-trip` | Boolean | `true` | Enable round-trip latency tests |
| `--warmup-iterations` | `-w` | Number | `1000` | Number of warmup iterations |
| `--percentiles` | Number[] | `[50.0, 95.0, 99.0, 99.9]` | Percentiles to calculate |
| `--buffer-size` | Number | `8192` | Buffer size for shared memory/queues |

### Advanced Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `--streaming-output` | String | - | File for streaming results during execution |
| `--continue-on-error` | Boolean | `false` | Continue running if one test fails |
| `--verbose` | `-v` | Boolean | `false` | Enable verbose output |
| `--host` | String | `"127.0.0.1"` | Host address for TCP sockets |
| `--port` | Number | `8080` | Port for TCP sockets |

### Examples

```bash
# Basic usage
ipc-benchmark --mechanisms uds shm --message-size 4096 --iterations 50000

# Duration-based testing
ipc-benchmark --duration 60s --concurrency 8

# Comprehensive latency analysis
ipc-benchmark --percentiles 50 90 95 99 99.9 99.99 --warmup-iterations 10000

# High-throughput testing
ipc-benchmark --mechanisms shm --message-size 65536 --buffer-size 1048576
```

## Configuration File

You can create a configuration file to avoid repeating command-line arguments:

### JSON Configuration Format

```json
{
  "mechanisms": ["uds", "shm", "tcp"],
  "message_size": 1024,
  "iterations": 10000,
  "concurrency": 4,
  "one_way": true,
  "round_trip": true,
  "warmup_iterations": 1000,
  "percentiles": [50.0, 95.0, 99.0, 99.9],
  "buffer_size": 8192,
  "output_file": "results.json",
  "streaming_output": "streaming.json",
  "host": "127.0.0.1",
  "port": 8080
}
```

### TOML Configuration Format

```toml
mechanisms = ["uds", "shm", "tcp"]
message_size = 1024
iterations = 10000
concurrency = 4
one_way = true
round_trip = true
warmup_iterations = 1000
percentiles = [50.0, 95.0, 99.0, 99.9]
buffer_size = 8192
output_file = "results.json"
streaming_output = "streaming.json"
host = "127.0.0.1"
port = 8080
```

### Using Configuration Files

```bash
# JSON configuration
ipc-benchmark --config config.json

# TOML configuration
ipc-benchmark --config config.toml

# Override specific options
ipc-benchmark --config config.json --concurrency 8
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Logging level (trace, debug, info, warn, error) | `info` |
| `IPC_BENCHMARK_TEMP_DIR` | Temporary directory for IPC files | `/tmp` |
| `IPC_BENCHMARK_OUTPUT_DIR` | Default output directory | Current directory |
| `IPC_BENCHMARK_CONFIG` | Default configuration file path | - |

### Environment Variable Examples

```bash
# Enable debug logging
RUST_LOG=debug ipc-benchmark

# Use custom temporary directory
IPC_BENCHMARK_TEMP_DIR=/var/tmp ipc-benchmark

# Set default configuration
IPC_BENCHMARK_CONFIG=./default.json ipc-benchmark
```

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
| `buffer_size` | Ring buffer size | `8192` | 4096 - 1GB |

**Optimal Settings:**
- Message size: 1024 - 65536 bytes for highest throughput
- Concurrency: 2-8 (one producer, multiple consumers)
- Buffer size: 64KB - 1MB depending on message size

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
```bash
# Maximum optimization
RUSTFLAGS="-C target-cpu=native -C opt-level=3" cargo build --release

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
  --mechanisms uds \
  --message-size 64 \
  --iterations 100000 \
  --concurrency 1 \
  --warmup-iterations 10000 \
  --percentiles 50 90 95 99 99.9 99.99 \
  --round-trip \
  --no-one-way
```

**Configuration:**
- Single-threaded to eliminate contention
- Small message sizes to minimize serialization overhead
- Many iterations for statistical significance
- Focus on round-trip measurements

### Throughput-Focused Testing

**Goal**: Measure maximum data transfer rate

```bash
ipc-benchmark \
  --mechanisms shm \
  --message-size 65536 \
  --duration 60s \
  --concurrency 8 \
  --buffer-size 1048576 \
  --one-way \
  --no-round-trip
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
    --mechanisms uds shm tcp \
    --concurrency $concurrency \
    --message-size 1024 \
    --iterations 10000 \
    --output-file "results_c${concurrency}.json"
done
```

### Comparative Analysis

**Goal**: Compare different IPC mechanisms

```bash
ipc-benchmark \
  --mechanisms uds shm tcp \
  --message-size 1024 \
  --iterations 50000 \
  --concurrency 4 \
  --one-way \
  --round-trip \
  --output-file comparison.json
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

### Validation Commands

```bash
# Check shared memory limits
ipcs -lm

# Check available ports
netstat -tuln | grep :8080

# Check file permissions
ls -la /tmp/ipc_benchmark_*

# Validate configuration
ipc-benchmark --config config.json --dry-run
```

### Performance Debugging

```bash
# Enable detailed logging
RUST_LOG=debug ipc-benchmark --verbose

# Profile with perf
perf record -g ipc-benchmark
perf report

# Memory usage analysis
valgrind --tool=massif ipc-benchmark
```

---

For additional configuration assistance, see the [troubleshooting section](README.md#troubleshooting) in the main README. 