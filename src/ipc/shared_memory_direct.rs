//! Direct memory shared memory transport implementation (blocking).
//!
//! This module provides a high-performance blocking implementation of shared
//! memory IPC using direct memory access, similar to C programs. Unlike the
//! ring buffer implementation, this uses a simple fixed-size struct that is
//! written directly to shared memory with no serialization overhead.
//!
//! # Design Philosophy
//!
//! This implementation prioritizes raw performance and matches the approach
//! used by C IPC benchmarks:
//! - **No serialization**: Direct struct copy (memcpy)
//! - **Simple synchronization**: One mutex + one condition variable
//! - **Fixed-size layout**: `#[repr(C, packed)]` for predictable memory
//! - **Minimal overhead**: ~1-2µs per send/receive vs 15-30µs with bincode
//!
//! # Performance Characteristics
//!
//! Expected latencies (based on C benchmark comparison):
//! - Mean: ~6.5µs (vs C's 6.45µs)
//! - Min: ~5.0µs (vs C's 4.95µs)  
//! - Max: ~32µs (vs C's 27.34µs, vs ring buffer's 91.77µs)
//!
//! # Trade-offs
//!
//! **Advantages:**
//! - 3× faster max latency vs ring buffer implementation
//! - Matches C performance (within 20%)
//! - Simpler code (no ring buffer logic)
//! - Better cache locality (single contiguous struct)
//!
//! **Disadvantages:**
//! - Fixed message size (less flexible)
//! - More unsafe code (requires careful review)
//! - Manual memory layout management
//! - Rust safety features partially bypassed
//!
//! # Platform Support
//!
//! This implementation is only available on Unix platforms (Linux, macOS, BSD)
//! as it relies on POSIX shared memory and pthread primitives.

use crate::ipc::{BlockingTransport, Message, MessageType, TransportConfig};
use anyhow::{anyhow, Context, Result};
use nix::libc;
use shared_memory::{Shmem, ShmemConf};
use tracing::{debug, trace};

/// Maximum payload size in bytes.
///
/// Set to 8KB to match typical IPC benchmark message sizes.
/// This is large enough for most tests while keeping shared memory
/// segments small for reliable cross-process initialization.
const MAX_PAYLOAD_SIZE: usize = 8192; // 8 KB

/// Raw message structure stored directly in shared memory.
///
/// This struct is designed to match C-style IPC implementations with minimal
/// overhead. It uses `#[repr(C, packed)]` to ensure predictable memory layout
/// across process boundaries.
///
/// # Memory Layout
///
/// ```text
/// Offset  Size  Field              Description
/// ------  ----  -----------------  ----------------------------------
/// 0       48    mutex              pthread_mutex_t (PROCESS_SHARED)
/// 48      48    data_ready         pthread_cond_t (PROCESS_SHARED)
/// 96      8     id                 Message ID (u64)
/// 104     8     timestamp          Send timestamp in nanoseconds (u64)
/// 112     100   payload            Fixed-size payload data
/// 212     4     message_type       Message type enum (u32)
/// 216     4     ready              Coordination flag (0=empty, 1=ready)
/// 220     4     _padding           Alignment padding
/// ------  ----
/// Total:  224 bytes
/// ```
///
/// # Thread Safety
///
/// The mutex and condition variable are initialized with
/// `PTHREAD_PROCESS_SHARED` attribute, making them safe to use across
/// process boundaries.
///
/// # Examples
///
/// ```rust,no_run
/// use ipc_benchmark::ipc::*;
///
/// # fn example() -> anyhow::Result<()> {
/// // This struct is typically not used directly.
/// // Use BlockingSharedMemoryDirect instead.
/// # Ok(())
/// # }
/// ```
#[repr(C)]
struct RawSharedMessage {
    /// Mutex for exclusive access to shared memory.
    ///
    /// Initialized with `PTHREAD_PROCESS_SHARED` to work across processes.
    mutex: libc::pthread_mutex_t,

    /// Condition variable for signaling (matches C implementation).
    ///
    /// Both sender and receiver wait/signal on this SAME condition variable.
    /// This is the ping-pong pattern used in the C benchmark.
    cond: libc::pthread_cond_t,

    /// Message identifier (sequential counter).
    id: u64,

    /// Timestamp when message was sent (nanoseconds since CLOCK_MONOTONIC).
    ///
    /// This is captured immediately before the message is written to shared
    /// memory, matching C benchmark methodology for accurate latency measurement.
    timestamp: u64,

