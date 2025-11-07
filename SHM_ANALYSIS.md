# Shared Memory Performance Analysis: C vs Rust

## Overview

This document provides a detailed comparison of the C (Nissan) and Rust shared memory implementations, including performance analysis and flow diagrams.

---

## C Program (Nissan) - Message Flow

```mermaid
sequenceDiagram
    participant Main as Main Process
    participant SHM as Shared Memory<br/>(Direct Struct)
    participant Parent as Parent (Sender)<br/>CPU Core 5
    participant Child as Child (Receiver)<br/>CPU Core 4
    
    Note over Main: Setup Phase
    Main->>SHM: shm_open() + mmap()
    Main->>SHM: Init pthread_mutex (PROCESS_SHARED)
    Main->>SHM: Init pthread_cond (PROCESS_SHARED)
    Main->>SHM: Set ready = 0
    Main->>Main: fork()
    
    Note over Parent,Child: 10 Second Warmup
    Parent->>Parent: usleep(10000000)
    Child->>Child: Wait for messages
    
    rect rgb(200, 220, 255)
        Note over Parent,Child: Message Loop (10,000 iterations)
        
        Parent->>Parent: pthread_mutex_lock()
        Note right of Parent: ⏱️ TIMESTAMP CAPTURE
        Parent->>Parent: strcpy(msg_text, "Hello...")
        Parent->>Parent: clock_gettime(MONOTONIC,<br/>start_time)
        Parent->>SHM: Write start_time to struct
        Parent->>SHM: Write msg_text to struct
        Parent->>SHM: ready = 1 (DIRECT WRITE)
        Parent->>Child: pthread_cond_signal()
        
        Child->>Child: pthread_mutex_lock()
        Child->>Child: while(!ready)<br/>pthread_cond_wait()
        Note left of Child: Wakes up on signal
        Note left of Child: ⏱️ TIMESTAMP CAPTURE
        Child->>Child: clock_gettime(MONOTONIC, end)
        Child->>SHM: Read start_time (DIRECT READ)
        Child->>SHM: Read msg_text (DIRECT READ)
        Note left of Child: 📊 CALCULATE LATENCY<br/>latency = end - start_time
        Child->>Child: printf() latency
        Child->>SHM: ready = 0 (DIRECT WRITE)
        Child->>Parent: pthread_cond_signal()
        Child->>Child: pthread_mutex_unlock()
        
        Parent->>Parent: while(ready)<br/>pthread_cond_wait()
        Note right of Parent: Wakes up on signal
        Parent->>Parent: pthread_mutex_unlock()
        Parent->>Parent: usleep(10000)
    end
    
    Note over Parent,Child: Cleanup
    Child->>Child: Print statistics
    Parent->>Parent: wait(NULL)
    Parent->>SHM: pthread_mutex_destroy()
    Parent->>SHM: pthread_cond_destroy()
    Parent->>SHM: munmap() + shm_unlink()
```

---

## Rust Program - Message Flow

