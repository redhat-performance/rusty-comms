//! Blocking shared memory transport implementation.
//!
//! This module provides a blocking implementation of the shared memory
//! transport using a ring buffer in shared memory segments.
//!
//! # Platform Support
//!
//! This implementation works on all platforms (Linux, macOS, Windows, BSD).
//!
//! # Blocking Behavior
//!
//! All operations in this module block the calling thread:
//! - `create()` blocks during shared memory segment creation
//! - `open()` blocks until segment is available
//! - `send()` blocks until space is available in ring buffer (using pthread
//!   condition variables on Unix)
//! - `recv()` blocks until data is available (using pthread condition variables
//!   on Unix)
//!
//! # Ring Buffer Protocol
//!
//! Uses a circular ring buffer in shared memory with atomic operations for
//! coordination. Messages are length-prefixed (4-byte u32, little-endian) +
//! bincode serialized data.
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::ipc::{
//!     BlockingSharedMemory, BlockingTransport, TransportConfig
//! };
//!
//! # fn example() -> anyhow::Result<()> {
//! let mut server = BlockingSharedMemory::new();
//! let mut config = TransportConfig::default();
//! config.shared_memory_name = "test_shm".to_string();
//! config.buffer_size = 8192;
//!
//! // Server: create shared memory
//! server.start_server_blocking(&config)?;
//!
//! // In another thread/process: client opens shared memory
//! // let mut client = BlockingSharedMemory::new();
//! // client.start_client_blocking(&config)?;
//! # Ok(())
//! # }
//! ```

use crate::ipc::{BlockingTransport, Message, TransportConfig};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use shared_memory::{Shmem, ShmemConf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, trace};

// For shared memory cleanup on Linux
#[cfg(target_os = "linux")]
use nix::libc;

#[cfg(unix)]
use libc::{pthread_cond_t, pthread_mutex_t};

/// Shared memory ring buffer structure.
///
/// This structure is placed at the start of the shared memory segment and
/// manages the circular buffer for message passing.
///
/// Uses process-shared pthread mutex and condition variables for efficient
/// synchronization.
#[repr(C)]
struct SharedMemoryRingBuffer {
    // Ring buffer metadata
    capacity: AtomicUsize,
    read_pos: AtomicUsize,
    write_pos: AtomicUsize,

    // Synchronization flags
    server_ready: AtomicBool,
    client_ready: AtomicBool,
    shutdown: AtomicBool,

    // Message count for coordination
    message_count: AtomicUsize,

    // Process-shared synchronization primitives
    #[cfg(unix)]
    mutex: pthread_mutex_t,
    #[cfg(unix)]
    data_ready: pthread_cond_t, // Signals when data is available to read
    #[cfg(unix)]
    space_ready: pthread_cond_t, // Signals when space is available to write

                                 // Data follows after this header
}

impl SharedMemoryRingBuffer {
    const HEADER_SIZE: usize = std::mem::size_of::<Self>();

    /// Create a new ring buffer header with process-shared synchronization
    fn new(capacity: usize) -> Self {
        #[cfg(unix)]
        unsafe {
            use std::mem::MaybeUninit;

            // Initialize mutex with PTHREAD_PROCESS_SHARED attribute
            let mut mutex_attr = MaybeUninit::uninit();
            libc::pthread_mutexattr_init(mutex_attr.as_mut_ptr());
            libc::pthread_mutexattr_setpshared(
                mutex_attr.as_mut_ptr(),
                libc::PTHREAD_PROCESS_SHARED,
            );

            let mut mutex = MaybeUninit::uninit();
            libc::pthread_mutex_init(mutex.as_mut_ptr(), mutex_attr.as_ptr());
            libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());

            // Initialize condition variables with PTHREAD_PROCESS_SHARED attribute
            let mut cond_attr = MaybeUninit::uninit();
            libc::pthread_condattr_init(cond_attr.as_mut_ptr());
            libc::pthread_condattr_setpshared(cond_attr.as_mut_ptr(), libc::PTHREAD_PROCESS_SHARED);

            let mut data_ready = MaybeUninit::uninit();
            libc::pthread_cond_init(data_ready.as_mut_ptr(), cond_attr.as_ptr());

            let mut space_ready = MaybeUninit::uninit();
            libc::pthread_cond_init(space_ready.as_mut_ptr(), cond_attr.as_ptr());

            libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());

            Self {
                capacity: AtomicUsize::new(capacity),
                read_pos: AtomicUsize::new(0),
                write_pos: AtomicUsize::new(0),
                server_ready: AtomicBool::new(false),
                client_ready: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                message_count: AtomicUsize::new(0),
                mutex: mutex.assume_init(),
                data_ready: data_ready.assume_init(),
                space_ready: space_ready.assume_init(),
            }
        }

        #[cfg(not(unix))]
        {
            Self {
                capacity: AtomicUsize::new(capacity),
                read_pos: AtomicUsize::new(0),
                write_pos: AtomicUsize::new(0),
                server_ready: AtomicBool::new(false),
                client_ready: AtomicBool::new(false),
                shutdown: AtomicBool::new(false),
                message_count: AtomicUsize::new(0),
            }
        }
    }

    /// Get pointer to the data area (after the header)
    fn data_ptr(&self) -> *mut u8 {
        unsafe { (self as *const Self as *mut u8).add(Self::HEADER_SIZE) }
    }

    /// Calculate available space for writing
    fn available_write_space(&self) -> usize {
        let capacity = self.capacity.load(Ordering::Acquire);
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            capacity - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        }
    }

    /// Calculate available data for reading
    fn available_read_data(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.capacity.load(Ordering::Acquire) - (read_pos - write_pos)
        }
    }

    /// Write data to the ring buffer (non-blocking, returns error if no space)
    ///
    /// Note: This method is currently unused as we've switched to the direct
    /// memory implementation, but kept for potential future use or testing.
    #[allow(dead_code)]
    fn write_data(&self, data: &[u8]) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        if self.available_write_space() < required_space {
            return Err(anyhow!("Not enough space in ring buffer"));
        }

        let capacity = self.capacity.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Write length prefix (little-endian)
        let len_bytes = (data_len as u32).to_le_bytes();
        for (i, &byte) in len_bytes.iter().enumerate() {
            unsafe {
                *data_ptr.add((write_pos + i) % capacity) = byte;
            }
        }

        // Write data
        for (i, &byte) in data.iter().enumerate() {
            unsafe {
                *data_ptr.add((write_pos + 4 + i) % capacity) = byte;
            }
        }

        self.write_pos
            .store((write_pos + required_space) % capacity, Ordering::Release);
        self.message_count.fetch_add(1, Ordering::Release);

        // On non-Unix there are no condvars; readers poll with
        // yield + sleep, so no signal is needed here.

        Ok(())
    }

    /// Read data from the ring buffer (non-blocking, returns error if no data)
    ///
    /// Note: This method is currently unused as we've switched to the direct
    /// memory implementation, but kept for potential future use or testing.
    #[allow(dead_code)]
    fn read_data(&self) -> Result<Vec<u8>> {
        if self.available_read_data() < 4 {
            return Err(anyhow!("No data available"));
        }

        let capacity = self.capacity.load(Ordering::Acquire);
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Read length prefix
        let mut len_bytes = [0u8; 4];
        for (i, byte) in len_bytes.iter_mut().enumerate() {
            unsafe {
                *byte = *data_ptr.add((read_pos + i) % capacity);
            }
        }
        let data_len = u32::from_le_bytes(len_bytes) as usize;

        // Basic validation to prevent reading garbage
        if data_len == 0 || data_len > capacity {
            return Err(anyhow!("Invalid message length: {}", data_len));
        }

        if self.available_read_data() < data_len + 4 {
            return Err(anyhow!("Incomplete message"));
        }

        // Read data
        let mut data = vec![0u8; data_len];
        for (i, byte) in data.iter_mut().enumerate() {
            unsafe {
                *byte = *data_ptr.add((read_pos + 4 + i) % capacity);
            }
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        Ok(data)
    }

    /// Write data to the ring buffer (blocking with condition
    /// variable).
    ///
    /// Uses `pthread_cond_wait` to block until space is available,
    /// then writes the data and signals any waiting readers.
    ///
    /// # Arguments
    ///
    /// * `data` - Mutable serialized message bytes. The timestamp
    ///   region will be updated in-place right before the write.
    /// * `timestamp_offset` - Byte range of the timestamp field
    ///   within `data`. When `Some`, the timestamp is refreshed
    ///   immediately before the memory write so that measured
    ///   latency excludes any backpressure wait time.
    ///
    /// # Safety
    /// Only available on Unix platforms with pthread support.
    #[cfg(unix)]
    unsafe fn write_data_blocking(
        &self,
        data: &mut [u8],
        timestamp_offset: Option<std::ops::Range<usize>>,
    ) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        // Lock mutex
        libc::pthread_mutex_lock(&self.mutex as *const _ as *mut _);

        // Wait for space to become available
        while self.available_write_space() < required_space {
            if self.shutdown.load(Ordering::Acquire) {
                libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                return Err(anyhow!("Connection closed"));
            }

            // Wait on condition variable (releases mutex,
            // reacquires on wake)
            libc::pthread_cond_wait(
                &self.space_ready as *const _ as *mut _,
                &self.mutex as *const _ as *mut _,
            );
        }

        // Update timestamp right before writing to shared memory
        // so that measured latency excludes backpressure wait time
        if let Some(ref ts_range) = timestamp_offset {
            let ts_now = crate::ipc::get_monotonic_time_ns();
            let ts_bytes = ts_now.to_le_bytes();
            data[ts_range.clone()].copy_from_slice(&ts_bytes);
        }

        // Space is available, write the data
        let capacity = self.capacity.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Write length prefix (little-endian)
        let len_bytes = (data_len as u32).to_le_bytes();
        for (i, &byte) in len_bytes.iter().enumerate() {
            *data_ptr.add((write_pos + i) % capacity) = byte;
        }

        // Write data
        for (i, &byte) in data.iter().enumerate() {
            *data_ptr.add((write_pos + 4 + i) % capacity) = byte;
        }

        self.write_pos
            .store((write_pos + required_space) % capacity, Ordering::Release);
        self.message_count.fetch_add(1, Ordering::Release);

        // Signal reader that data is available
        libc::pthread_cond_signal(&self.data_ready as *const _ as *mut _);

        // Unlock mutex
        libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);

        Ok(())
    }

    /// Read data from the ring buffer (blocking with condition
    /// variable).
    ///
    /// Uses `pthread_cond_wait` to block until data is available,
    /// then reads it and signals any waiting writers that space
    /// has been freed.
    ///
    /// # Safety
    /// Only available on Unix platforms with pthread support.
    #[cfg(unix)]
    unsafe fn read_data_blocking(&self) -> Result<Vec<u8>> {
        // Lock mutex
        libc::pthread_mutex_lock(&self.mutex as *const _ as *mut _);

        // Wait for data to become available
        while self.available_read_data() < 4 {
            // Check for shutdown while waiting
            if self.shutdown.load(Ordering::Acquire) {
                libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                return Err(anyhow!("Connection closed"));
            }

            // Wait on condition variable (releases mutex, reacquires on wake)
            libc::pthread_cond_wait(
                &self.data_ready as *const _ as *mut _,
                &self.mutex as *const _ as *mut _,
            );
        }

        // Data is available, read it
        let capacity = self.capacity.load(Ordering::Acquire);
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Read length prefix
        let mut len_bytes = [0u8; 4];
        for (i, byte) in len_bytes.iter_mut().enumerate() {
            *byte = *data_ptr.add((read_pos + i) % capacity);
        }
        let data_len = u32::from_le_bytes(len_bytes) as usize;

        // Validate data length
        if data_len > capacity {
            libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
            return Err(anyhow!(
                "Invalid data length: {} exceeds capacity {}",
                data_len,
                capacity
            ));
        }

        // Read data
        let mut data = vec![0u8; data_len];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = *data_ptr.add((read_pos + 4 + i) % capacity);
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        // Signal writer that space is available
        libc::pthread_cond_signal(&self.space_ready as *const _ as *mut _);

        // Unlock mutex
        libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);

        Ok(data)
    }
}

