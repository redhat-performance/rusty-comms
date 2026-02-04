# Tutorial: Cross-Process Testing

This tutorial shows how to run IPC benchmarks between separate processes, including across container boundaries.

## Objective

By the end of this tutorial, you will:
- Understand client/sender run modes
- Run benchmarks between separate terminals
- Test IPC across container boundaries

## Prerequisites

- Built `ipc-benchmark` binary
- Podman or Docker (for container tests)
- Two terminal windows

## Understanding Run Modes

| Mode | Role | Use Case |
|------|------|----------|
| `standalone` | Both processes on same host | Normal benchmarking (default) |
| `client` | Acts as IPC server/receiver | Inside containers |
| `sender` | Acts as IPC client/sender | Connect to remote server |

**Note:** Cross-process run modes currently require `--blocking`. The async
client/sender paths are not implemented yet, so `--run-mode client|sender`
without `--blocking` is not supported.

## Part 1: Basic Cross-Process Testing

### Step 1: Start the Receiver

In **Terminal 1**, start the receiver (client mode):

```bash
./target/release/ipc-benchmark \
  -m tcp \
  --run-mode client \
  --blocking \
  --host 0.0.0.0
```

The receiver will wait for a connection.

### Step 2: Run the Sender

In **Terminal 2**, run the sender:

```bash
./target/release/ipc-benchmark \
  -m tcp \
  --run-mode sender \
  --blocking \
  --host 127.0.0.1 \
  -i 10000 \
  -o results.json
```

### Step 3: View Results

The sender will display results and save to `results.json`. The receiver will exit automatically after the test.

## Part 2: Testing with Different Mechanisms

### TCP (Easiest)

```bash
# Terminal 1 (Receiver)
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0

# Terminal 2 (Sender)
ipc-benchmark -m tcp --run-mode sender --blocking --host 127.0.0.1 -i 10000
```

### Unix Domain Sockets

```bash
# Terminal 1 (Receiver)
ipc-benchmark -m uds --run-mode client --blocking

# Terminal 2 (Sender)
ipc-benchmark -m uds --run-mode sender --blocking -i 10000
```

### Shared Memory

```bash
# Terminal 1 (Receiver)
ipc-benchmark -m shm --run-mode client --blocking

# Terminal 2 (Sender)
ipc-benchmark -m shm --run-mode sender --blocking -i 10000 --one-way
```

**Note:** SHM cross-process typically works best with one-way tests.

## Part 3: Container Testing

### Step 1: Prepare Container

Create a simple container with the binary:

```bash
# Build the binary
cargo build --release

# Create a working container
podman run -d --name ipc-test \
  --network host \
  -v $(pwd)/target/release:/app:Z \
  fedora:latest \
  sleep infinity
```

### Step 2: Start Receiver in Container

```bash
podman exec -it ipc-test /app/ipc-benchmark \
  -m tcp \
  --run-mode client \
  --blocking \
  --host 0.0.0.0
```

### Step 3: Run Sender from Host

In another terminal:

```bash
./target/release/ipc-benchmark \
  -m tcp \
  --run-mode sender \
  --blocking \
  --host 127.0.0.1 \
  -i 10000 \
  -o host_to_container.json
```

### Step 4: Clean Up

```bash
podman stop ipc-test
podman rm ipc-test
```

## Part 4: Container-to-Container Testing

### Step 1: Create Network

```bash
podman network create ipc-net
```

### Step 2: Start Receiver Container

```bash
podman run -d --name receiver \
  --network ipc-net \
  -v $(pwd)/target/release:/app:Z \
  fedora:latest \
  /app/ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

### Step 3: Get Receiver IP

```bash
RECEIVER_IP=$(podman inspect receiver --format '{{.NetworkSettings.Networks.ipc-net.IPAddress}}')
echo "Receiver IP: $RECEIVER_IP"
```

### Step 4: Run Sender Container

```bash
podman run --rm --name sender \
  --network ipc-net \
  -v $(pwd)/target/release:/app:Z \
  -v $(pwd):/output:Z \
  fedora:latest \
  /app/ipc-benchmark -m tcp --run-mode sender --blocking \
    --host $RECEIVER_IP -i 10000 -o /output/c2c_results.json
```

### Step 5: Clean Up

```bash
podman stop receiver
podman rm receiver
podman network rm ipc-net
```

## Part 5: Shared Memory Across Containers

Shared memory requires special container configuration.

### Setup

```bash
# Receiver container with shared /dev/shm
podman run -d --name shm-receiver \
  --ipc=host \
  -v $(pwd)/target/release:/app:Z \
  fedora:latest \
  /app/ipc-benchmark -m shm --run-mode client --blocking

# Wait for server to start
sleep 2

# Sender from host
./target/release/ipc-benchmark \
  -m shm \
  --run-mode sender \
  --blocking \
  -i 10000 \
  --one-way \
  -o shm_results.json
```

**Important:** Use `--ipc=host` to share the IPC namespace.

## Troubleshooting

### Connection Refused

```
Error: Connection refused
```

**Solution:** Ensure the receiver is running and listening. Check the host/port.

### Timeout

```
Error: Timeout waiting for connection
```

**Solution:** 
- Check firewall rules
- Verify network connectivity
- Increase timeout if needed

### Shared Memory Errors

```
Error: Failed to open shared memory
```

**Solution:**
- Use `--ipc=host` for containers
- Check `/dev/shm` permissions
- Clean up stale segments: `rm /dev/shm/ipc_benchmark_*`

## Summary

Cross-process testing allows you to:
- Measure real IPC overhead between processes
- Test container networking performance
- Validate IPC across isolation boundaries

## Next Steps

- [Dashboard Analysis](dashboard-analysis.md) - Visualize cross-process results
- [Performance Tuning](../user-guide/performance-tuning.md) - Optimize for containers
- [Troubleshooting](../user-guide/troubleshooting.md) - Resolve issues
