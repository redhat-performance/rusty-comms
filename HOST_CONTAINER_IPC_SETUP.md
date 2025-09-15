# Host-Container IPC Setup

This document provides comprehensive setup instructions for cross-environment IPC testing between host and container environments. The benchmark suite supports **all three IPC mechanisms** (UDS, SHM, PMQ) for host-to-container communication.

## Overview

This configuration enables real-world IPC performance testing across environment boundaries:
- **Host Environment**: Safety domain running the benchmark client
- **Container Environment**: Non-safety domain running the IPC server
- **Communication**: Via Unix Domain Sockets, Shared Memory, or POSIX Message Queues
- **Orchestration**: Automated container management with built-in cross-environment coordination

## Quick Start

### Method 1: Unified Script (Recommended)

The `run_host_container.sh` script automatically manages containers and runs benchmarks:

```bash
# Unix Domain Sockets: 1000 messages, 1024 bytes, 1 worker
./run_host_container.sh uds 1000 1024 1

# Shared Memory: duration-based test with 4KB messages
DURATION=30s ./run_host_container.sh shm 0 4096 1

# POSIX Message Queues: with streaming CSV output
STREAM_CSV=./output/pmq_stream.csv ./run_host_container.sh pmq 5000 512 1

# All mechanisms comparison
./run_host_container.sh uds 1000 1024 1
./run_host_container.sh shm 1000 1024 1
./run_host_container.sh pmq 1000 1024 1
```

### Method 2: Manual Step-by-Step

For advanced users who need manual control:

#### 1. Start the Containerized Server

Choose the appropriate container server for your IPC mechanism:

```bash
# Unix Domain Sockets
./start_uds_container_server.sh &

# Shared Memory
./start_shm_container_server.sh &

# POSIX Message Queues
./start_pmq_container_server.sh &
```

#### 2. Run the Host Driver

```bash
# UDS: Message-count mode
./target/release/ipc-benchmark --mode host -m uds \
  --ipc-path ./sockets/ipc_benchmark.sock \
  --message-size 1024 --msg-count 1000

# SHM: Duration mode (30 seconds)
./target/release/ipc-benchmark --mode host -m shm \
  --shm-name ipc_benchmark_shm_crossenv \
  --message-size 1024 --duration 30s

# PMQ: With custom output
./target/release/ipc-benchmark --mode host -m pmq \
  --message-size 512 --msg-count 5000 \
  --output-file ./output/pmq_results.json
```

#### 3. View Results
```bash
# Check output directory
ls -la output/

# View JSON results
cat output/host_*_results.json | jq .

# View streaming CSV (if enabled)
head -20 output/*.csv
```

#### 4. Cleanup
```bash
# Stop specific containers
podman rm -f rusty-comms-uds-server
podman rm -f rusty-comms-shm-server
podman rm -f rusty-comms-pmq-server

# Or stop all benchmark containers
podman rm -f $(podman ps -q --filter "name=rusty-comms-")
```

## Architecture

```
┌─────────────────────────┐     ┌─────────────────────────┐
│        Host OS          │     │      Container          │
│                         │     │                         │
│  ┌─────────────────┐    │     │  ┌─────────────────┐    │
│  │ rusty-comms     │    │     │  │ rusty-comms     │    │
│  │ --mode client   │────┼─────┼──│ --mode host     │    │
│  │                 │    │     │  │                 │    │
│  └─────────────────┘    │     │  └─────────────────┘    │
│                         │     │                         │
│  ./sockets/             │◄────┤  /app/sockets/          │
│  ├─ ipc_benchmark.sock  │     │  ├─ ipc_benchmark.sock  │
│                         │     │                         │
└─────────────────────────┘     └─────────────────────────┘
              ▲                           ▲
              │                           │
         Unix Domain Socket Volume Mount
```

## Key Features

### Built-in Cross-Environment Support
Rusty-comms includes native support for cross-environment IPC testing:

- **Host Mode** (`--mode host`): Coordinates multiple server processes and waits for client connections
- **Client Mode** (`--mode client`): Connects to host coordinators and participates in benchmarks  
- **IPC Path** (`--ipc-path`): Specifies shared socket path for communication

### Volume Mounting Strategy
- **Host side**: `./sockets/` directory
- **Container side**: `/app/sockets/` directory  
- **Socket file**: `ipc_benchmark.sock` (shared between both)

