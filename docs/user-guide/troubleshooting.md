# Troubleshooting

This guide covers common issues and their solutions when using the IPC Benchmark Suite.

## Quick Diagnostics

### Enable Verbose Logging

The first step for any issue is to enable verbose logging:

```bash
# Debug level
ipc-benchmark -m uds -i 1000 -vv

# Trace level (maximum detail)
ipc-benchmark -m uds -i 1000 -vvv

# Save to log file
ipc-benchmark -m uds -i 1000 -vv --log-file debug.log
```

## Common Issues

### Binary Not Found

**Error:**
```
Could not resolve 'ipc-benchmark' binary for server mode
```

**Solution:**
Build the project first:
```bash
cargo build --release
```

Or run with the full path:
```bash
./target/release/ipc-benchmark -m uds
```

### Permission Denied

**Error:**
```
Permission denied (os error 13)
```

**Solutions:**

1. Check directory permissions:
```bash
# Ensure write access to current directory
ls -la .

# Ensure write access to /tmp
ls -la /tmp
```

2. Clean up stale files:
```bash
rm -f /tmp/ipc_benchmark_*
```

3. Run with appropriate permissions (not typically needed):
```bash
sudo ./target/release/ipc-benchmark -m uds
```

### Address Already in Use

**Error:**
```
Address already in use (os error 98)
```

**Solutions:**

1. Wait for previous test to complete
2. Use a different port:
```bash
ipc-benchmark -m tcp --port 9090
```
3. Check for orphan processes:
```bash
ps aux | grep ipc-benchmark
kill <pid>
```

## Mechanism-Specific Issues

### Unix Domain Sockets (UDS)

#### Socket File Remains After Crash

**Problem:** Stale socket files prevent new tests.

**Solution:**
```bash
rm -f /tmp/ipc_benchmark_*.sock
```

#### Not Available on Windows

**Error:**
```
Unix domain sockets are not available on this platform
```

**Solution:** Use TCP sockets instead:
```bash
ipc-benchmark -m tcp -i 10000
```

### Shared Memory (SHM)

#### Unexpected End of File

**Error:**
```
unexpected end of file
```

**Causes:**
- Buffer size too small
- Concurrency race conditions

**Solution:**
```bash
# Increase buffer size
ipc-benchmark -m shm -i 10000 --buffer-size 1048576
```

#### Timeout Sending Message

**Error:**
```
Timeout sending message
```

**Solution:** Increase buffer size or reduce concurrency:
```bash
ipc-benchmark -m shm -i 10000 --buffer-size 2097152
```

#### Backpressure Warnings

**Warning:**
```
WARN rusty_comms::ipc::shared_memory: Shared memory buffer is full; backpressure is occurring.
```

**This is informational.** It means the sender is faster than the receiver. Options:

1. Increase buffer size (if testing throughput):
```bash
ipc-benchmark -m shm -i 10000 --buffer-size 1048576
```

2. Add send delay (if testing latency):
```bash
ipc-benchmark -m shm -i 10000 --send-delay 10ms
```

3. Accept backpressure as a valid test scenario.

### TCP Sockets

#### Connection Refused

**Error:**
```
Connection refused (os error 111)
```

**Causes:**
- Server not started (in cross-process mode)
- Wrong host/port

**Solutions:**

1. Start the server first:
```bash
# Terminal 1
ipc-benchmark -m tcp --run-mode client --blocking --host 0.0.0.0
```

2. Verify host/port:
```bash
# Terminal 2
ipc-benchmark -m tcp --run-mode sender --blocking --host 127.0.0.1 --port 8080
```

### POSIX Message Queues (PMQ)

#### PMQ Not Available

**Error:**
```
POSIX Message Queues are not available on this platform
```

**Solution:** PMQ is Linux-only. Use another mechanism on other platforms.

#### Message Too Long

**Error:**
```
Message too long (os error 90)
```

**Cause:** Message size exceeds kernel limit.

**Solution:**

1. Check current limits:
```bash
cat /proc/sys/fs/mqueue/msgsize_max
```

2. Increase limit (requires root):
```bash
sudo sysctl -w fs.mqueue.msgsize_max=16384
```

3. Or use smaller messages:
```bash
ipc-benchmark -m pmq -s 8000 -i 10000
```

#### /dev/mqueue Not Mounted

**Error:**
```
No such file or directory (os error 2)
```