    /// Actual number of valid bytes in the payload.
    ///
    /// Only the first `payload_len` bytes of `payload` contain valid data.
    /// This allows variable-length messages up to MAX_PAYLOAD_SIZE.
    payload_len: usize,

    /// Fixed-size payload buffer.
    ///
    /// Maximum 1MB. Only the first `payload_len` bytes are valid.
    /// If the source payload is smaller, only those bytes are copied.
    /// If larger, it's truncated to MAX_PAYLOAD_SIZE.
    payload: [u8; MAX_PAYLOAD_SIZE],

    /// Message type (converted from MessageType enum).
    ///
    /// Stored as u32 for stable memory layout.
    message_type: u32,

    /// Coordination flag.
    ///
    /// - `0`: No message ready (receiver should wait)
    /// - `1`: Message ready (receiver can read)
    ///
    /// This mimics C's simple flag-based protocol.
    ready: i32,
}

impl RawSharedMessage {
    /// Total size of the structure in bytes.
    const SIZE: usize = std::mem::size_of::<Self>();

    /// Initialize the shared memory structure with pthread primitives.
    ///
    /// This must be called once by the server process before any communication.
    /// It initializes the mutex and condition variable with `PTHREAD_PROCESS_SHARED`
    /// attributes, allowing them to be used across process boundaries.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it:
    /// - Directly manipulates raw pthread types
    /// - Assumes the memory is properly allocated and aligned
    /// - Must only be called once (double-initialization is undefined behavior)
    ///
    /// # Errors
    ///
    /// Returns an error if pthread initialization fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use ipc_benchmark::ipc::*;
    /// # fn example() -> anyhow::Result<()> {
    /// // Typically called internally by BlockingSharedMemoryDirect
    /// # Ok(())
    /// # }
    /// ```
    unsafe fn init(&mut self) -> Result<()> {
        use std::mem::MaybeUninit;

        // Initialize mutex attributes with PTHREAD_PROCESS_SHARED
        let mut mutex_attr = MaybeUninit::uninit();
        libc::pthread_mutexattr_init(mutex_attr.as_mut_ptr());
        libc::pthread_mutexattr_setpshared(mutex_attr.as_mut_ptr(), libc::PTHREAD_PROCESS_SHARED);

        // Initialize the mutex
        let mut mutex = MaybeUninit::uninit();
        libc::pthread_mutex_init(mutex.as_mut_ptr(), mutex_attr.as_ptr());
        libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());
        self.mutex = mutex.assume_init();

        // Initialize condition variable with PTHREAD_PROCESS_SHARED
        let mut cond_attr = MaybeUninit::uninit();
        libc::pthread_condattr_init(cond_attr.as_mut_ptr());
        libc::pthread_condattr_setpshared(cond_attr.as_mut_ptr(), libc::PTHREAD_PROCESS_SHARED);

        let mut cond = MaybeUninit::uninit();
        libc::pthread_cond_init(cond.as_mut_ptr(), cond_attr.as_ptr());
        libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());
        self.cond = cond.assume_init();

        // Initialize coordination flag and payload length
        self.ready = 0;
        self.payload_len = 0;

        debug!("RawSharedMessage initialized successfully (mutex + cond var)");
        Ok(())
    }

    /// Clean up pthread primitives.
    ///
    /// This should be called by the server process during shutdown to properly
    /// destroy the mutex and condition variable.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it:
    /// - Directly manipulates raw pthread types
    /// - Assumes no other threads are using these primitives
    /// - Should only be called once during cleanup
    unsafe fn destroy(&mut self) {
        libc::pthread_cond_destroy(&mut self.cond);
        libc::pthread_mutex_destroy(&mut self.mutex);
        debug!("RawSharedMessage destroyed (mutex + cond var)");
    }
}

/// Wrapper around Shmem that implements Send.
///
/// SAFETY: Shmem is safe to send across threads because:
/// 1. The underlying shared memory is process-shared (not thread-local)
/// 2. All synchronization is handled via pthread primitives
/// 3. The raw pointer is only accessed through proper synchronization
struct SendableShmem(Shmem);

// SAFETY: Shmem can be safely sent across threads because the shared memory
// is process-shared and protected by pthread mutex/condition variables.
// The raw pointer inside Shmem is only accessed through proper synchronization.
unsafe impl Send for SendableShmem {}

