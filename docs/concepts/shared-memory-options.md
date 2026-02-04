# Shared Memory Options

This document explains the two shared memory implementations available in rusty-comms.

## Overview

| Implementation | Flag | Latency | Platform | Message Size |
|----------------|------|---------|----------|--------------|
| Ring Buffer | (default) | ~20 µs | All | Variable |
| Direct Memory | `--shm-direct` | ~7 µs | Unix only | Fixed (8KB) |

## Ring Buffer (Default)

### How It Works

The ring buffer implementation uses:
- POSIX shared memory segment
- Circular buffer for messages
- bincode serialization
- Mutex-protected access

```
┌─────────────────────────────────────────┐
│           Shared Memory Segment          │
├─────────────────────────────────────────┤
│ Header (write_pos, read_pos, locks)      │
├─────────────────────────────────────────┤
│ ┌─────┬─────┬─────┬─────┬─────┬─────┐  │
│ │ Msg │ Msg │ Msg │ ... │ Msg │ Msg │  │
│ │  1  │  2  │  3  │     │ N-1 │  N  │  │
│ └─────┴─────┴─────┴─────┴─────┴─────┘  │
│         Ring Buffer (circular)           │
└─────────────────────────────────────────┘
```

### Usage

```bash
# Ring buffer with async (default)
ipc-benchmark -m shm -i 10000

# Ring buffer with blocking
ipc-benchmark -m shm -i 10000 --blocking
```

### Characteristics

| Aspect | Value |
|--------|-------|
| Average Latency | ~20 µs |
| Maximum Latency | ~10 ms (worst case) |
| Serialization | bincode |
| Message Size | Variable |
| Platform | All (Linux, macOS, Windows) |

### Advantages

- **Cross-platform**: Works everywhere
- **Variable messages**: Any size up to buffer limit
- **Async compatible**: Works with Tokio
- **Safe**: Rust memory safety guarantees

### Disadvantages

- **Serialization overhead**: bincode adds ~15-30 µs
- **Higher tail latency**: Occasional mutex contention
- **More complexity**: Ring buffer management

## Direct Memory (`--shm-direct`)

### How It Works

The direct memory implementation uses:
- POSIX shared memory segment
- Direct memory copy (no serialization)
- Atomic operations for synchronization
- Fixed-size message slots

```
┌─────────────────────────────────────────┐
│           Shared Memory Segment          │
├─────────────────────────────────────────┤
│ ┌─────────────────────────────────────┐ │
│ │         Atomic Flags                 │ │
│ │   (ready, done, sequence)            │ │
│ ├─────────────────────────────────────┤ │
│ │         Message Slot                 │ │
│ │   id: u64                            │ │
│ │   timestamp: u64                     │ │
│ │   payload: [u8; 8192]                │ │
│ └─────────────────────────────────────┘ │
└─────────────────────────────────────────┘
```

### Usage

```bash
# Direct memory (auto-enables blocking)
ipc-benchmark -m shm --shm-direct -i 10000

# With CPU affinity for best results
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 0 --client-affinity 1
```

### Characteristics

| Aspect | Value |
|--------|-------|
| Average Latency | ~7 µs |
| Maximum Latency | ~22 µs |
| Serialization | None (memcpy) |
| Message Size | Fixed (8KB max payload) |
| Platform | Unix only |

### Advantages

- **3x faster average**: ~7 µs vs ~20 µs
- **450x better tail**: ~22 µs vs ~10 ms
- **No serialization**: Direct memory copy
- **Predictable**: Consistent performance

### Disadvantages

- **Unix only**: No Windows support
- **Fixed size**: 8KB maximum payload
- **Blocking only**: No async support
- **Less flexible**: Single producer/consumer

## Comparison

### Performance

| Metric | Ring Buffer | Direct Memory | Improvement |
|--------|-------------|---------------|-------------|
| Mean Latency | 20 µs | 7 µs | 3x faster |
| Max Latency | 10 ms | 22 µs | 450x better |
| P99 Latency | 50 µs | 15 µs | 3x better |

### Use Cases

| Scenario | Recommendation |
|----------|----------------|
| Maximum performance | Direct Memory |
| Cross-platform | Ring Buffer |
| Variable messages | Ring Buffer |
| Async required | Ring Buffer |
| Tail latency critical | Direct Memory |
| Windows support | Ring Buffer |

## Technical Details

### Ring Buffer Synchronization

```rust
// Simplified ring buffer write
fn write(&mut self, msg: &Message) -> Result<()> {
    let serialized = bincode::serialize(msg)?;
    
    // Lock the buffer
    let mut guard = self.lock()?;
    
    // Write to current position
    guard.write_at(self.write_pos, &serialized)?;
    
    // Advance write position
    self.write_pos = (self.write_pos + 1) % self.capacity;
    
    Ok(())
}
```

### Direct Memory Synchronization

```rust
// Simplified direct memory write
fn write(&mut self, msg: &DirectMessage) -> Result<()> {
    // Wait for slot to be free
    while self.slot.flag.load(Ordering::Acquire) != READY {
        std::hint::spin_loop();
    }
    
    // Direct memory copy (no serialization)
    unsafe {
        std::ptr::copy_nonoverlapping(
            msg as *const _ as *const u8,
            self.slot.data.as_mut_ptr(),
            std::mem::size_of::<DirectMessage>()
        );
    }
    
    // Signal data is ready
    self.slot.flag.store(WRITTEN, Ordering::Release);
    
    Ok(())
}
```

### Memory Layout

**Ring Buffer Message:**
```rust
struct Message {
    id: u64,
    timestamp: u64,
    msg_type: MessageType,
    payload: Vec<u8>,  // Variable size
}
// Serialized with bincode, variable length
```

**Direct Message:**
```rust
#[repr(C)]
struct DirectMessage {
    id: u64,
    timestamp: u64,
    payload_len: u32,
    payload: [u8; 8192],  // Fixed size
}
// Fixed layout, 8216 bytes total
```

## Choosing the Right Implementation

### Decision Tree

```
Need Windows support?
  Yes → Ring Buffer
  No ↓

Need variable message sizes?
  Yes → Ring Buffer
  No ↓

Need async/Tokio?
  Yes → Ring Buffer
  No ↓

Maximum performance priority?
  Yes → Direct Memory
  No → Either works
```

### Example Commands

```bash
# Flexibility priority
ipc-benchmark -m shm -i 10000

# Performance priority
ipc-benchmark -m shm --shm-direct -i 10000 \
  --server-affinity 0 --client-affinity 1
```

## See Also

- [SHM_COMPARISON.md](../../SHM_COMPARISON.md) - Detailed technical comparison
- [IPC Mechanisms](../user-guide/ipc-mechanisms.md) - All mechanism details
- [Performance Tuning](../user-guide/performance-tuning.md) - Optimization guide
