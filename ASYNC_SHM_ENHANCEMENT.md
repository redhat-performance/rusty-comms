# Async Shared Memory Enhancement Plan

## Problem
Current async SHM implementation uses `sleep(Duration::from_micros(10))` polling, causing:
- High latency (~520µs vs ~10µs for blocking)
- Low throughput
- CPU inefficiency

## Root Cause
- Cross-process async notification not supported
- `tokio::sync::Notify` doesn't work across processes
- Can't use `pthread_cond_wait` in async (blocks runtime)

## Solution: Hybrid Signaling

### Architecture
```
┌──────────────┐                    ┌──────────────┐
│   Client     │                    │   Server     │
│   Process    │                    │   Process    │
├──────────────┤                    ├──────────────┤
│ Shared Mem   │◄──── Data ────────►│ Shared Mem   │
│ Ring Buffer  │                    │ Ring Buffer  │
├──────────────┤                    ├──────────────┤
│ UDS Socket A │◄──── Signal ──────►│ UDS Socket B │
│ (AsyncFd)    │     (1 byte)       │ (AsyncFd)    │
└──────────────┘                    └──────────────┘
```

### Implementation Steps

1. **Add socketpair to SharedMemoryConnection**
   - Create Unix domain socketpair during connection setup
   - Store in `Arc<Mutex<UnixStream>>` (tokio)
   - One socket per connection

2. **Modify send_message()**
   ```rust
   async fn send_message(&self, message: &Message) -> Result<bool> {
       // Write data to shared memory ring buffer
       ring_buffer.write_data(&message_bytes)?;
       
       // Send 1-byte wake-up signal via socket
       self.signal_socket.write_u8(1).await?;
       
       Ok(false)
   }
   ```

3. **Modify receive_message()**
   ```rust
   async fn receive_message(&self) -> Result<Message> {
       loop {
           // Try to read from ring buffer (non-blocking)
           if let Ok(data) = ring_buffer.try_read_data() {
               return Message::from_bytes(&data);
           }
           
           // Wait for signal on socket (async, no polling!)
           self.signal_socket.readable().await?;
           let _ = self.signal_socket.try_read(&mut [0u8; 1])?;
       }
   }
   ```

4. **Socket Setup**
   - Create during `establish_connection()`
   - Pass fd numbers through environment or args
   - Clean up on close

### Expected Improvement
- Latency: 520µs → ~10-30µs (similar to UDS)
- Throughput: 200 msg/s → ~30,000 msg/s
- CPU: Reduced (no busy polling)

### Limitations
- Adds one socketpair per connection
- Small overhead for wake-up signals
- Still not as fast as blocking (syscall overhead)

### Alternative: Eventfd
Could use eventfd instead of socketpair:
- Lighter weight (just counter, no data)
- Linux-only
- Slightly lower overhead

## Implementation Priority
1. ✅ Quick fix (PMQ - DONE)
2. 🚧 Medium fix (SHM hybrid signaling - THIS)
3. 🔮 Advanced (eventfd optimization)

