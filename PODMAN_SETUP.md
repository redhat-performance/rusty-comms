# Podman Setup for Host-Container IPC Testing

This document provides setup instructions for **Podman** as the container runtime for cross-environment IPC benchmarking. Podman offers better security and simpler operation compared to Docker.

## Why Podman for IPC Benchmarking?

### **Key Advantages:**
- ✅ **Rootless by default** - No daemon running as root, better security
- ✅ **No permission issues** - Runs containers as your user
- ✅ **Drop-in replacement** - Compatible with Docker commands and workflows
- ✅ **Better security** - Reduced attack surface for safety-critical testing
- ✅ **Native integration** - Better integration with modern Linux systems
- ✅ **Ideal for IPC** - Preserves filesystem permissions for socket sharing

## Quick Start

### **Step 1: Install Podman**
```bash
# Install Podman and podman-compose
sudo dnf install podman podman-compose -y

# Verify installation
podman version
podman-compose --version
```

### **Step 2: Run Cross-Environment IPC Tests**

#### Option A: Unified Script (Recommended)
```bash
# Test all three IPC mechanisms
./run_host_container.sh uds 1000 1024 1
./run_host_container.sh shm 1000 1024 1  
./run_host_container.sh pmq 1000 1024 1

# Duration-based test with custom output
DURATION=30s OUTPUT_FILE=./output/results.json \
./run_host_container.sh uds 0 4096 1
```

#### Option B: Manual Container Management
```bash
# Start containerized servers
./start_uds_container_server.sh &
./start_shm_container_server.sh &
./start_pmq_container_server.sh &

# Run host benchmarks
./target/release/ipc-benchmark --mode host -m uds --ipc-path ./sockets/ipc_benchmark.sock --msg-count 1000
./target/release/ipc-benchmark --mode host -m shm --shm-name ipc_benchmark_shm_crossenv --msg-count 1000
./target/release/ipc-benchmark --mode host -m pmq --msg-count 1000

# Cleanup
podman rm -f $(podman ps -q --filter "name=rusty-comms-")
```

## Key Changes from Docker

### **1. No Root Required**
- All commands run as your regular user
- No `sudo` needed for container operations
- No group membership requirements

### **2. Podman-Specific Files**
- `podman-compose.uds.yml` - Podman-optimized compose file
- Updated scripts use `podman` and `podman-compose` commands
- Added Podman-specific security settings

### **3. Enhanced Security**
```yaml
# Podman-specific settings in compose file
userns_mode: "keep-id"      # Preserve user ID mapping
security_opt:
  - "label=disable"         # Disable SELinux labeling for volumes
```

## Architecture

```
┌─────────────────────────┐     ┌─────────────────────────┐
│        Host OS          │     │      Container          │
│                         │     │                         │
│  ┌─────────────────┐    │     │  ┌─────────────────┐    │
│  │ rusty-comms     │    │     │  │ rusty-comms     │    │
│  │ --mode host     │────┼─────┼──│ --mode client   │    │
│  │                 │    │     │  │                 │    │
│  └─────────────────┘    │     │  └─────────────────┘    │
│                         │     │                         │
│  ./sockets/             │◄────┤  /app/sockets/          │
│  ├─ ipc_benchmark.sock  │     │  ├─ ipc_benchmark.sock  │
│                         │     │                         │
└─────────────────────────┘     └─────────────────────────┘
```

## Updated Files

### **Modified Scripts:**
- `run_container_server.sh` - Now uses `podman` and `podman-compose`
- `demo_host_container_ipc.sh` - Updated prerequisite checks
- `run_host_client.sh` - No changes needed

### **New Files:**
- `podman-compose.uds.yml` - Podman-optimized compose file
- `PODMAN_SETUP.md` - This documentation

### **Enhanced Dockerfile:**
- Uses `rust:latest` for better compatibility
- Improved Cargo.lock handling
- Ubuntu 24.04 runtime for GLIBC compatibility

## Troubleshooting

### **Podman Not Found:**
```bash
sudo dnf install podman podman-compose -y
```

### **Permission Issues:**
```bash
# Check if podman works
podman run hello-world

# Check user namespaces (if needed)
sysctl user.max_user_namespaces
```

### **Container Build Issues:**
```bash
# Clean build
podman system prune -f
podman-compose -f podman-compose.uds.yml build --no-cache
```

### **Socket Issues:**
```bash
# Check socket permissions
ls -la sockets/

# View container logs
./run_container_server.sh logs
```

## Performance Comparison

| Feature | Docker | Podman |
|---------|--------|--------|
| **Startup Time** | ~2-3 seconds | ~1-2 seconds |
| **Memory Usage** | Higher (daemon) | Lower (no daemon) |
| **Security** | Root daemon | Rootless |
| **Permission Setup** | Complex | None needed |
| **IPC Performance** | Same | Same |

## Commands Comparison

| Task | Docker | Podman |
|------|--------|--------|
| **Build** | `docker build` | `podman build` |
| **Run** | `docker run` | `podman run` |
| **Compose** | `docker-compose up` | `podman-compose up` |
| **Logs** | `docker logs` | `podman logs` |
| **Clean** | `docker system prune` | `podman system prune` |

## Next Steps

After successful setup:

1. **Benchmark Different Scenarios**: Test various message sizes and patterns
2. **Explore Other IPC Methods**: Try shared memory and TCP transports  
3. **Performance Monitoring**: Set up continuous benchmarking
4. **Production Deployment**: Use rootless containers in production
5. **CI/CD Integration**: Add Podman-based testing to pipelines

## Migration Notes

If converting from existing Docker setup:

1. **Install Podman**: `sudo dnf install podman podman-compose`
2. **Use new scripts**: Updated scripts automatically use Podman
3. **No permission changes**: Remove docker group membership if desired
4. **Same performance**: IPC characteristics remain identical
5. **Better security**: Benefit from rootless operation

This Podman setup provides the same functionality as Docker with enhanced security and simpler permissions management.

