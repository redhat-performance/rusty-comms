# Quick Reference

A quick reference for running IPC benchmarks in different modes and configurations.

---

## Basic Commands

```bash
# Build first (required)
cargo build --release

# Basic test with defaults
./target/release/ipc-benchmark -m uds -i 10000

# Test all mechanisms
./target/release/ipc-benchmark -m all -i 10000 -o results.json
```

---

## By IPC Mechanism

### Unix Domain Sockets (UDS)

```bash
# Async mode (default)
ipc-benchmark -m uds -i 10000

# Blocking mode
ipc-benchmark -m uds -i 10000 --blocking

# With output
ipc-benchmark -m uds -i 10000 -o uds_results.json
```

### Shared Memory (SHM) - Ring Buffer

```bash
# Async mode (default)
ipc-benchmark -m shm -i 10000

# Blocking mode
ipc-benchmark -m shm -i 10000 --blocking

# With larger buffer
ipc-benchmark -m shm -i 10000 --buffer-size 1048576
```

### Shared Memory (SHM) - Direct Mode

```bash
# Direct memory access (auto-enables blocking, lowest latency)
ipc-benchmark -m shm --shm-direct -i 10000

# With CPU affinity for best results
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 2 --client-affinity 4

# Full precision test
ipc-benchmark -m shm --shm-direct -i 100000 -w 10000 \
  --server-affinity 2 --client-affinity 4 \
  -o shm_direct_results.json
```

### TCP Sockets

```bash
# Async mode (default)
ipc-benchmark -m tcp -i 10000

# Blocking mode
ipc-benchmark -m tcp -i 10000 --blocking

# Custom port
ipc-benchmark -m tcp -i 10000 --port 9090
```

### POSIX Message Queues (PMQ)

```bash
# Async mode (default) - Linux only
ipc-benchmark -m pmq -i 10000

# Blocking mode
ipc-benchmark -m pmq -i 10000 --blocking

# With message priority
ipc-benchmark -m pmq -i 10000 --pmq-priority 1
```

---

## By Execution Mode

### Standalone (Default)

Both sender and receiver run in the same process/host:

```bash
# All mechanisms support standalone
ipc-benchmark -m uds -i 10000
ipc-benchmark -m shm -i 10000
ipc-benchmark -m tcp -i 10000
ipc-benchmark -m pmq -i 10000
```

### Async vs Blocking

```bash
# Async (Tokio runtime) - default
ipc-benchmark -m uds -i 10000

# Blocking (std library, no Tokio)
ipc-benchmark -m uds -i 10000 --blocking
```

---

## Cross-Process Testing

### Basic Cross-Process (Same Host)

**Terminal 1 - Start receiver:**
```bash
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Terminal 2 - Start sender:**
```bash
ipc-benchmark -m tcp --run-mode sender --blocking \
  --host 127.0.0.1 -i 10000 -o results.json
```

### Cross-Process with UDS

**Terminal 1 - Receiver:**
```bash
ipc-benchmark -m uds --run-mode client --blocking
```

**Terminal 2 - Sender:**
```bash
ipc-benchmark -m uds --run-mode sender --blocking -i 10000
```

### Cross-Process with SHM

**Terminal 1 - Receiver:**
```bash
ipc-benchmark -m shm --run-mode client --blocking
```

**Terminal 2 - Sender:**
```bash
ipc-benchmark -m shm --run-mode sender --blocking -i 10000
```

---

## Host-to-Container (H2C)

### TCP (Easiest)

**Container - Start receiver:**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Host - Start sender:**
```bash
ipc-benchmark -m tcp --run-mode sender --blocking \
  --host <container-ip> -i 10000 -o h2c_tcp.json
```

### UDS (Shared Volume)

**Start container with shared volume:**
```bash
podman run -d --name mycontainer \
  -v /tmp/ipc_shared:/tmp/ipc_shared \
  <image>
```

**Container - Receiver:**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m uds --run-mode client --blocking
```

**Host - Sender:**
```bash
ipc-benchmark -m uds --run-mode sender --blocking -i 10000
```

### SHM (Shared /dev/shm)

**Start container with shared memory:**
```bash
podman run -d --name mycontainer \
  -v /dev/shm:/dev/shm \
  <image>
```

**Container - Receiver:**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m shm --run-mode client --blocking --cross-container
```

**Host - Sender:**
```bash
ipc-benchmark -m shm --run-mode sender --blocking \
  --cross-container -i 10000 -o h2c_shm.json
```

### PMQ (Shared IPC Namespace)

**Start container with shared IPC:**
```bash
podman run -d --name mycontainer \
  --ipc=host \
  -v /dev/mqueue:/dev/mqueue \
  <image>
```

**Container - Receiver:**
```bash
podman exec -it mycontainer /tmp/ipc-benchmark \
  -m pmq --run-mode client --blocking
```

**Host - Sender:**
```bash
ipc-benchmark -m pmq --run-mode sender --blocking -i 10000
```

---

## Container-to-Container (C2C)

### TCP

**Container 1 - Receiver:**
```bash
podman exec -it container1 /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

