# Configuration Guide

This document provides detailed information about configuring the IPC Benchmark Suite for various testing scenarios and environments.

## Table of Contents

- [Command Line Options](#command-line-options)
- [Configuration File](#configuration-file)
- [Environment Variables](#environment-variables)
- [IPC Mechanism Settings](#ipc-mechanism-settings)
- [Cross-Environment Configuration](#cross-environment-configuration)
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
| `--streaming-output-json` | String | - | JSON file for streaming results during execution |
| `--streaming-output-csv` | String | - | CSV file for streaming results during execution |
| `--continue-on-error` | Boolean | `false` | Continue running if one test fails |
| `--verbose` | `-v` | Boolean | `false` | Enable verbose output |
| `--host` | String | `"127.0.0.1"` | Host address for TCP sockets |
| `--port` | Number | `8080` | Port for TCP sockets |

### Cross-Environment Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `--mode` | String | `standalone` | Execution mode: `standalone`, `host`, `client` |
| `--ipc-path` | String | Auto-generated | IPC path for UDS (required for cross-env) |
| `--shm-name` | String | Auto-generated | Shared memory name for SHM cross-env |
| `--connection-timeout` | Number | `30` | Connection timeout in seconds for cross-env |

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

## Configuration File

You can create a configuration file to avoid repeating command-line arguments:

### JSON Configuration Format

```json
{
  "mechanisms": ["uds", "shm", "tcp", "pmq"],
  // Alternative: "mechanisms": ["all"] to test all available mechanisms
  "message_size": 1024,
  "msg_count": 10000,
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
mechanisms = ["uds", "shm", "tcp", "pmq"]
# Alternative: mechanisms = ["all"]  # to test all available mechanisms
message_size = 1024
msg_count = 10000
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

### POSIX Message Queues (PMQ)

| Setting | Description | Default | Range |
|---------|-------------|---------|-------|
| `message_queue_name` | Queue name | `ipc_benchmark_<uuid>` | Valid POSIX name |
| `message_queue_depth` | Max messages in queue | `10` | 1 - System limit |
| `buffer_size` | Message buffer size | `8192` | 1 - msgsize_max |

**Optimal Settings:**
- Message size: 64 - 8192 bytes (system-dependent)
- Concurrency: 1-2 (limited multi-connection support)
- Queue depth: 10-100 depending on system limits

## Cross-Environment Configuration

The benchmark suite supports three execution modes for different testing scenarios:

### Execution Modes

#### 1. Standalone Mode (Default)
Traditional single-process benchmarking where both client and server run within the same process.

```bash
# Default standalone mode
./target/release/ipc-benchmark -m uds --message-size 1024 --msg-count 1000

# Explicit standalone mode
./target/release/ipc-benchmark --mode standalone -m shm --duration 30s
```

#### 2. Host Mode (Cross-Environment Client)
The host process acts as a benchmark client, connecting to a containerized server.

```bash
# UDS host mode - connect to container server
./target/release/ipc-benchmark --mode host -m uds \
  --ipc-path ./sockets/ipc_benchmark.sock \
  --message-size 1024 --msg-count 1000

# SHM host mode - connect to container shared memory
./target/release/ipc-benchmark --mode host -m shm \
  --shm-name ipc_benchmark_shm_crossenv \
  --message-size 4096 --duration 30s

# PMQ host mode - connect to container message queue
./target/release/ipc-benchmark --mode host -m pmq \
  --message-size 512 --msg-count 5000
```

#### 3. Client Mode (Cross-Environment Server)
The client process acts as a passive server, waiting for connections from host processes.

```bash
# UDS client mode - create socket server for host
./target/release/ipc-benchmark --mode client -m uds \
  --ipc-path ./sockets/ipc_benchmark.sock

# SHM client mode - create shared memory for host
./target/release/ipc-benchmark --mode client -m shm \
  --shm-name ipc_benchmark_shm_crossenv

# PMQ client mode - create message queue for host
./target/release/ipc-benchmark --mode client -m pmq
```

### Host-to-Container Configuration

#### Unified Script Configuration

The `run_host_container.sh` script supports environment variable configuration:

```bash
# Basic configuration
export MECHANISM=uds              # uds, shm, pmq
export DURATION=30s               # Duration instead of message count
export OUTPUT_FILE=./output/results.json
export STREAM_JSON=./output/stream.json
export STREAM_CSV=./output/stream.csv

# UDS-specific configuration
export SOCKET_PATH=./sockets/custom.sock

# SHM-specific configuration  
export SHM_NAME=custom_shm
export BUFFER_SIZE=1048576

# Advanced configuration
export ROUND_TRIP=true            # Enable round-trip testing
export VERBOSE=true               # Enable verbose logging
export CONNECTION_TIMEOUT=60      # Connection timeout in seconds

# Run with configuration
./run_host_container.sh $MECHANISM $MESSAGES $MESSAGE_SIZE $WORKERS
```

#### Container Environment Variables

The containerized servers support these environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `IPC_BENCHMARK_TEMP_DIR` | Temporary directory for IPC files | `/tmp/ipc` |
| `IPC_BENCHMARK_OUTPUT_DIR` | Output directory for results | `/app/output` |
| `IPC_BENCHMARK_SOCKET_DIR` | Directory for socket files | `/app/sockets` |
| `IPC_BENCHMARK_DEFAULT_SOCKET_PATH` | Default UDS socket path | `/app/sockets/ipc_benchmark.sock` |
| `RUST_LOG` | Logging level | `info` |

#### Cross-Environment IPC Paths

**Unix Domain Sockets:**
- Host path: `./sockets/ipc_benchmark.sock`
- Container path: `/app/sockets/ipc_benchmark.sock`
- Shared via bind mount

**Shared Memory:**
- Name: `ipc_benchmark_shm_crossenv`
- Accessible by both host and container processes
- Requires `/dev/shm` mount

**POSIX Message Queues:**
- Name: `ipc_benchmark_pmq_crossenv`
- Requires `/dev/mqueue` mount in container
- Accessible system-wide

### Container Management Configuration

#### Podman Configuration
```bash
# Container runtime settings
PODMAN_OPTS="--rm --security-opt label=disable"

# Volume mounts for IPC
SOCKET_MOUNT="-v ./sockets:/app/sockets:Z"
SHM_MOUNT="--tmpfs /dev/shm:rw,noexec,nosuid,size=1g"
MQ_MOUNT="-v /dev/mqueue:/dev/mqueue:rw"

# Resource limits
MEMORY_LIMIT="--memory=2g"
CPU_LIMIT="--cpus=4"
```

#### Docker Configuration
```bash
# Docker equivalent settings
DOCKER_OPTS="--rm --privileged"

# Volume mounts (Docker syntax)
SOCKET_MOUNT="-v $(pwd)/sockets:/app/sockets"
SHM_MOUNT="--tmpfs /dev/shm:rw,noexec,nosuid,size=1g"
MQ_MOUNT="-v /dev/mqueue:/dev/mqueue"
```

### Security Configuration

#### Container Security
```yaml
# Podman security settings
security_opt:
  - "label=disable"           # Disable SELinux for volume mounts
  - "apparmor=unconfined"     # Disable AppArmor restrictions

# User namespace mapping
userns_mode: "keep-id"        # Preserve user ID mapping

# Capabilities
cap_add:
  - "IPC_LOCK"                # Required for shared memory
```

#### Host Security
```bash
# SELinux contexts for socket files
chcon -t container_file_t ./sockets/

# File permissions
chmod 755 ./sockets/
chmod 666 ./sockets/*.sock
```

### Performance Configuration for Cross-Environment

#### Host-Side Optimizations
```bash
# CPU affinity for host process
taskset -c 0-3 ./run_host_container.sh uds 10000 1024 1

# Process priority
nice -n -10 ./run_host_container.sh shm 10000 4096 1

# Memory locking
ulimit -l unlimited
```

#### Container-Side Optimizations
```bash
# Container resource limits
podman run --cpus=4 --memory=2g --memory-swap=2g

# Shared memory size
--tmpfs /dev/shm:rw,noexec,nosuid,size=2g

# CPU affinity within container
--cpuset-cpus=4-7
```

### Troubleshooting Cross-Environment Configuration

#### Common Configuration Issues

1. **Socket Permission Errors**
   ```bash
   # Fix socket directory permissions
   chmod 755 ./sockets/
   chown $(id -u):$(id -g) ./sockets/
   ```

2. **Shared Memory Access Denied**
   ```bash
   # Increase shared memory limits
   echo 2147483648 | sudo tee /proc/sys/kernel/shmmax
   
   # Mount shared memory in container
   podman run --tmpfs /dev/shm:rw,size=2g
   ```

3. **Message Queue Not Found**
   ```bash
   # Mount message queue filesystem
   sudo mkdir -p /dev/mqueue
   sudo mount -t mqueue none /dev/mqueue
   
   # In container
   podman run -v /dev/mqueue:/dev/mqueue
   ```

4. **Connection Timeouts**
   ```bash
   # Increase connection timeout
   CONNECTION_TIMEOUT=60 ./run_host_container.sh uds 1000 1024 1
   
   # Check container readiness
   podman logs rusty-comms-uds-server
   ```

#### Validation Commands

```bash
# Check container status
podman ps --filter "name=rusty-comms"

# Verify IPC resources
ls -la ./sockets/              # UDS sockets
ipcs -m                        # Shared memory segments
ipcs -q                        # Message queues

# Test connectivity
podman exec rusty-comms-uds-server ls -la /app/sockets/

# Monitor performance
podman stats rusty-comms-uds-server
```

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
  -m uds \
  --message-size 64 \
  --msg-count 100000 \
  --concurrency 1 \
  --warmup-iterations 10000 \
  --percentiles 50 90 95 99 99.9 99.99 \
  --round-trip \
  --no-one-way
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