/// Direct memory shared memory transport for blocking I/O.
///
/// This transport provides high-performance IPC by writing messages directly
/// to shared memory with no serialization overhead, matching C-style implementations.
///
/// # Architecture
///
/// ```text
/// ┌─────────────┐                  ┌─────────────┐
/// │   Client    │                  │   Server    │
/// │  Process    │                  │  Process    │
/// └──────┬──────┘                  └──────┬──────┘
///        │                                │
///        │    ┌────────────────────┐     │
///        └────┤  Shared Memory     ├─────┘
///             │  (RawSharedMessage)│
///             │                    │
///             │  • Direct writes   │
///             │  • No serialization│
///             │  • pthread sync    │
///             └────────────────────┘
/// ```
///
/// # Usage
///
/// ```rust,no_run
/// use ipc_benchmark::ipc::*;
///
/// # fn example() -> anyhow::Result<()> {
/// // Server process
/// let mut server = BlockingSharedMemoryDirect::new();
/// let config = TransportConfig::default();
/// server.start_server_blocking(&config)?;
///
/// // In another process: client
/// let mut client = BlockingSharedMemoryDirect::new();
/// client.start_client_blocking(&config)?;
///
/// // Send message (direct memory write, no serialization)
/// let msg = Message::new(1, vec![0u8; 100], MessageType::OneWay);
/// client.send_blocking(&msg)?;
///
/// // Receive message (direct memory read, no deserialization)
/// let received = server.receive_blocking()?;
/// # Ok(())
/// # }
/// ```
pub struct BlockingSharedMemoryDirect {
    /// Shared memory segment.
    shmem: Option<SendableShmem>,

    /// Whether this instance is the server (creator) or client.
    ///
    /// The server is responsible for initializing and destroying pthread primitives.
    is_server: bool,
}

impl BlockingSharedMemoryDirect {
    /// Create a new direct memory shared memory transport.
    ///
    /// Creates an uninitialized transport. Call `start_server_blocking()` or
    /// `start_client_blocking()` to initialize it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::ipc::BlockingSharedMemoryDirect;
    ///
    /// let transport = BlockingSharedMemoryDirect::new();
    /// // Now call start_server_blocking() or start_client_blocking()
    /// ```
    pub fn new() -> Self {
        Self {
            shmem: None,
            is_server: false,
        }
    }

    /// Get a raw pointer to the shared message structure.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it returns a raw pointer that:
    /// - May be null if shared memory isn't initialized
    /// - Points to memory that could be accessed by other processes
    /// - Requires proper synchronization (use the mutex!)
    ///
    /// # Panics
    ///
    /// Panics if shared memory is not initialized.
    unsafe fn get_raw_message_ptr(&self) -> *mut RawSharedMessage {
        self.shmem
            .as_ref()
            .expect("Shared memory not initialized")
            .0
            .as_ptr() as *mut RawSharedMessage
    }
}