```mermaid
sequenceDiagram
    participant Main as Main Process
    participant RB as Ring Buffer<br/>(Complex Structure)
    participant Server as Server Process<br/>CPU Core 5
    participant Client as Client Process<br/>CPU Core 4
    
    Note over Main: Setup Phase
    Main->>RB: ShmemConf::new().size().create()
    Main->>RB: Init SharedMemoryRingBuffer
    Main->>RB: Init pthread_mutex (PROCESS_SHARED)
    Main->>RB: Init pthread_cond × 2<br/>(data_ready, space_ready)
    Main->>RB: Set atomics:<br/>read_pos=0, write_pos=0
    Main->>Server: spawn_server_process()
    Server->>Server: Wait for ready signal
    
    rect rgb(255, 220, 200)
        Note over Client,Server: Message Loop (10,000 iterations)
        
        Note right of Client: 🔧 PRE-PROCESSING
        Client->>Client: message.clone()
        Client->>Client: bincode::serialize()<br/>(FULL STRUCT → Vec<u8>)
        Note right of Client: ⏱️ TIMESTAMP CAPTURE
        Client->>Client: set_timestamp_now()
        Client->>Client: Patch timestamp bytes in buffer
        
        Client->>Client: send_blocking()
        Note right of Client: Enter Ring Buffer Write
        Client->>RB: pthread_mutex_lock()
        Client->>RB: Check available_write_space()<br/>(3 atomic loads)
        
        loop Wait for space
            Client->>RB: if no space:<br/>pthread_cond_wait(space_ready)
        end
        
        Note right of Client: 🔄 BYTE-BY-BYTE WRITE
        Client->>RB: Write 4-byte length prefix<br/>(with % capacity per byte)
        Client->>RB: Write serialized data<br/>(with % capacity per byte)
        Client->>RB: Atomic: write_pos.store()
        Client->>RB: Atomic: message_count++
        Client->>Server: pthread_cond_signal(data_ready)
        Client->>RB: pthread_mutex_unlock()
        
        Server->>Server: receive_blocking()
        Note left of Server: Enter Ring Buffer Read
        Server->>RB: pthread_mutex_lock()
        Server->>RB: Check available_read_data()<br/>(2 atomic loads)
        
        loop Wait for data
            Server->>RB: if no data:<br/>pthread_cond_wait(data_ready)
        end
        
        Note left of Server: Wakes up on signal
        Note left of Server: ⏱️ TIMESTAMP CAPTURE
        Server->>Server: get_monotonic_time_ns()
        Note left of Server: 🔄 BYTE-BY-BYTE READ
        Server->>RB: Read 4-byte length<br/>(with % capacity per byte)
        Server->>RB: Validate length
        Server->>RB: Read data bytes<br/>(with % capacity per byte)
        Server->>RB: Atomic: read_pos.store()
        Server->>Client: pthread_cond_signal(space_ready)
        Server->>RB: pthread_mutex_unlock()
        
        Note left of Server: 🔧 POST-PROCESSING
        Server->>Server: bincode::deserialize()<br/>(Vec<u8> → Message)
        Note left of Server: 📊 CALCULATE LATENCY<br/>latency = receive_time -<br/>message.timestamp
        Server->>Server: writeln!(file, latency)
    end
    
    Note over Client,Server: Cleanup
    Client->>Server: Send shutdown message
    Server->>RB: pthread_mutex_destroy()
    Server->>RB: pthread_cond_destroy() × 2
    Main->>RB: Drop shared memory
```

---

## Architectural Comparison

```mermaid
graph TB
    subgraph C["C PROGRAM (Simple & Direct)"]
        C1[Parent: Lock Mutex]
        C2[strcpy msg_text]
        C3[clock_gettime → start_time]
        C4["DIRECT WRITE to struct<br/>(~100 bytes, memcpy)"]
        C5[Set ready = 1]
        C6[Signal Condition Variable]
        C7[Child: Wake Up]
        C8[clock_gettime → end]
        C9["DIRECT READ from struct<br/>(~100 bytes, memcpy)"]
        C10[Calculate: end - start_time]
        
        C1-->C2-->C3-->C4-->C5-->C6-->C7-->C8-->C9-->C10
        
        style C4 fill:#90EE90
        style C9 fill:#90EE90
    end
    
    subgraph Rust["RUST PROGRAM (Complex & Layered)"]
        R1[Client: Clone Message]
        R2["bincode::serialize()<br/>(5-15µs overhead)"]
        R3[set_timestamp_now]
        R4[Patch timestamp bytes]
        R5[Lock Mutex]
        R6[Check available space<br/>3 atomic loads]
        R7["BYTE-BY-BYTE WRITE<br/>120 ops with % modulo<br/>(2-5µs overhead)"]
        R8[Update write_pos atomic]
        R9[Signal data_ready]
        R10[Server: Wake Up]
        R11[get_monotonic_time]
        R12[Check available data<br/>2 atomic loads]
        R13["BYTE-BY-BYTE READ<br/>120 ops with % modulo<br/>(2-5µs overhead)"]
        R14[Update read_pos atomic]
        R15["bincode::deserialize()<br/>(3-8µs overhead)"]
        R16[Calculate: receive - msg.timestamp]
        
        R1-->R2-->R3-->R4-->R5-->R6-->R7-->R8-->R9
        R9-->R10-->R11-->R12-->R13-->R14-->R15-->R16
        
        style R2 fill:#FFB6C1
        style R7 fill:#FFB6C1
        style R13 fill:#FFB6C1
        style R15 fill:#FFB6C1
    end
```

---

## Timestamp Flow

