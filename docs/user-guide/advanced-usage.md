# Advanced Usage

This guide covers advanced features including execution modes, CPU affinity, cross-process testing, and performance optimization.

## Execution Modes: Async vs Blocking

The benchmark supports two execution modes for different use cases.

### Async Mode (Default)

Uses the Tokio runtime with async/await:

```bash
# Async mode (default)
ipc-benchmark -m uds -i 10000
```

**Characteristics:**
- Uses `tokio::spawn` for concurrency
- Non-blocking I/O operations
- Matches modern async Rust applications
- Efficient for high-concurrency workloads

### Blocking Mode

Uses pure standard library with blocking I/O:

```bash
# Blocking mode
ipc-benchmark -m uds -i 10000 --blocking
```

**Characteristics:**
- Uses `std::thread` for concurrency
- Blocking I/O operations
- No Tokio runtime overhead
- Simpler execution model

### Comparing Modes

Run the same test in both modes for direct comparison:

```bash
# Async test
ipc-benchmark -m uds -i 50000 -o async_results.json

# Blocking test
ipc-benchmark -m uds -i 50000 --blocking -o blocking_results.json
```

## Shared Memory Direct Mode

For maximum shared memory performance, use the `--shm-direct` flag:

```bash
# Direct memory mode (auto-enables blocking)
ipc-benchmark -m shm --shm-direct -i 10000
```

**Benefits:**
- 3x faster average latency (~7μs vs ~20μs)
- 450x better tail latency (~22μs vs ~10ms)
- No serialization overhead

**Limitations:**
- Unix-only (not Windows)
- Fixed message size (8KB max)

## CPU Affinity

Pin server and client to specific CPU cores for reproducible results:

```bash
# Pin server to core 0, client to core 1
ipc-benchmark -m uds -i 10000 \
  --server-affinity 0 \
  --client-affinity 1
```

### Terminology

| Flag | Role | Description |
|------|------|-------------|
| `--server-affinity` | Receiver | The process receiving messages |
| `--client-affinity` | Sender | The process sending messages |

### Best Practices

- Use different physical cores (not hyperthreads)
- Avoid cores running system services (core 0 on some systems)
- Check your CPU topology with `lscpu` or `lstopo`

```bash
# Example: Use cores 2 and 4
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 2 \
  --client-affinity 4
```

## Cross-Process Testing

Run benchmarks between separate processes, including across container boundaries.

### Run Modes

| Mode | Description | Use Case |
|------|-------------|----------|
| `standalone` | Both processes on same host (default) | Normal benchmarking |
| `client` | Acts as IPC server (receiver) | Inside containers |
| `sender` | Acts as IPC client (sender) | From host to container |

**Note:** Cross-process run modes currently require `--blocking`. The async
client/sender paths are not implemented yet, so `--run-mode client|sender`
without `--blocking` is not supported.

### Basic Cross-Process Setup

**Terminal 1: Start the receiver (client mode)**
```bash
./ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Terminal 2: Start the sender**
```bash
./ipc-benchmark -m tcp --run-mode sender --blocking \
  --host 127.0.0.1 \
  -i 10000 \
  -o results.json
```

### Container Testing

**Step 1: Copy binary to container**
```bash
podman cp ./target/release/ipc-benchmark mycontainer:/tmp/
```

**Step 2: Start receiver in container**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Step 3: Run sender from host**
```bash
./ipc-benchmark -m tcp --run-mode sender --blocking \
  --host <container-ip> \
  -i 10000 \
  -o results.json
```

### Mechanism-Specific Container Setup

| Mechanism | Container Configuration |
|-----------|------------------------|
| TCP | Network accessible (`--network host` or port forwarding) |
| UDS | Shared volume for socket file |
| SHM | Shared `/dev/shm` mount |
| PMQ | `--ipc=host` + mount `/dev/mqueue` |

### Cross-Container Flag

For shared memory across containers, the `--cross-container` flag enables container-safe synchronization:

```bash
# Usually auto-detected, but can be forced
ipc-benchmark -m shm --run-mode client --blocking --cross-container
```

## Latency-Focused Testing

Use `--send-delay` to measure latency under controlled load instead of maximum throughput:

```bash
# Send a message every 10ms
ipc-benchmark -m uds -i 10000 --send-delay 10ms

# Send a message every 50 microseconds
ipc-benchmark -m uds -i 10000 --send-delay 50us
```

**When to use:**
- Testing latency under realistic workloads
- Avoiding backpressure effects
- Simulating periodic message patterns

## Custom Percentiles

Request specific percentile calculations:

```bash
# Standard set
ipc-benchmark -m uds -i 10000 --percentiles 50 95 99 99.9

# Extended set for latency analysis
ipc-benchmark -m uds -i 100000 --percentiles 50 75 90 95 99 99.5 99.9 99.99
```

## Buffer Size Tuning

Control internal buffer sizes for different testing scenarios:

```bash
# Large buffer for high throughput
ipc-benchmark -m shm -i 100000 --buffer-size 1048576

# Small buffer to test backpressure
ipc-benchmark -m shm -i 10000 --buffer-size 8192
```

**Note:** If omitted, buffer size is auto-calculated based on message count and size.

## First Message Handling

By default, the first message is discarded to avoid cold-start effects:

```bash
# Include first message in results
ipc-benchmark -m uds -i 10000 --include-first-message
```

## Complete Advanced Example

```bash
# High-precision latency benchmark
./target/release/ipc-benchmark \
  -m shm \
  --shm-direct \
  -s 64 \
  -i 100000 \
  -w 10000 \
  --server-affinity 2 \
  --client-affinity 4 \
  --percentiles 50 90 95 99 99.9 99.99 \
  -o ./results/shm_latency.json \
  --streaming-output-json ./results/shm_streaming.json \
  -v
```

## Next Steps

- **[IPC Mechanisms](ipc-mechanisms.md)** - Detailed mechanism information
- **[Troubleshooting](troubleshooting.md)** - Common issues and solutions
- **[CLI Reference](../reference/cli-reference.md)** - Complete option reference