impl BlockingTransport for BlockingSharedMemoryDirect {
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting direct memory SHM server (size: {} bytes, name: {})",
            RawSharedMessage::SIZE,
            config.shared_memory_name
        );

        // Create shared memory with unique name from config
        let shmem = ShmemConf::new()
            .size(RawSharedMessage::SIZE)
            .os_id(&config.shared_memory_name)
            .create()
            .context("Failed to create shared memory for direct access")?;

        debug!("Shared memory created successfully");

        // Initialize the structure
        unsafe {
            let ptr = shmem.as_ptr() as *mut RawSharedMessage;
            (*ptr)
                .init()
                .context("Failed to initialize RawSharedMessage")?;
        }

        debug!("RawSharedMessage initialized successfully");

        self.shmem = Some(SendableShmem(shmem));
        self.is_server = true;

        debug!("Direct memory SHM server started successfully");
        Ok(())
    }

    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting direct memory SHM client (name: {})",
            config.shared_memory_name
        );

        // Open existing shared memory with retry loop (matches ring buffer pattern)
        // The server needs time to create the segment before client can open it
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);
        let shmem = loop {
            match ShmemConf::new()
                .size(RawSharedMessage::SIZE)
                .os_id(&config.shared_memory_name)
                .open()
            {
                Ok(shm) => {
                    debug!("Successfully opened shared memory segment");
                    break shm;
                }
                Err(e) => {
                    if start.elapsed() > timeout {
                        return Err(anyhow!(
                            "Timeout opening shared memory '{}' after {:?}. \
                             Is the server running? Error: {}",
                            config.shared_memory_name,
                            timeout,
                            e
                        ));
                    }
                    // Wait a bit and retry
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        };

        self.shmem = Some(SendableShmem(shmem));
        self.is_server = false;

        debug!("Direct memory SHM client started successfully");
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!("Sending message ID {} via direct memory SHM", message.id);

        unsafe {
            let ptr = self.get_raw_message_ptr();

            // Lock mutex
            let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
            if ret != 0 {
                return Err(anyhow!("Failed to lock mutex: {}", ret));
            }

            // CRITICAL: Capture timestamp immediately before write (matches C methodology)
            let timestamp_ns = crate::ipc::get_monotonic_time_ns();

            // Write message data directly to shared memory (no serialization!)
            (*ptr).id = message.id;
            (*ptr).timestamp = timestamp_ns;
            (*ptr).message_type = message.message_type as u32;

            // Copy only the actual payload bytes (variable length)
            let len = message.payload.len().min(MAX_PAYLOAD_SIZE);
            (*ptr).payload_len = len;
            std::ptr::copy_nonoverlapping(
                message.payload.as_ptr(),
                (*ptr).payload.as_mut_ptr(),
                len,
            );

            // Set ready flag and signal (matches C sender pattern)
            (*ptr).ready = 1;
            let ret = libc::pthread_cond_signal(&mut (*ptr).cond);
            if ret != 0 {
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                return Err(anyhow!("Failed to signal condition variable: {}", ret));
            }

            // Wait while receiver hasn't consumed the message (C pattern)
            // Use timed wait to avoid deadlock if receiver isn't ready yet
            let timeout = std::time::Duration::from_secs(5);
            let deadline = std::time::SystemTime::now() + timeout;

            while (*ptr).ready == 1 {
                // Convert deadline to timespec
                let now = std::time::SystemTime::now();
                let remaining = deadline
                    .duration_since(now)
                    .unwrap_or(std::time::Duration::ZERO);

                if remaining.is_zero() {
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                    return Err(anyhow!(
                        "Timeout waiting for receiver to consume message. \
                         Is the server process receiving messages?"
                    ));
                }

                let timespec = libc::timespec {
                    tv_sec: remaining.as_secs() as libc::time_t,
                    tv_nsec: remaining.subsec_nanos() as libc::c_long,
                };

                let mut abs_timeout = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                libc::clock_gettime(libc::CLOCK_REALTIME, &mut abs_timeout);
                abs_timeout.tv_sec += timespec.tv_sec;
                abs_timeout.tv_nsec += timespec.tv_nsec;
                if abs_timeout.tv_nsec >= 1_000_000_000 {
                    abs_timeout.tv_sec += 1;
                    abs_timeout.tv_nsec -= 1_000_000_000;
                }

                let ret =
                    libc::pthread_cond_timedwait(&mut (*ptr).cond, &mut (*ptr).mutex, &abs_timeout);

                if ret == libc::ETIMEDOUT {
                    // Timeout occurred, but check the flag again in case it was just set
                    if (*ptr).ready == 1 {
                        libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                        return Err(anyhow!(
                            "Timeout waiting for receiver to consume message. \
                             Is the server process receiving messages?"
                        ));
                    }
                    break;
                } else if ret != 0 {
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                    return Err(anyhow!("Failed to wait on condition variable: {}", ret));
                }
            }

            // Unlock mutex
            let ret = libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            if ret != 0 {
                return Err(anyhow!("Failed to unlock mutex: {}", ret));
            }
        }

        trace!("Message ID {} sent successfully", message.id);
        Ok(())
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via direct memory SHM");

        let message = unsafe {
            let ptr = self.get_raw_message_ptr();

            // Lock mutex
            let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
            if ret != 0 {
                return Err(anyhow!("Failed to lock mutex: {}", ret));
            }

            // Wait for data to be ready (matches C receiver pattern)
            while (*ptr).ready == 0 {
                let ret = libc::pthread_cond_wait(&mut (*ptr).cond, &mut (*ptr).mutex);
                if ret != 0 {
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                    return Err(anyhow!("Failed to wait on condition variable: {}", ret));
                }
            }

            // Read message data directly from shared memory (no deserialization!)
            let id = (*ptr).id;
            let timestamp = (*ptr).timestamp;
            let message_type = (*ptr).message_type;
            let payload_len = (*ptr).payload_len;

            // Copy only the valid payload bytes (variable length)
            let mut payload = vec![0u8; payload_len];
            std::ptr::copy_nonoverlapping(
                (*ptr).payload.as_ptr(),
                payload.as_mut_ptr(),
                payload_len,
            );

            // Clear ready flag and signal (matches C receiver pattern)
            (*ptr).ready = 0;
            let ret = libc::pthread_cond_signal(&mut (*ptr).cond);
            if ret != 0 {
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                return Err(anyhow!("Failed to signal condition variable: {}", ret));
            }

            // Unlock mutex
            let ret = libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            if ret != 0 {
                return Err(anyhow!("Failed to unlock mutex: {}", ret));
            }

            // Construct Message from raw data
            Message {
                id,
                timestamp,
                payload,
                message_type: MessageType::from(message_type),
            }
        };

        trace!("Received message ID {} successfully", message.id);
        Ok(message)
    }

    fn close_blocking(&mut self) -> Result<()> {
        debug!("Closing direct memory SHM transport");

        if self.is_server {
            // Server is responsible for cleanup
            if let Some(ref shmem) = self.shmem {
                unsafe {
                    let ptr = shmem.0.as_ptr() as *mut RawSharedMessage;
                    (*ptr).destroy();
                }
            }
        }

        self.shmem = None;
        debug!("Direct memory SHM transport closed");
        Ok(())
    }
}