/// Blocking shared memory transport.
///
/// This struct implements the `BlockingTransport` trait using shared memory
/// segments with a ring buffer for message passing.
///
/// # Lifecycle
///
/// 1. Create with `new()`
/// 2. Initialize as server with `start_server_blocking()` (creates segment) OR
///    client with `start_client_blocking()` (opens existing segment)
/// 3. Send/receive messages with `send_blocking()` / `receive_blocking()`
/// 4. Clean up with `close_blocking()`
pub struct BlockingSharedMemory {
    /// Pointer to the ring buffer in shared memory
    ring_buffer: Option<*mut SharedMemoryRingBuffer>,

    /// The shared memory segment (wrapped in Arc for safety)
    #[allow(dead_code)]
    shmem: Option<Arc<Mutex<Shmem>>>,

    /// Whether this instance is the server (creator)
    is_server: bool,

    /// Name of the active shared memory segment.
    ///
    /// Stored so close_blocking() can perform deterministic cleanup
    /// (e.g., shm_unlink on server instances).
    shared_memory_name: String,
}

// Safety: The ring buffer uses atomic operations for coordination
unsafe impl Send for BlockingSharedMemory {}
unsafe impl Sync for BlockingSharedMemory {}

impl BlockingSharedMemory {
    /// Create a new shared memory transport.
    ///
    /// Creates an uninitialized transport. Call `start_server_blocking()` or
    /// `start_client_blocking()` to initialize it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::ipc::BlockingSharedMemory;
    ///
    /// let transport = BlockingSharedMemory::new();
    /// // Now call start_server_blocking() or start_client_blocking()
    /// ```
    pub fn new() -> Self {
        Self {
            ring_buffer: None,
            shmem: None,
            is_server: false,
            shared_memory_name: String::new(),
        }
    }