### Performance Characteristics
Unix Domain Sockets provide:
- ✅ **High performance** - kernel bypasses network stack
- ✅ **Built-in access control** - filesystem permissions
- ✅ **Container compatibility** - works seamlessly across container boundary
- ✅ **Multi-client support** - can handle concurrent connections

## Files and Scripts

### Core Scripts
- `run_container_server.sh` - Container server management
- `run_host_client.sh` - Host driver (host mode)
- `podman-compose.uds.yml` - Container orchestration (Podman)

### Configuration Files
- `Dockerfile` - Optimized for UDS host-container communication
- `HOST_CONTAINER_IPC_SETUP.md` - This documentation

### Generated Artifacts
- `sockets/` - Shared socket directory
- `output/` - Benchmark results directory
- `output/host_client_results.json` - Client-side results
- `output/container_server_results.json` - Server-side results (if available)

## Parameters and Customization

### Available Parameters
```bash
# Message count and size
./run_host_client.sh 5000 2048 1

# Container server settings (messages and message size are controlled by docker-compose)
./run_container_server.sh start
```

### Environment Variables
The setup uses these environment variables:

**Container:**
- `RUST_LOG=debug` - Enable detailed logging
- `IPC_BENCHMARK_SOCKET_DIR=/app/sockets` - Socket directory
- `IPC_BENCHMARK_OUTPUT_DIR=/app/output` - Results directory

**Host:**
- `RUST_LOG=info` - Standard logging level  
- `IPC_BENCHMARK_OUTPUT_DIR=./output` - Local results directory
- Optional env for run_host_client.sh:
  - `DURATION=30s` (overrides message count)
  - `OUTPUT_FILE=./output/custom.json`
  - `STREAM_JSON=./output/lat_stream.json`
  - `STREAM_CSV=./output/lat_stream.csv`

## Cross-Environment Coordination

### Built-in Coordination Features
Rusty-comms automatically handles:
- **Process Synchronization** - Host coordinator waits for client connections
- **Connection Management** - Proper setup and teardown of IPC resources
- **Result Collection** - Aggregation of results from multiple processes
- **Error Handling** - Graceful failure handling and cleanup

### Command-Line Mapping
Traditional vs. Cross-Environment parameters:

| Traditional | Cross-Environment | Purpose |
|-------------|-------------------|---------|
| `--role server` | `--mode host` | Run as host coordinator |
| `--role client` | `--mode client` | Run as client process |
| `--socket-path` | `--ipc-path` | Specify socket file path |
| `--messages` | `--msg-count` | Number of messages to send |
| `--workers` | `--concurrency` | Number of concurrent workers |

## Troubleshooting

### Common Issues

**Socket not found:**
```bash
# Check if container is running
podman ps | grep rusty-comms-uds-server

# Check socket directory
ls -la sockets/

# View container logs
./run_container_server.sh logs
```

**Permission issues:**
```bash
# Check socket permissions
ls -la sockets/ipc_benchmark.sock

# Fix directory permissions (host side)
chmod 777 sockets/
```

**Container build issues:**
```bash
# Force rebuild
./run_container_server.sh build

# Check build logs
docker build --no-cache .
```

### Debug Mode
Enable verbose logging:
```bash
# Container side (edit podman-compose.uds.yml if needed)
# RUST_LOG=trace

# Host side
RUST_LOG=debug ./run_host_client.sh 1000 1024 1
```

## Performance Expectations

Typical performance characteristics for Unix Domain Sockets:

- **Latency**: 10-50 microseconds
- **Throughput**: 1-10 GB/s (depending on message size)
- **CPU Overhead**: Low (kernel-optimized path)
- **Memory Usage**: Minimal (no network buffers)

## Next Steps

After successful setup, consider:

1. **Experiment with different message sizes**: Test with various payload sizes
2. **Try other IPC mechanisms**: Shared memory (`--mechanisms shared-memory`)  
3. **Scale testing**: Increase concurrency and message counts
4. **Automate testing**: Integrate into CI/CD pipelines
5. **Monitor performance**: Set up continuous performance monitoring

## Security Considerations

- **Volume Mounts**: Only mount necessary directories
- **Container User**: Container runs as non-root `ipc-benchmark` user
- **Socket Permissions**: File system controls access to socket
- **Network Isolation**: No network ports exposed (pure local IPC)

This setup provides a solid foundation for cross-environment IPC performance testing while maintaining security and ease of use.

