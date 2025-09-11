# Host-to-Container IPC Benchmark Script

A unified script for running host-to-container IPC benchmarks across all three mechanisms (UDS, SHM, PMQ) with comprehensive parameter support. This script is specifically designed for cross-environment testing between host and containerized environments.

## ⚠️ Important: Host-to-Container Testing Only

This script is for **host-to-container cross-environment testing** (automotive safety ↔ non-safety domain communication). It runs in **host mode** and communicates with containerized servers.

### Testing Modes in the Codebase

| Mode | Description | Use Case |
|------|-------------|----------|
| **Standalone** | Single-process client+server | Traditional same-machine benchmarks |
| **Host** | Cross-environment coordinator | **This script** - Host side of host-to-container testing |
| **Client** | Containerized server | Container side (started with `start_*_container_server.sh`) |

For standalone testing (no containers), use: `./target/release/ipc-benchmark --mode standalone`

## Quick Start

```bash
# Make executable (first time only)
chmod +x run_host_container.sh

# Basic tests
./run_host_container.sh uds 1000      # UDS with 1000 messages  
./run_host_container.sh shm 30s       # SHM for 30 seconds
./run_host_container.sh pmq 5m 4096 2 # PMQ for 5 minutes, 4KB messages, 2 workers

# Get help
./run_host_container.sh --help
```

## Usage Syntax

```bash
./run_host_container.sh [MECHANISM] [MESSAGES/DURATION] [MESSAGE_SIZE] [WORKERS]
```

### Parameters

| Parameter | Description | Default | Examples |
|-----------|-------------|---------|----------|
| **MECHANISM** | IPC mechanism | `uds` | `uds`, `shm`, `pmq` |
| **MESSAGES/DURATION** | Message count OR duration | `1000` | `1000`, `30s`, `5m`, `1h` |
| **MESSAGE_SIZE** | Message size in bytes | `1024` | `512`, `1024`, `4096` |
| **WORKERS** | Concurrent workers | `1` | `1`, `2`, `4`, `8` |

### Mechanisms

- **`uds`** - Unix Domain Sockets (default)
- **`shm`** - Shared Memory  
- **`pmq`** - POSIX Message Queues

### Duration Formats

- **`30s`** - 30 seconds
- **`5m`** - 5 minutes  
- **`1h`** - 1 hour
- **`500ms`** - 500 milliseconds
- **`30`** - 30 seconds (default unit)

## Environment Variables

| Variable | Description | Default | Example |
|----------|-------------|---------|---------|
| `MECHANISM` | Override mechanism | - | `MECHANISM=shm` |
| `DURATION` | Test duration | - | `DURATION=30s` |
| `OUTPUT_FILE` | Output JSON file | `./output/host_{mechanism}_results.json` | `OUTPUT_FILE=./my_results.json` |
| `STREAM_JSON` | Streaming JSON output | - | `STREAM_JSON=./live.json` |
| `STREAM_CSV` | Streaming CSV output | - | `STREAM_CSV=./live.csv` |
| `SOCKET_PATH` | UDS socket path | `./sockets/ipc_benchmark.sock` | `SOCKET_PATH=/tmp/custom.sock` |
| `SHM_NAME` | Shared memory name | `ipc_benchmark_shm_crossenv` | `SHM_NAME=my_shm` |
| `BUFFER_SIZE` | SHM buffer size | `65536` | `BUFFER_SIZE=131072` |
| `ROUND_TRIP` | Enable round-trip testing | `false` | `ROUND_TRIP=true` |
| `VERBOSE` | Enable verbose output | `false` | `VERBOSE=true` |

## Example Commands

### Basic Usage

```bash
# Default UDS test (1000 messages, 1024 bytes)
./run_host_container.sh

# Specific mechanism with message count
./run_host_container.sh uds 500 2048 1

# Duration-based testing
./run_host_container.sh shm 30s 4096 2
./run_host_container.sh pmq 5m 1024 4
```

