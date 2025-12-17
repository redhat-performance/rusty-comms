# Host-Container IPC Benchmarking Guide

This guide explains how to run IPC benchmarks between the host and a Podman container.

## Overview

Host-container mode enables realistic IPC performance measurement across process isolation boundaries. The host acts as the benchmark driver (sending messages and collecting results), while the container acts as the responder.

```
┌─────────────────────┐                    ┌─────────────────────┐
│   Host (Driver)     │  <── IPC ──>       │ Container (Server)  │
│   - Sends messages  │                    │ - Receives messages │
│   - Measures latency│                    │ - Echoes back (RT)  │
│   - Collects results│                    │                     │
└─────────────────────┘                    └─────────────────────┘
```

## Prerequisites

1. **Podman installed**: See [PODMAN_SETUP.md](PODMAN_SETUP.md)
2. **Container image built**:
   ```bash
   podman build -t localhost/ipc-benchmark:latest .
   ```
3. **Binary compiled**:
   ```bash
   cargo build --release
   ```

## Quick Start

### Basic UDS Benchmark

```bash
# Run 1000 one-way messages via Unix Domain Sockets
./target/release/ipc-benchmark \
    -m uds \
    --run-mode host \
    --blocking \
    --msg-count 1000 \
    --message-size 128
```

### Basic Shared Memory Benchmark

```bash
# Run 1000 one-way messages via shared memory
./target/release/ipc-benchmark \
    -m shm \
    --run-mode host \
    --blocking \
    --msg-count 1000 \
    --message-size 1024
```

### Round-Trip Latency Test

```bash
# Measure round-trip latency with TCP
./target/release/ipc-benchmark \
    -m tcp \
    --run-mode host \
    --blocking \
    --round-trip \
    --msg-count 500
```

## Run Modes

### `--run-mode standalone` (Default)

The traditional mode where both client and server run as host processes. This is the default and doesn't use containers.

### `--run-mode host`

Host-container mode. The host:
1. Creates a Podman container with the correct IPC mounts
2. Runs the benchmark binary inside the container in client mode
3. Sends messages from host to container
4. Collects latency measurements

```bash
./target/release/ipc-benchmark -m uds --run-mode host --blocking
```

### `--run-mode client`

Client mode (used inside container). Normally invoked automatically by host mode, but can be used for manual testing:

```bash
# Inside container or for debugging
./target/release/ipc-benchmark \
    -m uds \
    --run-mode client \
    --blocking \
    --socket-path /tmp/rusty-comms/test.sock
```

## Supported IPC Mechanisms

### Unix Domain Sockets (UDS)

Best for: Low-latency local communication

```bash
./target/release/ipc-benchmark \
    -m uds \
    --run-mode host \
    --blocking \
    --msg-count 10000 \
    --message-size 256
```

**Container config:** Mounts `/tmp/rusty-comms` for socket files

### Shared Memory (SHM)

Best for: High-throughput, large messages

```bash
# Ring buffer mode (default)
./target/release/ipc-benchmark \
    -m shm \
    --run-mode host \
    --blocking \
    --msg-count 10000 \
    --message-size 4096

# Direct memory mode (lower latency)
./target/release/ipc-benchmark \
    -m shm \
    --run-mode host \
    --blocking \
    --shm-direct \
    --msg-count 10000 \
    --message-size 1024
```

**Container config:** Uses `--ipc=host` for shared `/dev/shm`

**Note:** Round-trip is not supported for SHM in blocking mode (one-way only).

### TCP Sockets

Best for: Network protocol testing, compatibility

```bash
./target/release/ipc-benchmark \
    -m tcp \
    --run-mode host \
    --blocking \
    --msg-count 10000 \
    --round-trip
```

**Container config:** Uses `--network=host`

### POSIX Message Queues (PMQ)

Best for: Kernel-managed, priority-based messaging

```bash
./target/release/ipc-benchmark \
    -m pmq \
    --run-mode host \
    --blocking \
    --msg-count 100  # Keep small due to queue limits
    --message-size 256
```

**Container config:** Mounts `/dev/mqueue` with `--privileged`

**Note:** PMQ has strict kernel limits. Message counts >10 may fail.

## Common Options

### Test Configuration

| Option | Description | Default |
|--------|-------------|---------|
| `--msg-count N` | Number of messages to send | 1000 |
| `--message-size N` | Payload size in bytes | 64 |
| `--one-way` | Measure one-way latency | true |
| `--round-trip` | Measure round-trip latency | false |
| `--warmup-iterations N` | Warmup messages before test | 0 |

### Container Options

| Option | Description | Default |
|--------|-------------|---------|
| `--container-image IMG` | Container image to use | `localhost/ipc-benchmark:latest` |
| `--container-prefix PFX` | Prefix for container names | `rusty-comms` |