    /// Wait for the peer to be ready (busy-wait with yields)
    fn wait_for_peer_ready(&self, timeout: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        let ring_buffer = self
            .ring_buffer
            .ok_or_else(|| anyhow!("Ring buffer not initialized"))?;

        loop {
            let ready = if self.is_server {
                unsafe { (*ring_buffer).client_ready.load(Ordering::Acquire) }
            } else {
                unsafe { (*ring_buffer).server_ready.load(Ordering::Acquire) }
            };

            if ready {
                return Ok(());
            }

            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for peer to be ready"));
            }

            // Yield to avoid busy-waiting CPU spin
            thread::yield_now();
            // Small sleep to reduce CPU usage
            thread::sleep(Duration::from_micros(100));
        }
    }

    /// Ensure peer is ready before sending/receiving.
    /// This is called automatically on first send/receive in server mode.
    fn ensure_peer_ready(&mut self) -> Result<()> {
        // For server, we need to wait for client to connect
        if self.is_server {
            // Check if client is already ready
            let ring_buffer = self
                .ring_buffer
                .ok_or_else(|| anyhow!("Ring buffer not initialized"))?;

            let client_ready = unsafe { (*ring_buffer).client_ready.load(Ordering::Acquire) };

            if !client_ready {
                debug!("Waiting for client to connect to shared memory");
                self.wait_for_peer_ready(Duration::from_secs(30))?;
                debug!("Client connected to shared memory");
            }
        }
        Ok(())
    }
}

