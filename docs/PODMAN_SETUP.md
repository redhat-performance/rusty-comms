# Podman Setup for IPC Benchmarking

This guide explains how to set up Podman for running host-to-container IPC benchmarks.

## Prerequisites

### Install Podman

**Fedora/RHEL 9+:**
```bash
sudo dnf install podman
```

**Ubuntu/Debian:**
```bash
sudo apt-get install podman
```

**Verify installation:**
```bash
podman --version
# Expected: podman version 4.x.x or later
```

### Rootless Podman (Recommended)

The benchmark works with rootless Podman, which doesn't require root privileges:

```bash
# Verify rootless mode works
podman run --rm alpine echo "Rootless Podman works!"
```

## Building the Container Image

### Using the Provided Containerfile

Build the container image from the project root:

```bash
cd /path/to/rusty-comms

# Build the image
podman build -t localhost/ipc-benchmark:latest .
```

### Verify the Image

```bash
# Check image exists
podman images | grep ipc-benchmark

# Test the image
podman run --rm localhost/ipc-benchmark:latest --help
```

### Sample Containerfile

If you need to create your own Containerfile:

```dockerfile
# Containerfile for IPC Benchmark
FROM rust:1.70 as builder

WORKDIR /build
COPY . .

# Build release binary
RUN cargo build --release

# Runtime image
FROM fedora:39

# Install minimal dependencies
RUN dnf install -y glibc && dnf clean all

# Copy the benchmark binary
COPY --from=builder /build/target/release/ipc-benchmark /usr/local/bin/

ENTRYPOINT ["/usr/local/bin/ipc-benchmark"]
```

## Container Configuration by IPC Mechanism

Each IPC mechanism requires specific container configuration. These are handled automatically by `--run-mode host`, but understanding them helps with troubleshooting.

### Unix Domain Sockets (UDS)

**Requirement:** Shared socket directory

```bash
# Volume mount for UDS
podman run -v /tmp/rusty-comms:/tmp/rusty-comms:rw \
    localhost/ipc-benchmark:latest \
    --run-mode client -m uds --socket-path /tmp/rusty-comms/test.sock
```

**Host setup:**
```bash
# Create socket directory (done automatically)
mkdir -p /tmp/rusty-comms
chmod 755 /tmp/rusty-comms
```

### Shared Memory (SHM)

**Option 1: Share IPC namespace (recommended)**

```bash
podman run --ipc=host \
    localhost/ipc-benchmark:latest \
    --run-mode client -m shm --shared-memory-name /ipc-bench-test
```

**Option 2: Mount /dev/shm directly**

```bash
podman run -v /dev/shm:/dev/shm:rw \
    localhost/ipc-benchmark:latest \
    --run-mode client -m shm --shared-memory-name /ipc-bench-test
```

**Notes:**
- `--ipc=host` shares the entire IPC namespace (shm, semaphores, message queues)
- Direct `/dev/shm` mount provides more isolation
- Both host and container see the same shared memory segments

### POSIX Message Queues (PMQ)

**Requirement:** Mount mqueue filesystem + privileged mode

```bash
podman run --privileged \
    -v /dev/mqueue:/dev/mqueue:rw \
    localhost/ipc-benchmark:latest \
    --run-mode client -m pmq --message-queue-name /ipc-bench-test
```

**Notes:**
- PMQ requires access to `/dev/mqueue`
- `--privileged` may be needed for mqueue syscalls
- Queue depth is limited by kernel settings

### TCP Sockets

**Requirement:** Host network mode

```bash
podman run --network=host \
    localhost/ipc-benchmark:latest \
    --run-mode client -m tcp --host 127.0.0.1 --port 45000
```

**Notes:**
- `--network=host` shares the host's network stack
- Container can directly access localhost ports
- No port mapping required

## Verifying Container Setup

### Quick Test

```bash
# List containers managed by the benchmark
./target/release/ipc-benchmark --list-containers

# If any exist, stop them
./target/release/ipc-benchmark --stop-container all
```

### Manual Container Test

```bash
# Start a container manually
podman run -it --rm \
    -v /tmp/rusty-comms:/tmp/rusty-comms:rw \
    localhost/ipc-benchmark:latest \
    --help
```

## Troubleshooting

### Image Not Found

```
Error: Container image 'localhost/ipc-benchmark:latest' not found.
```

**Solution:** Build the image:
```bash
podman build -t localhost/ipc-benchmark:latest .
```

### Permission Denied for Socket

```
Error: Permission denied creating socket at /tmp/rusty-comms/test.sock
```

**Solution:** Check directory permissions:
```bash
ls -la /tmp/rusty-comms/
chmod 755 /tmp/rusty-comms
```

### Shared Memory Not Found

```
Error: Timeout opening shared memory '/ipc-bench-shm-xxx'
```

**Solutions:**
1. Ensure `--ipc=host` is used for the container
2. Check if the server is running: `ls /dev/shm/ | grep ipc-bench`
3. Clean up stale segments: `rm /dev/shm/ipc-bench-*`

### PMQ Permission Error

```
Error: EMFILE: Too many open files
```

**Solutions:**
1. Increase message queue limits:
   ```bash
   sudo sysctl fs.mqueue.msg_max=100
   sudo sysctl fs.mqueue.queues_max=256
   ```
2. Use `--privileged` flag for the container
3. Clean up stale queues: `rm /dev/mqueue/ipc-bench-*`

### Container Fails to Start

```
Error: OCI runtime error
```

**Solutions:**
1. Check Podman is working: `podman run --rm alpine echo test`
2. Verify image: `podman images`
3. Check container logs: `podman logs <container-name>`

## Kernel Limits

For high-performance testing, you may need to adjust kernel limits:

```bash
# Shared memory limits
sudo sysctl kernel.shmmax=1073741824  # 1GB
sudo sysctl kernel.shmall=262144       # 1GB in pages

# Message queue limits
sudo sysctl fs.mqueue.msg_max=100
sudo sysctl fs.mqueue.msgsize_max=8192
sudo sysctl fs.mqueue.queues_max=256

# File descriptor limits (for many connections)
ulimit -n 65536
```

## SELinux Considerations (RHEL/Fedora)

If SELinux is enabled, you may need additional configuration:

```bash
# Check SELinux status
getenforce

# Option 1: Use :z or :Z suffix for volume mounts
podman run -v /tmp/rusty-comms:/tmp/rusty-comms:z ...

# Option 2: Create policy exception (advanced)
ausearch -c 'ipc-benchmark' --raw | audit2allow -M ipc-benchmark
semodule -i ipc-benchmark.pp
```

## Next Steps

After setup, see [HOST_CONTAINER_USAGE.md](HOST_CONTAINER_USAGE.md) for running benchmarks.