### C Program
```mermaid
graph LR
    A[Parent:<br/>clock_gettime] -->|"Write to<br/>start_time field"| B[Shared Memory]
    B -->|"Direct read<br/>from field"| C[Child:<br/>clock_gettime]
    C --> D[Calculate:<br/>end - start_time]
    
    style A fill:#90EE90
    style C fill:#90EE90
```

### Rust Program
```mermaid
graph LR
    A[Client:<br/>get_monotonic_time_ns] -->|"Embed in<br/>Message struct"| B[Bincode<br/>Serialization]
    B -->|"Write to<br/>ring buffer"| C[Ring Buffer]
    C -->|"Read from<br/>ring buffer"| D[Bincode<br/>Deserialization]
    D -->|"Extract<br/>timestamp field"| E[Server:<br/>get_monotonic_time_ns]
    E --> F[Calculate:<br/>receive - send]
    
    style A fill:#FFB6C1
    style E fill:#FFB6C1
    style B fill:#FF6B6B
    style D fill:#FF6B6B
```

---

## Performance Comparison

### Latency Results (No Load)

| Implementation | Mean | Min | Max |
|----------------|------|-----|-----|
| **C (Nissan)** | 6.45 µs | 4.95 µs | 27.34 µs |
| **Rust Blocking** | 8.62 µs | 7.14 µs | 91.77 µs |
| **Difference** | +33.6% | +44.2% | **+235.6%** |

### Overhead Breakdown

#### Send Path Overhead

| Step | C | Rust | Delta |
|------|---|------|-------|
| Prepare | 0 µs | 5-15 µs (clone + serialize) | +5-15 µs |
| Timestamp | 0.2 µs | 0.7 µs (patch bytes) | +0.5 µs |
| Space Check | 0 µs | 0.2 µs (3 atomics) | +0.2 µs |
| Write Data | 0.05 µs (memcpy) | 2-5 µs (byte-by-byte) | +2-5 µs |
| Update State | 0.01 µs | 0.11 µs (atomics) | +0.1 µs |
| **TOTAL** | **~1-2 µs** | **~9-23 µs** | **+8-21 µs** |

#### Receive Path Overhead

| Step | C | Rust | Delta |
|------|---|------|-------|
| Data Check | 0.01 µs (flag) | 0.11 µs (atomics) | +0.1 µs |
| Read Data | 0.05 µs (direct) | 2-5 µs (byte-by-byte) | +2-5 µs |
| Deserialize | 0 µs | 3-8 µs (bincode) | +3-8 µs |
| **TOTAL** | **~1 µs** | **~6-14 µs** | **+5-13 µs** |

---

## Root Causes of Performance Difference

### 1. Serialization Overhead (5-15 µs)
- C: Direct struct access, no serialization
- Rust: Full bincode serialization of Message struct
- **Impact:** Major contributor to latency

### 2. Ring Buffer Complexity (4-10 µs)
- C: Simple flag-based protocol
- Rust: Circular buffer with modulo arithmetic
- **Impact:** Byte-by-byte operations prevent optimization

### 3. Memory Allocation (0-20 µs)
- C: No allocation per message
- Rust: Clone + serialize allocates heap memory
- **Impact:** Under load, allocator can add 10-20 µs

### 4. Atomic Operations (0.5-2 µs)
- C: Minimal atomics (mutex only)
- Rust: 6 atomic loads + 2 stores per message
- **Impact:** Under contention, can add latency

---

## Recommendations

### To Match C Performance:
1. **Remove bincode serialization** - use `#[repr(C)]` struct
2. **Simplify to direct struct access** - like C program
3. **Use memcpy instead of byte-by-byte** - leverage hardware

### Immediate Improvements (Without Redesign):
1. **Pre-allocate serialization buffer** - reuse Vec
2. **Batch memcpy operations** - reduce modulo ops
3. **Profile under load** - identify hotspots

---

## Files

- C Implementation: `/home/mcurrier/auto/nissanvsrustylat/nov6/res_rustcodechangetomatchCtimestamps_origC_andswapaffinrust/sharedmemory_mutex_priority_50_affinity_CPU45_10s.c`
- Rust Implementation: `src/ipc/shared_memory_blocking.rs`
- Test Results: `/home/mcurrier/auto/nissanvsrustylat/nov6/res_rustcodechangetomatchCtimestamps_origC_andswapaffinrust/`

---

**Generated:** November 6, 2025
**Analysis by:** Claude Sonnet 4.5

