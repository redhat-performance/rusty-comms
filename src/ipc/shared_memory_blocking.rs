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
pub struct SharedMemoryRingBuffer {
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
    /// Header size for the ring buffer structure.
    pub const HEADER_SIZE: usize = std::mem::size_of::<Self>();

    /// Create a new ring buffer header with process-shared synchronization
    pub fn new(capacity: usize) -> Self {
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
    /// Used by the non-Unix fallback path in send_blocking().
    #[cfg(not(unix))]
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

        // Write data using bulk copy (memcpy) when possible
        let data_start = (write_pos + 4) % capacity;
        unsafe {
            if data_start + data_len <= capacity {
                // Data fits contiguously - use fast memcpy
                std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), data_len);
            } else {
                // Data wraps around - copy in two parts
                let first_part = capacity - data_start;
                std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), first_part);
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(first_part),
                    data_ptr,
                    data_len - first_part,
                );
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
    /// Used by the non-Unix fallback path in receive_blocking().
    #[cfg(not(unix))]
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

        // Read data using bulk copy (memcpy) when possible
        let mut data = vec![0u8; data_len];
        let data_start = (read_pos + 4) % capacity;
        unsafe {
            if data_start + data_len <= capacity {
                // Data is contiguous - use fast memcpy
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(data_start),
                    data.as_mut_ptr(),
                    data_len,
                );
            } else {
                // Data wraps around - copy in two parts
                let first_part = capacity - data_start;
                std::ptr::copy_nonoverlapping(
                    data_ptr.add(data_start),
                    data.as_mut_ptr(),
                    first_part,
                );
                std::ptr::copy_nonoverlapping(
                    data_ptr,
                    data.as_mut_ptr().add(first_part),
                    data_len - first_part,
                );
            }
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        Ok(data)
    }

    /// Write data to the ring buffer (blocking with condition variable).
    ///
    /// Uses pthread_cond_wait to block until space is available, then writes
    /// the data and signals any waiting readers.
    ///
    /// # Parameters
    /// - `data`: The message data to write (may be mutated for timestamp update)
    /// - `timestamp_offset`: Optional range in data where timestamp should be updated
    /// - `cross_container`: If true, use container-safe timed waits; if false, use fast pthread_cond_wait
    ///
    /// # Safety
    /// Only available on Unix platforms with pthread support.
    #[cfg(unix)]
    unsafe fn write_data_blocking(
        &self,
        data: &mut [u8],
        timestamp_offset: Option<std::ops::Range<usize>>,
        cross_container: bool,
    ) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        // Lock mutex
        libc::pthread_mutex_lock(&self.mutex as *const _ as *mut _);

        if cross_container {
            // Container-safe path: Use timed waits with polling fallback
            let mut wait_count = 0;
            let loop_start = std::time::Instant::now();
            while self.available_write_space() < required_space {
                if self.shutdown.load(Ordering::Acquire) {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    return Err(anyhow!("Connection closed"));
                }

                // Detect broken pthread primitives
                if wait_count >= 100 && loop_start.elapsed() < Duration::from_millis(10) {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    trace!("Detected broken pthread condvar, falling back to polling");
                    return self.write_data_polling(data, timestamp_offset.clone());
                }

                // Use timed wait (500µs) for cross-container robustness
                let mut timeout = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                libc::clock_gettime(libc::CLOCK_REALTIME, &mut timeout);
                timeout.tv_nsec += 500_000; // 500µs
                if timeout.tv_nsec >= 1_000_000_000 {
                    timeout.tv_sec += 1;
                    timeout.tv_nsec -= 1_000_000_000;
                }

                libc::pthread_cond_timedwait(
                    &self.space_ready as *const _ as *mut _,
                    &self.mutex as *const _ as *mut _,
                    &timeout,
                );

                wait_count += 1;
                if wait_count > 60000 {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    return Err(anyhow!("Timeout waiting for buffer space"));
                }
            }
        } else {
            // Fast path: Use pthread_cond_wait (infinite wait, lowest latency)
            while self.available_write_space() < required_space {
                if self.shutdown.load(Ordering::Acquire) {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    return Err(anyhow!("Connection closed"));
                }

                // Wait on condition variable (releases mutex, reacquires on wake)
                libc::pthread_cond_wait(
                    &self.space_ready as *const _ as *mut _,
                    &self.mutex as *const _ as *mut _,
                );
            }
        }

        // CRITICAL: Update timestamp RIGHT BEFORE writing to shared memory
        // This ensures accurate latency measurement even under backpressure
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

        // Write data using bulk copy (memcpy) when possible.
        // This is dramatically faster than byte-by-byte copy with modular
        // arithmetic, which prevents SIMD vectorization.
        let data_start = (write_pos + 4) % capacity;
        if data_start + data_len <= capacity {
            // Data fits contiguously - use fast memcpy
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), data_len);
        } else {
            // Data wraps around - copy in two parts
            let first_part = capacity - data_start;
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), first_part);
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(first_part),
                data_ptr,
                data_len - first_part,
            );
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

    /// Fallback polling-based write when pthread primitives don't work.
    ///
    /// This is used when pthread_cond_timedwait fails, which can happen
    /// in containerized environments where process-shared mutexes/condvars
    /// may not work correctly across the container boundary.
    ///
    /// # Safety
    /// Only available on Unix platforms.
    #[cfg(unix)]
    unsafe fn write_data_polling(
        &self,
        data: &mut [u8],
        timestamp_offset: Option<std::ops::Range<usize>>,
    ) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(10);

        // Poll for space availability with sleep
        while self.available_write_space() < required_space {
            if self.shutdown.load(Ordering::Acquire) {
                return Err(anyhow!("Connection closed"));
            }

            if start.elapsed() > timeout {
                return Err(anyhow!(
                    "Timeout waiting for buffer space (polling fallback)"
                ));
            }

            // Sleep to avoid busy-waiting
            thread::sleep(Duration::from_micros(100));
        }

        // CRITICAL: Update timestamp RIGHT BEFORE writing to shared memory
        // This ensures accurate latency measurement even under backpressure
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

        // Write data using bulk copy (memcpy) when possible
        let data_start = (write_pos + 4) % capacity;
        if data_start + data_len <= capacity {
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), data_len);
        } else {
            let first_part = capacity - data_start;
            std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), first_part);
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(first_part),
                data_ptr,
                data_len - first_part,
            );
        }

        self.write_pos
            .store((write_pos + required_space) % capacity, Ordering::Release);
        self.message_count.fetch_add(1, Ordering::Release);

        // Signal any reader blocked on the condvar path.
        // Without this, a blocking reader (read_data_blocking) paired
        // with a polling writer would never be woken, causing a
        // deadlock in mixed-mode operation.
        libc::pthread_cond_signal(&self.data_ready as *const _ as *mut _);

        Ok(())
    }

    /// Read data from the ring buffer (blocking with condition variable).
    ///
    /// Uses pthread_cond_wait to block until data is available, then reads
    /// it and signals any waiting writers.
    ///
    /// # Parameters
    /// - `cross_container`: If true, use container-safe timed waits; if false, use fast pthread_cond_wait
    ///
    /// # Safety
    /// Only available on Unix platforms with pthread support.
    #[cfg(unix)]
    unsafe fn read_data_blocking(&self, cross_container: bool) -> Result<Vec<u8>> {
        // Lock mutex
        libc::pthread_mutex_lock(&self.mutex as *const _ as *mut _);

        if cross_container {
            // Container-safe path: Use timed waits with polling fallback
            let mut wait_count = 0;
            let loop_start = std::time::Instant::now();
            while self.available_read_data() < 4 {
                if self.shutdown.load(Ordering::Acquire) {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    return Err(anyhow!("Connection closed"));
                }

                // Detect broken pthread primitives
                if wait_count >= 100 && loop_start.elapsed() < Duration::from_millis(10) {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    trace!("Detected broken pthread condvar, falling back to polling");
                    return self.read_data_polling();
                }

                // Use timed wait (500µs) for cross-container robustness
                let mut timeout = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                libc::clock_gettime(libc::CLOCK_REALTIME, &mut timeout);
                timeout.tv_nsec += 500_000; // 500µs
                if timeout.tv_nsec >= 1_000_000_000 {
                    timeout.tv_sec += 1;
                    timeout.tv_nsec -= 1_000_000_000;
                }

                libc::pthread_cond_timedwait(
                    &self.data_ready as *const _ as *mut _,
                    &self.mutex as *const _ as *mut _,
                    &timeout,
                );

                wait_count += 1;
                if wait_count > 60000 {
                    libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);
                    return Err(anyhow!("Timeout waiting for data"));
                }
            }
        } else {
            // Fast path: Use pthread_cond_wait (infinite wait, lowest latency)
            while self.available_read_data() < 4 {
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

        // Read data using bulk copy (memcpy) when possible
        let mut data = vec![0u8; data_len];
        let data_start = (read_pos + 4) % capacity;
        if data_start + data_len <= capacity {
            // Data is contiguous - use fast memcpy
            std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), data_len);
        } else {
            // Data wraps around - copy in two parts
            let first_part = capacity - data_start;
            std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), first_part);
            std::ptr::copy_nonoverlapping(
                data_ptr,
                data.as_mut_ptr().add(first_part),
                data_len - first_part,
            );
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        // Signal writer that space is available
        libc::pthread_cond_signal(&self.space_ready as *const _ as *mut _);

        // Unlock mutex
        libc::pthread_mutex_unlock(&self.mutex as *const _ as *mut _);

        Ok(data)
    }

    /// Fallback polling-based read when pthread primitives don't work.
    ///
    /// This is used when pthread_cond_timedwait fails, which can happen
    /// in containerized environments where process-shared mutexes/condvars
    /// may not work correctly across the container boundary.
    ///
    /// # Safety
    /// Only available on Unix platforms.
    #[cfg(unix)]
    unsafe fn read_data_polling(&self) -> Result<Vec<u8>> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(10);

        // Poll for data availability with sleep
        while self.available_read_data() < 4 {
            if self.shutdown.load(Ordering::Acquire) {
                return Err(anyhow!("Connection closed"));
            }

            if start.elapsed() > timeout {
                return Err(anyhow!("Timeout waiting for data (polling fallback)"));
            }

            // Sleep to avoid busy-waiting
            thread::sleep(Duration::from_micros(100));
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
            return Err(anyhow!(
                "Invalid data length: {} exceeds capacity {}",
                data_len,
                capacity
            ));
        }

        // Read data using bulk copy (memcpy) when possible
        let mut data = vec![0u8; data_len];
        let data_start = (read_pos + 4) % capacity;
        if data_start + data_len <= capacity {
            std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), data_len);
        } else {
            let first_part = capacity - data_start;
            std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), first_part);
            std::ptr::copy_nonoverlapping(
                data_ptr,
                data.as_mut_ptr().add(first_part),
                data_len - first_part,
            );
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        // Wake writers that may be blocked on the condvar path.
        // This keeps mixed-mode operation (polling reader + blocking writer)
        // from stalling when cross-container fallback is active on one side.
        libc::pthread_cond_signal(&self.space_ready as *const _ as *mut _);

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

    /// Cross-container mode flag
    ///
    /// When true, uses container-safe synchronization (timed waits, polling fallbacks).
    /// When false (default), uses fast pthread_cond_wait for same-host communication.
    cross_container: bool,
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
            cross_container: false, // Default to fast same-host mode
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
                self.wait_for_peer_ready(Duration::from_secs(10))?;
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

        // Check if we should open existing segment instead of creating
        // (Used when host pre-creates the segment for container access)
        let open_existing = config.shm_open_existing;

        let shmem = if open_existing {
            // Open existing segment (created by host)
            let start = std::time::Instant::now();
            let timeout = Duration::from_secs(10);
            loop {
                match ShmemConf::new()
                    .size(total_size)
                    .os_id(&config.shared_memory_name)
                    .open()
                {
                    Ok(shm) => break shm,
                    Err(e) => {
                        if start.elapsed() > timeout {
                            return Err(anyhow!(
                                "Failed to open shared memory segment: {}. Error: {}",
                                config.shared_memory_name,
                                e
                            ));
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        } else {
            // Create new segment (normal standalone mode)
            ShmemConf::new()
                .size(total_size)
                .os_id(&config.shared_memory_name)
                .create()
                .with_context(|| {
                    format!(
                        "Failed to create shared memory segment: {}. \
                         Ensure no existing segment with this name.",
                        config.shared_memory_name
                    )
                })?
        };

        let ptr = shmem.as_ptr() as *mut SharedMemoryRingBuffer;

        if open_existing {
            // Opening existing segment - just set server ready, don't reinitialize
            unsafe {
                (*ptr).server_ready.store(true, Ordering::Release);
            }
            debug!("Opened existing SHM segment as server");
        } else {
            // Creating new segment - initialize the ring buffer
            unsafe {
                std::ptr::write(ptr, SharedMemoryRingBuffer::new(buffer_size));
                (*ptr).server_ready.store(true, Ordering::Release);
            }

            // Set SHM permissions to 777 for container access
            // The file is at /dev/shm/<name> (strip leading "/" from shm name)
            #[cfg(unix)]
            {
                let shm_file_name = config
                    .shared_memory_name
                    .strip_prefix('/')
                    .unwrap_or(&config.shared_memory_name);
                let shm_path = format!("/dev/shm/{}", shm_file_name);
                if let Ok(c_path) = std::ffi::CString::new(shm_path.as_bytes()) {
                    let result = unsafe { libc::chmod(c_path.as_ptr(), 0o777) };
                    if result == 0 {
                        debug!("Set SHM permissions to 777");
                    } else {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(-1);
                        debug!("Failed to set SHM permissions: errno={}", errno);
                    }
                }
            }

            debug!("Created new SHM segment as server");
        }

        self.ring_buffer = Some(ptr);
        self.shmem = Some(Arc::new(Mutex::new(shmem)));
        self.is_server = true;
        self.shared_memory_name = config.shared_memory_name.clone();
        self.cross_container = config.cross_container;

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
        let timeout = Duration::from_secs(10);
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
            // Signal data_ready in case server is waiting on condvar.
            // Only available on Unix where pthread condvars exist.
            #[cfg(unix)]
            {
                libc::pthread_cond_broadcast(&(*ptr).data_ready as *const _ as *mut _);
            }
        }

        self.ring_buffer = Some(ptr);
        self.shmem = Some(Arc::new(Mutex::new(shmem)));
        self.is_server = false;
        self.shared_memory_name = config.shared_memory_name.clone();
        self.cross_container = config.cross_container;

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

        // Serialize the message directly (no clone needed).
        // The timestamp in the serialized bytes will be overwritten right before
        // the actual memory write inside write_data_blocking, ensuring accurate
        // latency measurement.
        let mut serialized = bincode::serialize(message).context("Failed to serialize message")?;

        // Timestamp will be captured inside write_data_blocking right before
        // the actual memory write, ensuring accurate latency even under backpressure

        // Send message - timestamp is updated atomically right before memory write
        // Use condition variable-based blocking write
        #[cfg(unix)]
        {
            unsafe {
                (*ring_buffer).write_data_blocking(
                    &mut serialized,
                    Some(Message::timestamp_offset()),
                    self.cross_container,
                )?;
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
        let data = unsafe { (*ring_buffer).read_data_blocking(self.cross_container)? };

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

        // CRITICAL: Capture receive timestamp immediately after read
        // This ensures accurate latency measurement
        let receive_time_ns = crate::ipc::get_monotonic_time_ns();

        // Deserialize message
        let mut message: Message =
            bincode::deserialize(&data).context("Failed to deserialize message")?;

        // Calculate and store one-way latency using accurate receive timestamp
        // Only for Request/OneWay messages - Response messages already have
        // one_way_latency_ns set by the server, so don't overwrite it
        if message.message_type != crate::ipc::MessageType::Response {
            message.one_way_latency_ns = receive_time_ns.saturating_sub(message.timestamp);
        }

        trace!("Received message ID {}", message.id);
        Ok(message)
    }

    fn close_blocking(&mut self) -> Result<()> {
        debug!("Closing blocking shared memory transport");

        // Capture cleanup metadata before dropping local state.
        #[cfg(target_os = "linux")]
        let should_unlink = self.is_server && !self.shared_memory_name.is_empty();
        #[cfg(target_os = "linux")]
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
        #[cfg(target_os = "linux")]
        if should_unlink {
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
                    debug!("Skipping shm_unlink: SHM name contains interior NUL");
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
    use std::sync::mpsc;
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
        let (received_tx, received_rx) = mpsc::channel();

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
            // Signal client that the message has been consumed so the client
            // does not close the transport first and trigger a shutdown race.
            received_tx.send(()).unwrap();

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

        // Wait until server confirms receipt before closing client transport.
        received_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test that send_blocking fails when the transport is not
    /// initialized (no shared memory segment).
    #[test]
    fn test_send_on_uninit_transport_fails() {
        let mut transport = BlockingSharedMemory::new();
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);

        let result = transport.send_blocking(&msg);
        assert!(
            result.is_err(),
            "send on uninitialized SHM transport should fail"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not initialized"),
            "Error should mention 'not initialized', got: {}",
            err_msg,
        );
    }

    /// Test that receive_blocking fails when the transport is not
    /// initialized.
    #[test]
    fn test_receive_on_uninit_transport_fails() {
        let mut transport = BlockingSharedMemory::new();
        let result = transport.receive_blocking();
        assert!(
            result.is_err(),
            "receive on uninitialized SHM transport should fail"
        );
    }

    /// Test that close_blocking is idempotent on a fresh transport.
    #[test]
    fn test_close_idempotent_uninit() {
        let mut transport = BlockingSharedMemory::new();
        transport.close_blocking().unwrap();
        transport.close_blocking().unwrap();
        assert!(transport.ring_buffer.is_none());
        assert!(transport.shmem.is_none());
    }

    /// Test cross-container mode for send/receive.
    ///
    /// The cross_container flag enables timed waits with polling fallback
    /// instead of bare pthread_cond_wait, so that broken pthread condvars
    /// (which can happen across PID namespaces) are tolerated.
    #[test]
    #[cfg(unix)]
    fn test_cross_container_send_receive() {
        let segment_name = format!(
            "test_shm_cc_{}",
            uuid::Uuid::new_v4().as_u128() & 0xffff_ffff
        );
        let segment_clone = segment_name.clone();
        let (received_tx, received_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_clone,
                buffer_size: 8192,
                cross_container: true,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 77);
            assert_eq!(msg.payload.len(), 64);
            received_tx.send(()).unwrap();

            server.close_blocking().unwrap();
        });

        // Allow server to create segment
        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 8192,
            cross_container: true,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(77, vec![0xBBu8; 64], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        received_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test that the ring buffer wrap-around logic works correctly.
    ///
    /// Uses a small buffer size to force write_pos to wrap around the end
    /// of the buffer, exercising the two-part memcpy paths in both
    /// write_data_blocking and read_data_blocking.
    #[test]
    fn test_ring_buffer_wrap_around() {
        let segment_name = format!(
            "test_shm_wrap_{}",
            uuid::Uuid::new_v4().as_u128() & 0xffff_ffff
        );
        let segment_clone = segment_name.clone();
        let msg_count = 20;
        let (done_tx, done_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_clone,
                // Small buffer forces frequent wrap-around
                buffer_size: 256,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for expected_id in 1..=msg_count {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, expected_id);
            }
            done_tx.send(()).unwrap();
            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 256,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        for id in 1..=msg_count {
            let msg = Message::new(id, vec![0xCDu8; 20], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test that receive_blocking preserves one_way_latency_ns for Response messages
    /// instead of overwriting it with a new calculation.
    #[test]
    fn test_receive_preserves_response_latency() {
        let segment_name = format!(
            "test_shm_resp_{}",
            uuid::Uuid::new_v4().as_u128() & 0xffff_ffff
        );
        let segment_clone = segment_name.clone();
        let (received_tx, received_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_clone,
                buffer_size: 4096,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive the message and check its latency was calculated
            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 99);
            assert_eq!(msg.message_type, MessageType::OneWay);
            assert!(
                msg.one_way_latency_ns > 0,
                "OneWay message should have latency calculated"
            );
            received_tx.send(()).unwrap();

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 4096,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(99, vec![0u8; 16], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        received_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test multiple messages with cross-container mode to exercise
    /// timed waits and signaling across multiple send/receive cycles.
    #[test]
    #[cfg(unix)]
    fn test_cross_container_multiple_messages() {
        let segment_name = format!(
            "test_shm_ccm_{}",
            uuid::Uuid::new_v4().as_u128() & 0xffff_ffff
        );
        let segment_clone = segment_name.clone();
        let msg_count = 10;
        let (done_tx, done_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: segment_clone,
                buffer_size: 4096,
                cross_container: true,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for expected_id in 1..=msg_count {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, expected_id);
            }
            done_tx.send(()).unwrap();
            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 4096,
            cross_container: true,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        for id in 1..=msg_count {
            let msg = Message::new(id, vec![0u8; 32], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Sending before the transport is initialized should fail.
    #[test]
    fn test_shm_blocking_send_when_not_initialized() {
        let mut transport = BlockingSharedMemory::new();
        let msg = Message::new(1, vec![0u8; 32], MessageType::OneWay);
        let result = transport.send_blocking(&msg);
        assert!(
            result.is_err(),
            "Send on uninitialized transport should fail"
        );
    }

    /// Receiving before the transport is initialized should fail.
    #[test]
    fn test_shm_blocking_receive_when_not_initialized() {
        let mut transport = BlockingSharedMemory::new();
        let result = transport.receive_blocking();
        assert!(
            result.is_err(),
            "Receive on uninitialized transport should fail"
        );
    }

    /// Verify that close_blocking is safe to call multiple times.
    #[test]
    fn test_shm_blocking_close_twice() {
        let segment_name = format!(
            "test_shm_ct_{}",
            uuid::Uuid::new_v4().as_u128() & 0xffff_ffff
        );
        let mut server = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 4096,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();

        server.close_blocking().unwrap();
        // Second close should be a no-op, not a panic
        server.close_blocking().unwrap();
    }

    /// Test that the round-trip / one-way latency ratio is reasonable.
    ///
    /// A healthy ratio is approximately 2× (one OW leg in each direction).
    /// Before the ring-buffer memcpy and timestamp fixes, this ratio was ~8×.
    /// This test ensures the fix holds by asserting the ratio stays below 4×.
    ///
    /// NOTE: This test is currently `#[ignore]` because the blocking SHM ring buffer
    /// uses a single `read_pos`/`write_pos` pair shared by both sides. In round-trip
    /// mode (client sends, then reads; server reads, then sends), a side can read
    /// its own message due to the race for the mutex, corrupting the exchange.
    /// See also `test_round_trip_communication` which is ignored for the same reason.
    /// This test will become active once the ring buffer supports bidirectional
    /// communication (e.g., dual ring buffers or per-direction slots).
    #[test]
    #[ignore] // Ring buffer is unidirectional - both sides race for the same read_pos
    fn test_rt_ow_latency_ratio_shm_blocking() {
        use crate::ipc::MessageType;
        use uuid::Uuid;

        let num_iterations: usize = 1000;
        let payload_size = 64;
        let segment_name = format!("test_rtow_{}", Uuid::new_v4());

        // Server thread: receives Request, sends back Response with the OW latency embedded
        let server_name = segment_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemory::new();
            let config = TransportConfig {
                shared_memory_name: server_name,
                buffer_size: 65536,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for _ in 0..num_iterations {
                // Receive request from client
                let request = server.receive_blocking().unwrap();
                assert_eq!(request.message_type, MessageType::Request);

                // Send response back, embedding the measured one_way_latency_ns
                let mut response =
                    Message::new(request.id, vec![0u8; payload_size], MessageType::Response);
                response.one_way_latency_ns = request.one_way_latency_ns;
                server.send_blocking(&response).unwrap();
            }

            server.close_blocking().unwrap();
        });

        // Give server time to create the segment
        thread::sleep(Duration::from_millis(200));

        // Client: connect, do round-trips, measure OW and RT
        let mut client = BlockingSharedMemory::new();
        let config = TransportConfig {
            shared_memory_name: segment_name,
            buffer_size: 65536,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let mut ow_latencies = Vec::with_capacity(num_iterations);
        let mut rt_latencies = Vec::with_capacity(num_iterations);

        // Warmup: do a few iterations to stabilize caches / scheduling
        let warmup = 50;
        for i in 0..warmup {
            let payload = vec![0xABu8; payload_size];
            let msg = Message::new(i as u64, payload, MessageType::Request);
            let send_time = crate::ipc::get_monotonic_time_ns();
            client.send_blocking(&msg).unwrap();
            let response = client.receive_blocking().unwrap();
            let _rt = crate::ipc::get_monotonic_time_ns() - send_time;
            let _ow = response.one_way_latency_ns;
            // discard warmup
        }

        // Measured iterations
        for i in warmup..num_iterations {
            let payload = vec![0xABu8; payload_size];
            let msg = Message::new(i as u64, payload, MessageType::Request);
            let send_time = crate::ipc::get_monotonic_time_ns();
            client.send_blocking(&msg).unwrap();
            let response = client.receive_blocking().unwrap();
            let rt = crate::ipc::get_monotonic_time_ns() - send_time;
            let ow = response.one_way_latency_ns;

            if ow > 0 {
                ow_latencies.push(ow);
                rt_latencies.push(rt);
            }
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        // Compute mean latencies
        assert!(
            !ow_latencies.is_empty(),
            "Should have collected OW latency samples"
        );

        let mean_ow: f64 = ow_latencies.iter().sum::<u64>() as f64 / ow_latencies.len() as f64;
        let mean_rt: f64 = rt_latencies.iter().sum::<u64>() as f64 / rt_latencies.len() as f64;
        let ratio = mean_rt / mean_ow;

        eprintln!(
            "[shm_blocking] Mean OW: {:.0} ns, Mean RT: {:.0} ns, Ratio RT/OW: {:.2}x ({} samples)",
            mean_ow,
            mean_rt,
            ratio,
            ow_latencies.len()
        );

        // The ratio should be approximately 2× (one OW in each direction).
        // Allow up to 4× to account for scheduling jitter and test environment noise.
        // Before the fix, this was consistently ~8×.
        assert!(
            ratio < 4.0,
            "RT/OW ratio {:.2}x is too high (expected < 4.0x). \
             Mean OW: {:.0} ns, Mean RT: {:.0} ns. \
             This indicates the ring buffer latency fixes may have regressed.",
            ratio,
            mean_ow,
            mean_rt,
        );
    }
}