### Environment Variable Examples

```bash
# Custom output file and duration
DURATION=10s OUTPUT_FILE=./results/test1.json ./run_host_container.sh uds

# Round-trip testing with streaming
ROUND_TRIP=true STREAM_JSON=./live.json ./run_host_container.sh shm 1000 4096

# Verbose SHM with custom buffer
VERBOSE=true BUFFER_SIZE=131072 ./run_host_container.sh shm 30s 8192

# Custom socket path for UDS
SOCKET_PATH=/tmp/benchmark.sock ./run_host_container.sh uds 2000 512

# PMQ with custom output and duration
DURATION=2m OUTPUT_FILE=./pmq_results.json ./run_host_container.sh pmq
```

### Advanced Examples

```bash
# High-throughput SHM test
BUFFER_SIZE=262144 ./run_host_container.sh shm 1m 8192 4

# Long-running UDS benchmark with streaming
DURATION=1h STREAM_CSV=./hourly_uds.csv ROUND_TRIP=true ./run_host_container.sh uds 0 1024 2

# Comparative testing
for mechanism in uds shm pmq; do
    OUTPUT_FILE=./output/compare_${mechanism}.json ./run_host_container.sh $mechanism 30s 4096 1
done
```

## Container Server Management

Before running benchmarks, start the appropriate container server:

```bash
# Start container servers (pick one)
./start_uds_container_server.sh    # For UDS tests
./start_shm_container_server.sh    # For SHM tests  
./start_pmq_container_server.sh    # For PMQ tests

# Or use the general container script
MECHANISM=uds ./run_container_server.sh start
```

## Output Files

Results are saved as JSON files in `./output/` directory:

- **UDS**: `./output/host_uds_results.json`
- **SHM**: `./output/host_shm_results.json`  
- **PMQ**: `./output/host_pmq_results.json`

### View Results

```bash
# Pretty-print with jq
cat ./output/host_uds_results.json | jq

# Extract key metrics
jq '.throughput.messages_per_second' ./output/host_uds_results.json
jq '.latency_ns.p95' ./output/host_uds_results.json
```

## Migration from Old Scripts

Replace the old specific scripts with the new host-to-container script:

```bash
# Old way
./run_uds_host_client.sh 1000 1024 1
./run_shm_host_client.sh 1000 1024 1  
./run_pmq_host_client.sh 1000 1024 1

# New host-to-container way
./run_host_container.sh uds 1000 1024 1
./run_host_container.sh shm 1000 1024 1
./run_host_container.sh pmq 1000 1024 1
```

```bash
# Old way with environment variables
DURATION=30s OUTPUT_FILE=./custom.json ./run_host_client.sh 0 4096 1

# New host-to-container way (simpler)
DURATION=30s OUTPUT_FILE=./custom.json ./run_host_container.sh uds 0 4096 1
```

## Troubleshooting

### Common Issues

1. **Permission errors**: `chmod +x run_host_container.sh`
2. **Container not running**: Start the appropriate container server first
3. **Socket path issues**: Use `SOCKET_PATH` environment variable
4. **Build errors**: Run `cargo build --release` manually

### Debug Mode

```bash
# Enable verbose logging
VERBOSE=true ./run_host_container.sh uds 100 1024 1

# Check container logs
podman logs rusty-comms-uds-server
```

## Features

✅ **Unified Interface** - Single script for all mechanisms  
✅ **Smart Duration/Message Detection** - Automatic parameter parsing  
✅ **Environment Variable Support** - Flexible configuration  
✅ **Comprehensive Help** - Built-in documentation  
✅ **Colored Output** - Easy-to-read status messages  
✅ **Error Handling** - Graceful failure with clear messages  
✅ **Streaming Support** - Real-time JSON/CSV output  
✅ **Round-trip Testing** - Bidirectional benchmarking  
✅ **Custom Paths** - Configurable socket/shared memory paths  