impl BlockingTransport for BlockingSharedMemory {
    #[allow(clippy::arc_with_non_send_sync)]
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking shared memory server with segment: {}",
            config.shared_memory_name
        );

        let buffer_size = config.buffer_size;
        let total_size = SharedMemoryRingBuffer::HEADER_SIZE + buffer_size;

        // Try to remove any existing segment first (cleanup from previous runs)
        // This is a best-effort cleanup - we ignore errors
        #[cfg(target_os = "linux")]
        {
            use std::ffi::CString;
            // Prepend "/" if not already present (POSIX shm requires it)
            let shm_name = if config.shared_memory_name.starts_with('/') {
                config.shared_memory_name.clone()
            } else {
                format!("/{}", config.shared_memory_name)
            };
            if let Ok(c_name) = CString::new(shm_name.as_bytes()) {
                unsafe {
                    libc::shm_unlink(c_name.as_ptr());
                }
            }
        }

        // Create shared memory segment
        let shmem = ShmemConf::new()
            .size(total_size)
            .os_id(&config.shared_memory_name)
            .create()
            .with_context(|| {
                format!(
                    "Failed to create shared memory segment: {}. \
                     Ensure no existing segment with this name.",
                    config.shared_memory_name
                )
            })?;

        // Initialize the ring buffer
        let ptr = shmem.as_ptr() as *mut SharedMemoryRingBuffer;
        unsafe {
            std::ptr::write(ptr, SharedMemoryRingBuffer::new(buffer_size));
            (*ptr).server_ready.store(true, Ordering::Release);
        }

        self.ring_buffer = Some(ptr);
        self.shmem = Some(Arc::new(Mutex::new(shmem)));
        self.is_server = true;
        self.shared_memory_name = config.shared_memory_name.clone();

        debug!("Shared memory server created successfully");

        // Don't wait for client here - do it on first send/receive.
        // This allows the server to signal readiness before blocking.
        Ok(())
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking shared memory client, connecting to: {}",
            config.shared_memory_name
        );

        let buffer_size = config.buffer_size;
        let total_size = SharedMemoryRingBuffer::HEADER_SIZE + buffer_size;

        // Open existing shared memory segment (retry with timeout)
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let shmem = loop {
            match ShmemConf::new()
                .size(total_size)
                .os_id(&config.shared_memory_name)
                .open()
            {
                Ok(shm) => break shm,
                Err(e) => {
                    if start.elapsed() > timeout {
                        return Err(anyhow!(
                            "Failed to open shared memory segment: {}. \
                             Is the server running? Error: {}",
                            config.shared_memory_name,
                            e
                        ));
                    }
                    // Wait a bit and retry
                    thread::sleep(Duration::from_millis(100));
                }
            }
        };

        let ptr = shmem.as_ptr() as *mut SharedMemoryRingBuffer;

        // Mark client as ready
        unsafe {
            (*ptr).client_ready.store(true, Ordering::Release);
        }

        self.ring_buffer = Some(ptr);
        self.shmem = Some(Arc::new(Mutex::new(shmem)));
        self.is_server = false;
        self.shared_memory_name = config.shared_memory_name.clone();

        debug!("Client connected to shared memory successfully");
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!(
            "Sending message ID {} via blocking shared memory",
            message.id
        );

        // Ensure peer is ready (wait for client if server, no-op if client)
        self.ensure_peer_ready()?;

        let ring_buffer = self.ring_buffer.ok_or_else(|| {
            anyhow!(
                "Cannot send: shared memory not initialized. \
                 Call start_server_blocking() or start_client_blocking() first."
            )
        })?;

        // Capture timestamp immediately before IPC syscall with minimal
        // intervening work for accurate latency measurement.
        //
        // Pre-serialize with dummy timestamp to get buffer structure, then update
        // only the timestamp bytes immediately before send. This ensures any
        // scheduling delays between timestamp capture and send are included in
        // the measured latency.
        let mut message_with_timestamp = message.clone();
        message_with_timestamp.timestamp = 0; // Dummy timestamp for pre-serialization
        let mut serialized =
            bincode::serialize(&message_with_timestamp).context("Failed to serialize message")?;

        // Timestamp will be captured inside write_data_blocking right before
        // the actual memory write, ensuring accurate latency even under backpressure

        // Send message - timestamp is updated atomically right before memory write
        // Use condition variable-based blocking write
        #[cfg(unix)]
        {
            unsafe {
                (*ring_buffer)
                    .write_data_blocking(&mut serialized, Some(Message::timestamp_offset()))?;
            }
            trace!("Message ID {} sent successfully", message.id);
            Ok(())
        }

        #[cfg(not(unix))]
        {
            // Fallback to busy-wait for non-Unix platforms
            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(5);

            loop {
                match unsafe { (*ring_buffer).write_data(&serialized) } {
                    Ok(()) => {
                        trace!("Message ID {} sent successfully", message.id);
                        return Ok(());
                    }
                    Err(_) => {
                        if start.elapsed() > timeout {
                            return Err(anyhow!(
                                "Timeout: ring buffer full, possible backpressure"
                            ));
                        }
                        thread::yield_now();
                        thread::sleep(Duration::from_micros(100));
                    }
                }
            }
        }
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via blocking shared memory");

        // Ensure peer is ready (wait for client if server, no-op if client)
        self.ensure_peer_ready()?;

        let ring_buffer = self.ring_buffer.ok_or_else(|| {
            anyhow!(
                "Cannot receive: shared memory not initialized. \
                 Call start_server_blocking() or start_client_blocking() first."
            )
        })?;

        // Use condition variable-based blocking read
        #[cfg(unix)]
        let data = unsafe { (*ring_buffer).read_data_blocking()? };

        #[cfg(not(unix))]
        let data = loop {
            match unsafe { (*ring_buffer).read_data() } {
                Ok(d) => break d,
                Err(_) => {
                    // Check for shutdown
                    if unsafe { (*ring_buffer).shutdown.load(Ordering::Acquire) } {
                        return Err(anyhow!("Connection closed"));
                    }
                    thread::yield_now();
                    thread::sleep(Duration::from_micros(100));
                }
            }
        };

        // Deserialize message
        let message: Message =
            bincode::deserialize(&data).context("Failed to deserialize message")?;

        trace!("Received message ID {}", message.id);
        Ok(message)
    }

    fn close_blocking(&mut self) -> Result<()> {
        debug!("Closing blocking shared memory transport");

        // Capture cleanup metadata before dropping local state.
        let should_unlink = self.is_server && !self.shared_memory_name.is_empty();
        let shm_name = self.shared_memory_name.clone();

        if let Some(ring_buffer) = self.ring_buffer {
            unsafe {
                (*ring_buffer).shutdown.store(true, Ordering::Release);

                // Signal all waiters to wake up and check shutdown flag
                #[cfg(unix)]
                {
                    libc::pthread_cond_broadcast(&(*ring_buffer).data_ready as *const _ as *mut _);
                    libc::pthread_cond_broadcast(&(*ring_buffer).space_ready as *const _ as *mut _);
                }
            }
        }

        self.ring_buffer = None;
        self.shmem = None;
        self.is_server = false;

        // Best-effort unlink so stale SHM objects do not accumulate across runs.
        if should_unlink {
            #[cfg(unix)]
            {
                use std::ffi::CString;
                let posix_name = if shm_name.starts_with('/') {
                    shm_name
                } else {
                    format!("/{}", shm_name)
                };

                match CString::new(posix_name.as_bytes()) {
                    Ok(c_name) => unsafe {
                        let rc = libc::shm_unlink(c_name.as_ptr());
                        if rc == 0 {
                            debug!("Unlinked SHM segment on close: {}", posix_name);
                        } else {
                            debug!(
                                "shm_unlink failed during close for {}: {}",
                                posix_name,
                                std::io::Error::last_os_error()
                            );
                        }
                    },
                    Err(_) => {
                        debug!("Skipping shm_unlink: name contains interior NUL");
                    }
                }
            }
        }

        self.shared_memory_name.clear();

        debug!("Blocking shared memory transport closed");
        Ok(())
    }
}