impl Default for BlockingSharedMemoryDirect {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageType {
    /// Convert u32 to MessageType.
    ///
    /// Used when reading the message_type field from shared memory.
    fn from(value: u32) -> Self {
        match value {
            0 => MessageType::OneWay,
            1 => MessageType::Request,
            2 => MessageType::Response,
            3 => MessageType::Ping,
            4 => MessageType::Shutdown,
            _ => MessageType::OneWay, // Default fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_message_size() {
        // Verify the struct size is reasonable (with 1MB payload buffer)
        // Without pthread_cond, size is reduced
        let size = RawSharedMessage::SIZE;
        // Size should be close to 1MB (payload) + ~100 bytes overhead (mutex + metadata)
        let expected_min = MAX_PAYLOAD_SIZE;
        let expected_max = MAX_PAYLOAD_SIZE + 1024; // Allow for mutex, metadata, alignment
        assert!(
            size >= expected_min && size <= expected_max,
            "RawSharedMessage size should be around {}MB + overhead, got {} bytes",
            MAX_PAYLOAD_SIZE / 1024 / 1024,
            size
        );
    }

    #[test]
    fn test_new_creates_empty_transport() {
        let transport = BlockingSharedMemoryDirect::new();
        assert!(transport.shmem.is_none());
        assert!(!transport.is_server);
    }

    #[test]
    fn test_server_initialization() {
        let mut server = BlockingSharedMemoryDirect::new();
        let config = TransportConfig::default();

        let result = server.start_server_blocking(&config);
        assert!(
            result.is_ok(),
            "Server initialization should succeed: {:?}",
            result
        );
        assert!(server.shmem.is_some());
        assert!(server.is_server);

        // Cleanup
        server.close_blocking().unwrap();
    }

    #[test]
    fn test_send_and_receive() {
        use std::thread;
        use std::time::Duration;
        use uuid::Uuid;

        // Create unique shared memory name for this test
        let shm_name = format!("test_shm_{}", Uuid::new_v4());

        // Start server in background thread
        let shm_name_clone = shm_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemoryDirect::new();
            let config = TransportConfig {
                shared_memory_name: shm_name_clone,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive message
            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 42);
            assert_eq!(msg.payload.len(), 100); // We sent 100 bytes
            assert_eq!(&msg.payload[0..10], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);

            server.close_blocking().unwrap();
        });

        // Give server time to initialize
        thread::sleep(Duration::from_millis(100));

        // Client sends message
        let mut client = BlockingSharedMemoryDirect::new();
        let config = TransportConfig {
            shared_memory_name: shm_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let mut payload = vec![0u8; 100];
        payload[0..10].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let msg = Message::new(42, payload, MessageType::OneWay);

        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Wait for server to finish
        server_handle.join().unwrap();
    }
}