**Solution:**
```bash
sudo mkdir -p /dev/mqueue
sudo mount -t mqueue none /dev/mqueue
```

#### Queue Already Exists

**Error:**
```
File exists (os error 17)
```

**Solution:** Clean up stale queues:
```bash
ls /dev/mqueue/
rm /dev/mqueue/ipc_benchmark_*
```

#### SELinux Blocks PMQ in QM Containers

**Error:**
```
Permission denied (os error 13)
```
or PMQ tests time out waiting for queues to appear.

**Cause:** The `qm_t` SELinux domain blocks all POSIX message queue
system calls (`mq_open`, `mq_send`, `mq_receive`). This affects
H2QM and QM-C2C PMQ benchmarks when SELinux is in Enforcing mode.

**Solution:**

1. **With the test runner** — pass `--allow-selinux-permissive`:
```bash
python3 utils/comprehensive-rusty-comms-testing.py --allow-selinux-permissive
```
Without the flag, affected PMQ tests are automatically skipped.

2. **Running manually** — temporarily disable enforcement:
```bash
# Disable enforcement (immediate, system-wide)
setenforce 0

# Run your PMQ benchmark...

# Restore enforcement when done
setenforce 1
```

3. **Check current mode:**
```bash
getenforce
```

> **Note:** `setenforce` changes do not persist across reboots.
> The permanent SELinux mode is set in `/etc/selinux/config`.

## Performance Issues

### Inconsistent Results

**Problem:** Latency measurements vary between runs.

**Solutions:**

1. Use CPU affinity:
```bash
ipc-benchmark -m uds -i 10000 \
  --server-affinity 0 --client-affinity 1
```

2. Increase warmup iterations:
```bash
ipc-benchmark -m uds -i 10000 -w 5000
```

3. Disable CPU frequency scaling:
```bash
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

4. Run on idle system

### High Maximum Latency

**Problem:** P99.9 or max latency is much higher than expected.

**Causes:**
- OS scheduling jitter
- CPU frequency changes
- System interrupts

**Solutions:**

1. Use CPU affinity (most effective)
2. Disable turbo boost:
```bash
echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo
```
3. Use `--shm-direct` for shared memory (450x better tail latency)

### First Message Spike

**Problem:** First message has very high latency.

**This is expected.** Cold-start effects include:
- CPU cache misses
- Memory allocation
- Branch prediction misses

**Solution:** The tool automatically discards the first message. To include it:
```bash
ipc-benchmark -m uds -i 10000 --include-first-message
```

## Cross-Process Issues

### Container Can't Connect

**Problem:** Sender can't reach receiver in container.

**Solutions:**

1. Check container IP:
```bash
podman inspect <container> | grep IPAddress
```

2. Use correct host binding in container:
```bash
podman exec <container> /tmp/ipc-benchmark \
  -m tcp --run-mode client --blocking --host 0.0.0.0
```

3. For SHM, share `/dev/shm`:
```bash
podman run --volume /dev/shm:/dev/shm ...
```

4. For PMQ, share IPC namespace:
```bash
podman run --ipc=host --volume /dev/mqueue:/dev/mqueue ...
```

## Debugging Commands

### Check System Limits

```bash
# Shared memory
ipcs -lm

# Message queues
cat /proc/sys/fs/mqueue/msg_max
cat /proc/sys/fs/mqueue/msgsize_max

# Open files
ulimit -n
```

### Check for Stale Resources

```bash
# Stale socket files
ls -la /tmp/ipc_benchmark_*

# Stale message queues
ls /dev/mqueue/

# Stale shared memory segments
ipcs -m | grep ipc_benchmark
```

### Clean Up Resources

```bash
# Remove socket files
rm -f /tmp/ipc_benchmark_*

# Remove message queues
rm /dev/mqueue/ipc_benchmark_* 2>/dev/null

# Remove shared memory (find IPC key first)
ipcs -m | grep ipc_benchmark
ipcrm -m <shmid>
```

## Getting Help

If issues persist:

1. Run with maximum verbosity:
```bash
ipc-benchmark -m uds -i 100 -vvv --log-file detailed.log 2>&1 | tee output.txt
```

2. Capture system info:
```bash
uname -a
cat /etc/os-release
rustc --version
cargo --version
```

3. Check the [GitHub Issues](https://github.com/redhat-performance/rusty-comms/issues)

4. Open a new issue with:
   - Command used
   - Error message
   - Log file contents
   - System information