### Performance Options

| Option | Description | Default |
|--------|-------------|---------|
| `--blocking` | Use blocking I/O (required for host mode) | false |
| `--shm-direct` | Use direct memory SHM (lower latency) | false |
| `--server-affinity N` | Pin server to CPU core N | none |
| `--client-affinity N` | Pin client to CPU core N | none |

## Managing Containers

### List Containers

```bash
./target/release/ipc-benchmark --list-containers
```

Output:
```
Benchmark containers (prefix: rusty-comms):
--------------------------------------------------
  rusty-comms-uds : running
  rusty-comms-shm : stopped

Use --stop-container <mechanism> to stop a container.
Use --stop-container all to stop all containers.
```

### Stop Containers

```bash
# Stop specific mechanism
./target/release/ipc-benchmark --stop-container uds

# Stop all benchmark containers
./target/release/ipc-benchmark --stop-container all
```

## Example Benchmark Session

### Complete UDS Comparison

```bash
#!/bin/bash

# Build if needed
cargo build --release

# Run standalone mode baseline
echo "=== Standalone Mode ==="
./target/release/ipc-benchmark \
    -m uds --blocking \
    --msg-count 5000 \
    --message-size 256 \
    2>&1 | grep -E "(Latency|Throughput)"

# Run host-container mode
echo "=== Host-Container Mode ==="
./target/release/ipc-benchmark \
    -m uds --blocking \
    --run-mode host \
    --msg-count 5000 \
    --message-size 256 \
    2>&1 | grep -E "(Latency|Throughput)"

# Cleanup
./target/release/ipc-benchmark --stop-container all
```

### Multi-Mechanism Comparison

```bash
#!/bin/bash

for mech in uds shm tcp; do
    echo "=== $mech ===" 
    ./target/release/ipc-benchmark \
        -m $mech \
        --run-mode host \
        --blocking \
        --msg-count 1000 \
        --message-size 512 \
        2>&1 | grep -E "(mean|Throughput)"
done

./target/release/ipc-benchmark --stop-container all
```

## Output Formats

### Console Output (Default)

```
============================================================
                     Benchmark Configuration
============================================================
Mechanism:        Unix Domain Sockets
Message Size:     256 bytes
...

============================================================
                     One-Way Latency Results
============================================================
Latency (mean):   12.5µs
Latency (min):    8.2µs
Latency (max):    45.3µs
Latency (P99):    28.1µs
Throughput:       80,000 msg/s
```

### JSON Output

```bash
./target/release/ipc-benchmark \
    -m uds --run-mode host --blocking \
    --output-file results.json
```

### Streaming CSV

```bash
./target/release/ipc-benchmark \
    -m uds --run-mode host --blocking \
    --streaming-output latencies.csv \
    --streaming-format csv
```

## Performance Tips

### For Lowest Latency

1. Use `--shm-direct` for shared memory
2. Pin to isolated CPU cores: `--server-affinity 2 --client-affinity 3`
3. Disable warmup for quick tests: `--warmup-iterations 0`
4. Keep message sizes small (<1KB)

### For Highest Throughput

1. Use larger message sizes (4KB-64KB)
2. Use shared memory (`-m shm`)
3. Increase buffer sizes: `--buffer-size 65536`

### For Accurate Results

1. Use warmup: `--warmup-iterations 100`
2. Run sufficient messages: `--msg-count 10000`
3. Run on idle system
4. Use CPU affinity for consistency

## Troubleshooting

### "Host mode async is not yet implemented"

Host-container mode requires blocking I/O. Add `--blocking`:

```bash
./target/release/ipc-benchmark -m uds --run-mode host --blocking
```

### "Container image not found"

Build the container image:

```bash
podman build -t localhost/ipc-benchmark:latest .
```

### "Container did not become ready"

The container server failed to start. Check:
1. Image exists: `podman images | grep ipc-benchmark`
2. Test manually: `podman run --rm localhost/ipc-benchmark:latest --help`
3. Check permissions for volume mounts

### "Timeout opening shared memory"

Shared memory segment not accessible:
1. Ensure host created the segment first
2. Check `--ipc=host` is working
3. Clean up stale segments: `rm /dev/shm/ipc-bench-*`

### PMQ Hangs or Fails

POSIX Message Queues have strict kernel limits:
1. Use small message counts (`--msg-count 5`)
2. Use small message sizes (`--message-size 256`)
3. Avoid round-trip mode with PMQ

## See Also

- [PODMAN_SETUP.md](PODMAN_SETUP.md) - Container installation and configuration
- [README.md](../README.md) - Main project documentation

