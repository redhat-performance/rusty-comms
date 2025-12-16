# @rusty-comms/AGENTS.md

# Project-Specific AI Agent Guidelines

**Project:** IPC Benchmark Suite (rusty-comms)  
**Purpose:** Comprehensive IPC performance measurement tool  
**For general guidelines:** See `~/AGENTS.md`

---

## Project Overview

This is a Rust-based IPC benchmark suite that measures latency and throughput across multiple
interprocess communication mechanisms. It supports both async (Tokio) and blocking (std) execution
modes for fair comparison.

**Key Constraints:**
- **MSRV**: Rust 1.70 — do not use features or dependencies requiring newer Rust versions
- **Primary Target**: Linux (RHEL 9.6) — Unix-specific features are acceptable
- **Performance-Critical**: Minimize overhead in measurement paths

---

## Architecture Patterns

### Dual Execution Modes

Every IPC mechanism has paired implementations:
- `<mechanism>.rs` — async implementation using Tokio
- `<mechanism>_blocking.rs` — blocking implementation using std

```
src/ipc/
├── mod.rs                          # IpcTransport trait definition
├── unix_domain_socket.rs           # Async UDS
├── unix_domain_socket_blocking.rs  # Blocking UDS
├── tcp_socket.rs                   # Async TCP
├── tcp_socket_blocking.rs          # Blocking TCP
├── shared_memory.rs                # Async shared memory (ring buffer)
├── shared_memory_blocking.rs       # Blocking shared memory
├── shared_memory_direct.rs         # High-performance direct memory access
├── posix_message_queue.rs          # Async PMQ
└── posix_message_queue_blocking.rs # Blocking PMQ
```

### Trait-Based Design

All transports implement traits from `src/ipc/mod.rs`. When adding or modifying transports:
- Async transports implement `IpcTransport` (async trait)
- Blocking transports implement `BlockingIpcTransport`

---

## Coding Conventions

### Timing and Measurement

- **Always use monotonic clocks** via `get_monotonic_time_ns()` for latency measurement
- Capture timestamps immediately before/after IPC operations, not during message construction
- Never use `SystemTime` or wall-clock time for performance measurement

### Backpressure Detection

All `send` methods must detect and report when buffers are full:
- Return `Ok(true)` when backpressure is detected
- Log a warning on first occurrence per transport instance
- Do not fail silently when writes block

### Resource Cleanup

Implement proper cleanup for all system resources:
- `Drop` trait for automatic cleanup
- Explicit `close()` method for controlled shutdown
- Clean up: sockets, shared memory segments (`shm_unlink`), message queues (`mq_unlink`)

### Error Handling

- Use `anyhow::Result` for fallible operations
- Use `.with_context()` for meaningful error messages
- Use `thiserror` for custom error types when needed

### Logging

- Use `tracing` macros (`tracing::info!`, `tracing::warn!`, etc.)
- Never use `println!` or `eprintln!` for diagnostics
- Log level guidelines:
  - `error!`: Unrecoverable failures
  - `warn!`: Backpressure, timeouts, recoverable issues
  - `info!`: Test start/completion, configuration summary
  - `debug!`: Per-message details (disabled by default)
  - `trace!`: Internal state transitions

---

## Testing Guidelines

### Test File Organization

- Unit tests: inline `#[cfg(test)]` modules
- Integration tests: `tests/integration_blocking_*.rs` for blocking mode
- Each mechanism needs one-way and round-trip coverage

### Test Patterns

```rust
#[test]
fn test_mechanism_one_way() {
    // Server setup, client connection, message send/receive
}

#[test]
fn test_mechanism_round_trip() {
    // Full request-response cycle
}
```

### Avoiding Flakiness

- Do not rely on fixed sleeps; use synchronization primitives
- If a sleep is unavoidable, add a comment explaining why
- Use generous timeouts for CI environments (macOS has higher scheduler jitter)

---

## Performance Guidelines

### Hot Path Rules

- No allocations in measurement loops
- No logging in tight loops (use counters, log summary)
- Prefer stack allocation over heap when message size is bounded
- Use `bincode` for standard serialization; direct `memcpy` for `--shm-direct`

### Benchmarking Best Practices

- Always use `--warmup-iterations` to stabilize measurements
- Pin processes with `--server-affinity` and `--client-affinity` for reproducibility
- Run on idle systems for accurate results

---

## Dependency Management

### MSRV Compatibility

Many dependencies are pinned in `Cargo.toml` to maintain Rust 1.70 compatibility:
- `tokio-macros = "=2.5.0"` (2.6.0+ requires Rust 1.71+)
- `mio = "=1.0.4"` (1.1.0+ requires Rust 1.71+)
- See `Cargo.toml` comments for full list

**Before adding new dependencies:**
1. Check MSRV compatibility
2. Prefer well-maintained, minimal dependencies
3. Import only needed features

---

## Common Patterns

### Server Spawning

The benchmark spawns server processes via the same binary in server-only mode:
- Parent sends readiness signal via pipe (`0x01` byte)
- Child waits for this signal before starting tests
- Resources are cleaned up on child termination

### CPU Affinity

Use `core_affinity` crate for pinning:
- `--server-affinity N` pins receiver to core N
- `--client-affinity M` pins sender to core M

---

**End of Project-Specific Guidelines**
