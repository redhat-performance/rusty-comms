# Latency Measurement Methodology

This document explains how rusty-comms measures latency and why the methodology matters.

## Overview

Accurate latency measurement requires:
- High-precision timing source
- Correct timestamp placement
- Understanding what is being measured

## Clock Source

### Monotonic Clocks

Rusty-comms uses **monotonic clocks** for all latency measurements.

```rust
// Uses CLOCK_MONOTONIC on Unix
let timestamp = get_monotonic_time_ns();
```

**Why monotonic?**
- Immune to system clock adjustments (NTP, manual changes)
- Measures time since system boot
- Consistent across long-running benchmarks

### Platform Support

| Platform | Clock Source |
|----------|--------------|
| Linux | `CLOCK_MONOTONIC` via nix crate |
| macOS | `CLOCK_MONOTONIC` via nix crate |
| Windows | System time (less precise) |

## Timestamp Capture Points

### One-Way Latency

```
Sender                              Receiver
  │                                    │
  ├─ [T1] Capture timestamp            │
  ├─ Serialize message                 │
  ├─ Send to transport                 │
  │     ════════════════════>          │
  │                          Receive   ┤
  │                          Deserialize
  │                    [T2] Capture timestamp
  │                                    │
  
  Latency = T2 - T1
```

**What's included:**
- ✅ Serialization time
- ✅ Transport time (syscalls, kernel)
- ✅ Deserialization time

**What's excluded:**
- ❌ Message construction before T1
- ❌ Processing after T2

### Round-Trip Latency

```
Client                              Server
  │                                    │
  ├─ [T1] Capture timestamp            │
  ├─ Send request                      │
  │     ════════════════════>          │
  │                          Receive   ┤
  │                          Process   ┤
  │                          Send response
  │     <════════════════════          │
  ├─ Receive response                  │
  ├─ [T2] Capture timestamp            │
  │                                    │
  
  Latency = T2 - T1
```

**Includes:**
- Request transmission
- Server processing (minimal)
- Response transmission

## Implementation Details

### Timestamp in Message

The timestamp is embedded in the message:

```rust
pub struct Message {
    pub id: u64,
    pub timestamp: u64,  // Monotonic timestamp used for latency measurement
    pub payload: Vec<u8>,
    // ...
}
```

### Capture Point

Timestamp is captured **immediately before serialization**:

```rust
// Create message
let mut message = message.clone();
message.set_timestamp_now();  // Capture here

// Serialize and send
let serialized = bincode::serialize(&message)?;
stream.write_all(&serialized)?;
```

### Latency Calculation

On the receive side:

```rust
let receive_time_ns = get_monotonic_time_ns();
let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
```

## Streaming Output Timestamps vs Latency Timing

The per-message streaming outputs (`--streaming-output-json` / `--streaming-output-csv`)
include a `timestamp_ns` column. This timestamp is **Unix epoch time** (nanoseconds
since epoch) for when the record was captured so you can correlate results with
external monitoring.

The **latency values themselves** are derived from **monotonic timing** and remain
immune to NTP/manual clock adjustments.

## Why This Matters

### Common Mistakes

**Mistake 1: Capturing too early**
```rust
// BAD: Includes message construction time
let timestamp = get_monotonic_time_ns();
let message = Message::new(id, large_payload.clone(), msg_type);
// ... more setup ...
send(message);
```

**Mistake 2: Using wall-clock time**
```rust
// BAD: Affected by NTP adjustments
let timestamp = SystemTime::now();
```

**Mistake 3: Measuring wrong scope**
```rust
// BAD: Includes unrelated work
let start = Instant::now();
prepare_data();
create_message();
send_message();
process_result();
let elapsed = start.elapsed();  // What are we measuring?
```

### Correct Approach

```rust
// GOOD: Timestamp right before IPC operation
let mut message = create_message();
message.timestamp = get_monotonic_time_ns();  // Here
send_message(&message)?;
```

## Statistical Considerations

### First Message Effect

The first message typically has higher latency due to:
- CPU cache misses
- Memory allocation
- Branch prediction warm-up

**Solution:** By default, the first message is discarded. Use `--include-first-message` to include it.

### Warmup

Warmup iterations stabilize:
- JIT compilation (if applicable)
- OS buffer allocation
- CPU frequency scaling

```bash
ipc-benchmark -m uds -i 10000 -w 1000
```

### Percentiles vs Averages

- **Mean**: Affected by outliers
- **Median (P50)**: Typical experience
- **P99**: Worst case for most users
- **P99.9**: Tail latency

```bash
ipc-benchmark --percentiles 50 95 99 99.9 99.99
```

## Comparing with Other Tools

### ping

`ping` measures ICMP round-trip, including:
- Network stack overhead
- ICMP processing

Rusty-comms TCP latency should be lower than `ping` to localhost.

### C benchmarks

For valid comparisons:
- Both must use monotonic clocks
- Capture points must be equivalent
- Serialization overhead must be considered

## Validation

### Sanity Checks

Expected latency ranges on modern hardware:

| Mechanism | Expected Range |
|-----------|----------------|
| Shared Memory (direct) | 1-10 µs |
| Unix Domain Sockets | 2-15 µs |
| POSIX Message Queues | 5-20 µs |
| TCP (localhost) | 5-30 µs |

### If Results Seem Wrong

1. Check for system interference
2. Verify CPU frequency is stable
3. Ensure sufficient warmup
4. Check for backpressure

## See Also

- [Performance Tuning](../user-guide/performance-tuning.md) - System configuration
- [Async vs Blocking](async-vs-blocking.md) - Execution mode impact
- [Troubleshooting](../user-guide/troubleshooting.md) - Diagnosing issues