/// Ensure resources are cleaned up even if `close_blocking()` is never
/// called (e.g. due to a panic or early return). Sets the shutdown flag,
/// wakes any blocked readers/writers, and unlinks the SHM segment when
/// this instance is the server (creator).
impl Drop for BlockingSharedMemory {
    fn drop(&mut self) {
        // close_blocking() is idempotent; safe to call even if the
        // transport was never started or was already closed.
        if let Err(e) = self.close_blocking() {
            tracing::debug!(
                "BlockingSharedMemory::drop: close_blocking \
                 returned error (best-effort cleanup): {}",
                e
            );
        }
    }
}

// Implement Default for convenience
impl Default for BlockingSharedMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_new_creates_empty_transport() {
        let transport = BlockingSharedMemory::new();
        assert!(transport.ring_buffer.is_none());
        assert!(transport.shmem.is_none());
        assert!(!transport.is_server);
    }

    #[test]
    fn test_server_creates_segment_successfully() {
        let segment_name = "test_shm_blocking_server";

        // Server now only creates segment in start_server_blocking(), doesn't wait for client
        // Wait for client happens on first send/receive via ensure_peer_ready()
        let handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_name.to_string(),
                buffer_size: 4096,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Trigger peer wait by receiving a message
            let _msg = server.receive_blocking().unwrap();
            server.close_blocking().unwrap();
        });

        // Give server time to create segment
        thread::sleep(Duration::from_millis(200));

        // Connect from client
        let mut client = BlockingSharedMemory::new();
        let client_config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 4096,
            ..Default::default()
        };
        client.start_client_blocking(&client_config).unwrap();

        // Send a message so server can receive it
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Server should complete
        handle.join().unwrap();
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        let segment_name = "test_shm_blocking_no_server";

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 4096,
            ..Default::default()
        };

        // This should timeout since no server exists
        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        // Note: This test may take 30 seconds due to timeout
    }

    #[test]
    fn test_send_and_receive_message() {
        let segment_name = "test_shm_blocking_send_recv";

        // Start server in thread
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_name.to_string(),
                buffer_size: 4096,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive message
            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 42);
            assert_eq!(msg.payload.len(), 100);

            server.close_blocking().unwrap();
        });

        // Give server time to start
        thread::sleep(Duration::from_millis(200));

        // Connect client and send
        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 4096,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(42, vec![0u8; 100], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Wait for server
        server_handle.join().unwrap();
    }

    #[test]
    #[ignore] // TODO: Ring buffer needs bidirectional support for round-trip
    fn test_round_trip_communication() {
        let segment_name = "test_shm_blocking_round_trip";

        // Start server
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_name.to_string(),
                buffer_size: 4096,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive request
            let request = server.receive_blocking().unwrap();
            assert_eq!(request.message_type, MessageType::Request);

            // Send response
            let response = Message::new(request.id, Vec::new(), MessageType::Response);
            server.send_blocking(&response).unwrap();
            server.close_blocking().unwrap();
        });

        // Give server time to start
        thread::sleep(Duration::from_millis(200));

        // Client
        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 4096,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send request
        let request = Message::new(123, vec![1, 2, 3], MessageType::Request);
        client.send_blocking(&request).unwrap();

        // Receive response
        let response = client.receive_blocking().unwrap();
        assert_eq!(response.message_type, MessageType::Response);
        assert_eq!(response.id, 123);

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    #[test]
    fn test_close_cleanup() {
        let segment_name = "test_shm_blocking_close";

        // Test that close() properly cleans up resources
        let mut server = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 4096,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();
        server.close_blocking().unwrap();

        // Verify server fields are None after close
        assert!(server.ring_buffer.is_none());
        assert!(server.shmem.is_none());

        // Test client cleanup
        let mut client = BlockingSharedMemory::new();
        client.close_blocking().unwrap();

        // Verify client fields are None after close
        assert!(client.ring_buffer.is_none());
        assert!(client.shmem.is_none());
    }

    #[test]
    fn test_default_creates_new_transport() {
        let transport = BlockingSharedMemory::default();
        assert!(transport.ring_buffer.is_none());
        assert!(transport.shmem.is_none());
        assert!(!transport.is_server);
    }

    #[test]
    fn test_multiple_messages() {
        let segment_name = "test_shm_blocking_multi_msg";

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_name.to_string(),
                buffer_size: 8192,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive multiple messages
            for expected_id in 1..=5 {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, expected_id);
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 8192,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send multiple messages
        for id in 1..=5 {
            let msg = Message::new(id, vec![0u8; 50], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    #[test]
    fn test_large_message() {
        let segment_name = "test_shm_blocking_large_msg";
        let large_size = 10000; // 10KB message

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_name.to_string(),
                buffer_size: 65536, // Large buffer
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 1);
            assert_eq!(msg.payload.len(), large_size);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name.to_string(),
            buffer_size: 65536,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, vec![0xAB; large_size], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Verifies that the Drop implementation calls
    /// close_blocking() for cleanup without panicking.
    #[test]
    fn test_drop_calls_close() {
        let transport = BlockingSharedMemory::new();
        drop(transport);
    }

    /// Verifies that dropping a started server without calling
    /// close_blocking() still cleans up the SHM segment. This
    /// exercises the Drop impl on a transport that has live
    /// resources (ring_buffer, shmem, is_server=true).
    #[test]
    fn test_drop_cleans_up_server_resources() {
        // macOS limits POSIX shm names to 31 chars (incl. leading '/')
        let id = &uuid::Uuid::new_v4().as_simple().to_string()[..8];
        let seg = format!("shm_drop_{}", id);
        let mut server = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: seg.clone(),
            buffer_size: 4096,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();

        // Confirm SHM segment exists before drop
        #[cfg(target_os = "linux")]
        {
            let shm_path = format!("/dev/shm/{}", seg);
            assert!(
                std::path::Path::new(&shm_path).exists(),
                "SHM segment should exist before drop"
            );
        }

        // Drop without calling close_blocking()
        drop(server);

        // Confirm SHM segment is removed after drop
        #[cfg(target_os = "linux")]
        {
            let shm_path = format!("/dev/shm/{}", seg);
            assert!(
                !std::path::Path::new(&shm_path).exists(),
                "SHM segment should be unlinked after drop"
            );
        }
    }

    /// Verifies that close_blocking() on the server unlinks the
    /// SHM segment from /dev/shm so it does not leak.
    #[test]
    fn test_close_unlinks_shm_segment() {
        // macOS limits POSIX shm names to 31 chars (incl. leading '/')
        let id = &uuid::Uuid::new_v4().as_simple().to_string()[..8];
        let seg = format!("shm_unlnk_{}", id);
        let mut server = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: seg.clone(),
            buffer_size: 4096,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();

        #[cfg(target_os = "linux")]
        {
            let shm_path = format!("/dev/shm/{}", seg);
            assert!(
                std::path::Path::new(&shm_path).exists(),
                "SHM segment should exist after start"
            );
        }

        server.close_blocking().unwrap();

        #[cfg(target_os = "linux")]
        {
            let shm_path = format!("/dev/shm/{}", seg);
            assert!(
                !std::path::Path::new(&shm_path).exists(),
                "SHM should be unlinked after close"
            );
        }

        // close is idempotent — second call must not panic
        server.close_blocking().unwrap();
    }
}