**Container 2 - Sender:**
```bash
podman exec -it container2 /tmp/ipc-benchmark \
  -m tcp --run-mode sender --blocking \
  --host <container1-ip> -i 10000 -o c2c_tcp.json
```

### SHM (Both Containers Share /dev/shm)

**Start both containers with shared memory:**
```bash
podman run -d --name container1 -v /dev/shm:/dev/shm <image>
podman run -d --name container2 -v /dev/shm:/dev/shm <image>
```

**Container 1 - Receiver:**
```bash
podman exec -it container1 /tmp/ipc-benchmark \
  -m shm --run-mode client --blocking --cross-container
```

**Container 2 - Sender:**
```bash
podman exec -it container2 /tmp/ipc-benchmark \
  -m shm --run-mode sender --blocking \
  --cross-container -i 10000 -o c2c_shm.json
```

---

## Output Options

### Summary Only (Console)

```bash
ipc-benchmark -m uds -i 10000
```

### JSON Summary File

```bash
ipc-benchmark -m uds -i 10000 -o results.json
```

### Streaming Output (Per-Message Data)

```bash
# JSON streaming
ipc-benchmark -m uds -i 10000 --streaming-output-json stream.json

# CSV streaming
ipc-benchmark -m uds -i 10000 --streaming-output-csv stream.csv

# Both
ipc-benchmark -m uds -i 10000 \
  --streaming-output-json stream.json \
  --streaming-output-csv stream.csv
```

### Full Output Set

```bash
ipc-benchmark -m uds -i 10000 \
  -o summary.json \
  --streaming-output-json streaming.json \
  --streaming-output-csv streaming.csv \
  --log-file benchmark.log \
  -v
```

---

## Performance-Focused Commands

### Lowest Latency (SHM Direct)

```bash
ipc-benchmark -m shm --shm-direct -i 100000 -w 10000 \
  --server-affinity 2 --client-affinity 4 \
  -o lowest_latency.json
```

### Highest Throughput

```bash
ipc-benchmark -m shm -i 100000 -s 65536 \
  --buffer-size 2097152 \
  --server-affinity 2 --client-affinity 4 \
  -o throughput.json
```

### Duration-Based Test

```bash
# Run for 60 seconds
ipc-benchmark -m uds -d 60s -o duration_test.json

# Run for 5 minutes
ipc-benchmark -m shm -d 5m -o long_test.json
```

### Controlled Latency (With Send Delay)

```bash
# One message every 10ms
ipc-benchmark -m uds -i 10000 --send-delay 10ms

# One message every 100μs
ipc-benchmark -m uds -i 100000 --send-delay 100us
```

---

## Test Type Selection

### One-Way Only

```bash
ipc-benchmark -m uds -i 10000 --one-way
```

### Round-Trip Only

```bash
ipc-benchmark -m uds -i 10000 --round-trip
```

### Both (Default)

```bash
ipc-benchmark -m uds -i 10000 --one-way --round-trip
```

---

## Comparison Commands

### Compare All Mechanisms

```bash
ipc-benchmark -m all -i 50000 -o mechanism_comparison.json
```

### Compare Async vs Blocking

```bash
# Async
ipc-benchmark -m uds -i 50000 -o async_uds.json

# Blocking
ipc-benchmark -m uds -i 50000 --blocking -o blocking_uds.json
```

### Compare SHM Ring Buffer vs Direct

```bash
# Ring buffer
ipc-benchmark -m shm -i 50000 --blocking -o shm_ring.json

# Direct
ipc-benchmark -m shm --shm-direct -i 50000 -o shm_direct.json
```

---

## Troubleshooting Commands

### Verbose Output

```bash
# Info level
ipc-benchmark -m uds -i 1000 -v

# Debug level
ipc-benchmark -m uds -i 1000 -vv

# Trace level (maximum)
ipc-benchmark -m uds -i 1000 -vvv
```

### Save Debug Log

```bash
ipc-benchmark -m uds -i 1000 -vv --log-file debug.log
```

### Clean Up Stale Resources

```bash
# Socket files
rm -f /tmp/ipc_benchmark_*

# Message queues
rm -f /dev/mqueue/ipc_benchmark_*

# Check shared memory
ipcs -m | grep ipc_benchmark
```

---

## Quick Comparison Table

| Scenario | Command |
|----------|---------|
| Simple UDS test | `ipc-benchmark -m uds -i 10000` |
| Blocking mode | `ipc-benchmark -m uds -i 10000 --blocking` |
| Lowest latency | `ipc-benchmark -m shm --shm-direct -i 10000` |
| All mechanisms | `ipc-benchmark -m all -i 10000` |
| Duration test | `ipc-benchmark -m uds -d 30s` |
| With CPU pinning | `ipc-benchmark -m uds -i 10000 --server-affinity 2 --client-affinity 4` |
| Cross-process receiver | `ipc-benchmark -m tcp --run-mode client --blocking` |
| Cross-process sender | `ipc-benchmark -m tcp --run-mode sender --blocking -i 10000` |
| Save JSON results | `ipc-benchmark -m uds -i 10000 -o results.json` |
| Streaming data | `ipc-benchmark -m uds -i 10000 --streaming-output-json stream.json` |

---

*For detailed explanations, see the [User Guide](user-guide/README.md).*
