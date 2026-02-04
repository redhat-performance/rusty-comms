# Async vs Blocking Execution Modes

This document explains the two execution modes available in rusty-comms and their implications for benchmarking.

## Overview

| Mode | Runtime | I/O Model | Flag |
|------|---------|-----------|------|
| Async | Tokio | Non-blocking | (default) |
| Blocking | std | Blocking | `--blocking` |

## Async Mode (Default)

### Technology

Uses the [Tokio](https://tokio.rs/) async runtime with Rust's async/await syntax.

```rust
// Async code example
async fn send_message(stream: &mut TcpStream, msg: &Message) -> Result<()> {
    let data = bincode::serialize(msg)?;
    stream.write_all(&data).await?;
    Ok(())
}
```

### Characteristics

- **Non-blocking I/O**: Operations don't block the thread
- **Task-based concurrency**: Uses `tokio::spawn`
- **Event-driven**: Efficiently handles many connections
- **Runtime overhead**: Tokio runtime has some overhead

### When Async is Used

```bash
# Default - async mode
ipc-benchmark -m uds -i 10000

# Explicitly not blocking
ipc-benchmark -m uds -i 10000  # same as above
```

### Implementation

```rust
// Uses tokio types
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run_server() {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    let (stream, _) = listener.accept().await?;
    // ...
}
```

## Blocking Mode

### Technology

Uses the Rust standard library with traditional blocking I/O.

```rust
// Blocking code example
fn send_message(stream: &mut TcpStream, msg: &Message) -> Result<()> {
    let data = bincode::serialize(msg)?;
    stream.write_all(&data)?;  // Blocks until complete
    Ok(())
}
```

### Characteristics

- **Blocking I/O**: Operations block until complete
- **Thread-based**: Uses `std::thread::spawn`
- **Simple model**: Easier to reason about
- **No runtime**: Pure standard library, no Tokio overhead

### When Blocking is Used

```bash
# Blocking mode
ipc-benchmark -m uds -i 10000 --blocking

# Direct shared memory (auto-enables blocking)
ipc-benchmark -m shm --shm-direct -i 10000
```

### Implementation

```rust
// Uses std types
use std::net::TcpListener;
use std::io::{Read, Write};

fn run_server() {
    let listener = TcpListener::bind("127.0.0.1:8080")?;
    let (stream, _) = listener.accept()?;  // Blocks here
    // ...
}
```

## Performance Comparison

### When Async is Faster

- High concurrency (100+ connections)
- Mixed workloads with waiting
- Network I/O bound tasks

### When Blocking is Faster

- Low concurrency (1-4 threads)
- CPU-bound operations
- Simple request-response patterns
- Avoiding runtime overhead matters

### Typical Results

For single-threaded IPC benchmarks:

| Metric | Async | Blocking |
|--------|-------|----------|
| Mean latency | Similar | Similar |
| Min latency | Higher | Lower |
| Overhead | Tokio runtime | None |

## Comparing Modes

### Fair Comparison

To fairly compare modes:

```bash
# Same parameters, different modes
./ipc-benchmark -m uds -i 50000 -o async.json
./ipc-benchmark -m uds -i 50000 --blocking -o blocking.json

# Compare results
cat async.json | jq '.results[0].one_way_results.latency.mean_ns'
cat blocking.json | jq '.results[0].one_way_results.latency.mean_ns'
```

### Test All Mechanisms

```bash
for mode in "" "--blocking"; do
  mode_name=$([ -z "$mode" ] && echo "async" || echo "blocking")
  ./ipc-benchmark -m all -i 50000 $mode -o "${mode_name}_results.json"
done
```

## When to Use Each Mode

### Use Async When

- Testing async application behavior
- High concurrency requirements
- Matching production async code
- Need to test Tokio integration

### Use Blocking When

- Testing synchronous systems
- Baseline performance measurement
- Minimal overhead is critical
- Simpler debugging needed

### Use `--shm-direct` When

- Maximum shared memory performance
- Fixed message sizes acceptable
- Running on Unix/Linux

## Code Architecture

### Async Files

```
src/
├── benchmark.rs              # Async benchmark runner
├── ipc/
│   ├── unix_domain_socket.rs # Async UDS
│   ├── tcp_socket.rs         # Async TCP
│   ├── shared_memory.rs      # Async SHM
│   └── posix_message_queue.rs # Async PMQ
```

### Blocking Files

```
src/
├── benchmark_blocking.rs     # Blocking benchmark runner
├── ipc/
│   ├── unix_domain_socket_blocking.rs
│   ├── tcp_socket_blocking.rs
│   ├── shared_memory_blocking.rs
│   ├── shared_memory_direct.rs  # Direct mode (blocking only)
│   └── posix_message_queue_blocking.rs
```

## Common Questions

### Q: Does blocking mean slower?

No. For IPC benchmarks with low concurrency, blocking can actually have lower latency due to reduced runtime overhead.

### Q: Can I mix modes in one run?

No. Each run uses one mode. Run separate benchmarks to compare.

### Q: Why does `--shm-direct` require blocking?

The direct memory implementation uses synchronization primitives that work best with blocking I/O patterns.

## See Also

- [Shared Memory Options](shared-memory-options.md) - Ring buffer vs direct
- [Performance Tuning](../user-guide/performance-tuning.md) - Optimizing results
- [CLI Reference](../reference/cli-reference.md) - Mode flags
