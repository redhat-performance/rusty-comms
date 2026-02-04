# Performance Tuning

This guide covers system configuration and best practices for accurate, reproducible benchmark results.

## Why Tuning Matters

Without proper tuning, benchmark results can vary significantly due to:
- CPU frequency scaling
- Process scheduling
- Cache effects
- Background system activity

## CPU Configuration

### Disable Frequency Scaling

CPU frequency scaling causes measurement variability. Set to performance mode:

```bash
# Check current governor
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Set to performance (requires root)
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

### Disable Turbo Boost

Turbo boost can cause inconsistent results:

```bash
# Intel CPUs
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo

# AMD CPUs
echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost
```

### CPU Affinity

Pin processes to specific cores to avoid migration:

```bash
ipc-benchmark -m uds -i 10000 \
  --server-affinity 2 \
  --client-affinity 4
```

**Best practices:**
- Use different physical cores (not hyperthreads)
- Avoid core 0 (often handles system interrupts)
- Check topology with `lscpu` or `lstopo`

## Memory Configuration

### Shared Memory Limits

Check and increase shared memory limits:

```bash
# Check current limits
ipcs -lm

# Increase maximum segment size (2GB)
echo 2147483648 | sudo tee /proc/sys/kernel/shmmax

# Increase maximum segments
echo 4096 | sudo tee /proc/sys/kernel/shmmni
```

### Huge Pages

Enable huge pages for better memory performance:

```bash
# Enable transparent huge pages
echo always | sudo tee /sys/kernel/mm/transparent_hugepage/enabled

# Or allocate explicit huge pages
echo 1024 | sudo tee /proc/sys/vm/nr_hugepages
```

## Network Configuration (TCP)

### Increase Buffer Sizes

```bash
# Maximum receive buffer
echo 16777216 | sudo tee /proc/sys/net/core/rmem_max

# Maximum send buffer
echo 16777216 | sudo tee /proc/sys/net/core/wmem_max

# TCP buffer range
echo "4096 87380 16777216" | sudo tee /proc/sys/net/ipv4/tcp_rmem
echo "4096 65536 16777216" | sudo tee /proc/sys/net/ipv4/tcp_wmem
```

## POSIX Message Queue Configuration

### Check Limits

```bash
cat /proc/sys/fs/mqueue/msg_max      # Max messages per queue
cat /proc/sys/fs/mqueue/msgsize_max  # Max message size
```

### Increase Limits

```bash
# Increase message size limit
echo 16384 | sudo tee /proc/sys/fs/mqueue/msgsize_max

# Increase queue depth
echo 100 | sudo tee /proc/sys/fs/mqueue/msg_max
```

### Mount Message Queue Filesystem

```bash
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue
```

## Benchmark Configuration

### Warmup Iterations

Use warmup to stabilize measurements:

```bash
# Default is 1000, increase for more stability
ipc-benchmark -m uds -i 100000 -w 10000
```

### Message Count vs Duration

**Message count** for consistent comparisons:
```bash
ipc-benchmark -m uds -i 100000
```

**Duration** for time-consistent tests:
```bash
ipc-benchmark -m uds -d 60s
```

### Buffer Size

For throughput tests, ensure buffers are large enough:

```bash
ipc-benchmark -m shm -i 100000 --buffer-size 1048576
```

### Send Delay for Latency Testing

Avoid backpressure effects when measuring latency:

```bash
ipc-benchmark -m uds -i 10000 --send-delay 10ms
```

## System Preparation

### Before Benchmarking

1. **Close unnecessary applications**
2. **Stop background services** (if possible)
3. **Check system load**: `uptime`
4. **Check for I/O activity**: `iostat -x 1`
5. **Check memory usage**: `free -h`

### Reproducibility Checklist

```bash
# 1. Set CPU governor
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# 2. Disable turbo
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null || true

# 3. Check load
uptime

# 4. Run benchmark with affinity
ipc-benchmark -m shm --shm-direct -i 100000 \
  --server-affinity 2 --client-affinity 4 \
  -w 10000 \
  -o results.json
```

## Compiler Optimizations

### Release Build

Always use release builds for benchmarking:

```bash
cargo build --release
```

### Native CPU Optimization

Build for your specific CPU:

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

### Link-Time Optimization

Enable LTO for maximum performance:

```bash
RUSTFLAGS="-C lto=fat" cargo build --release
```

## Interpreting Results

### Expected Variability

| Metric | Acceptable Variance |
|--------|---------------------|
| Mean latency | < 5% |
| P99 latency | < 10% |
| Throughput | < 5% |

### Signs of Problems

- **High max latency**: System interference
- **Bimodal distribution**: CPU frequency changes
- **Increasing latency over time**: Memory pressure
- **Inconsistent runs**: Insufficient warmup

### Multiple Runs

Run benchmarks multiple times and average:

```bash
for i in 1 2 3 4 5; do
  ipc-benchmark -m uds -i 100000 -o "run_${i}.json"
done
```

## See Also

- [Advanced Usage](advanced-usage.md) - CPU affinity details
- [Troubleshooting](troubleshooting.md) - Performance issues
- [IPC Mechanisms](ipc-mechanisms.md) - Mechanism-specific tuning
