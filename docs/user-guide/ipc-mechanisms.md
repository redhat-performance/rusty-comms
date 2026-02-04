# IPC Mechanisms

This guide provides detailed information about each Inter-Process Communication mechanism supported by rusty-comms.

## Overview

| Mechanism | Short | Latency | Throughput | Platform | Best For |
|-----------|-------|---------|------------|----------|----------|
| Unix Domain Sockets | `uds` | Low (2-10μs) | High | Unix | General local IPC |
| Shared Memory | `shm` | Very Low (1-7μs) | Very High | Cross-platform (tested on Linux) | Maximum performance |
| TCP Sockets | `tcp` | Medium (5-20μs) | High | Cross-platform | Network capability |
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
- Intended to be cross-platform (primarily tested on Linux)
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

## Next Steps

- **[Advanced Usage](advanced-usage.md)** - Cross-process testing, CPU affinity
- **[Troubleshooting](troubleshooting.md)** - Mechanism-specific issues
- **[Performance Tuning](performance-tuning.md)** - System optimization
