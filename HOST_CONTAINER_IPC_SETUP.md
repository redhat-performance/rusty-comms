# Host-Container IPC Setup with Unix Domain Sockets

This setup demonstrates Option 1 for cross-environment IPC testing: **Unix Domain Sockets** for host-to-container communication.

## Overview

This configuration runs a UDS server inside a Podman container and the benchmark driver on the host. Communication occurs via a Unix Domain Socket shared through a bind mount. This uses rusty-comms' built-in cross-environment coordination.

## Quick Start

### Manual Step-by-Step

#### 1. Start the Containerized Server (Client Mode)
```bash
# Start container server
./run_container_server.sh start

# Check status
./run_container_server.sh status
```

#### 2. Run the Host Driver (Host Mode)
```bash
# Message-count mode
./run_host_client.sh 1000 1024 1

# Duration mode (example: 30 seconds)
DURATION=30s ./run_host_client.sh 0 1024 1

# With custom output and per-message streaming
OUTPUT_FILE=./output/host_run.json \
STREAM_JSON=./output/lat_stream.json \
STREAM_CSV=./output/lat_stream.csv \
./run_host_client.sh 10000 1024 1
```

#### 3. View Results
```bash
# Check output directory
ls -la output/

# View results
ls -la output/
cat output/host_client_results.json
```

#### 4. Cleanup
```bash
# Stop the container
./run_container_server.sh stop
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

