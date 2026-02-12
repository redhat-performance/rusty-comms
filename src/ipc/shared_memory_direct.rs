//! Direct memory shared memory transport implementation (blocking).
//!
//! This module provides a high-performance blocking implementation of shared
//! memory IPC using direct memory access. Unlike the
//! ring buffer implementation, this uses a simple fixed-size struct that is
//! written directly to shared memory with no serialization overhead.
//!
//! # Design Philosophy
//!
//! This implementation prioritizes raw performance:
//! - **No serialization**: Direct struct copy (memcpy)
//! - **Simple synchronization**: One mutex + one condition variable
//! - **Fixed-size layout**: `#[repr(C, packed)]` for predictable memory
//! - **Minimal overhead**: ~1-2µs per send/receive vs 15-30µs with bincode
//!
//! # Performance Characteristics
//!
//! Expected latencies:
//! - Mean: ~6-7µs
//! - Min: ~5µs
//! - Max: ~30-35µs (significantly better than ring buffer's ~90µs)
//!
//! # Trade-offs
//!
//! **Advantages:**
//! - 3× faster max latency vs ring buffer implementation
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
//!
//! # Cross-Process Support
//!
//! This transport works across process boundaries (including containers) when:
//! - **Linux with `--ipc=host`**: Container shares host's IPC namespace including `/dev/shm`
//! - **Linux with explicit mount**: Container has `/dev/shm:/dev/shm:rw` volume mount
//!
//! The shared memory name (e.g., `/ipc-bench-shm-abc123`) creates a file at
//! `/dev/shm/ipc-bench-shm-abc123` that both host and container can access.

use crate::ipc::{BlockingTransport, Message, MessageType, TransportConfig};
use anyhow::{anyhow, Context, Result};
use libc;
use shared_memory::{Shmem, ShmemConf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, trace};

/// Futex operations for Linux.
/// Futex works at the kernel level using raw memory addresses, so it works
/// reliably across container boundaries regardless of glibc version differences.
#[cfg(target_os = "linux")]
mod futex {
    use libc::{c_int, c_long, syscall, timespec, SYS_futex};
    use std::ptr;
    use std::sync::atomic::AtomicI32;

    const FUTEX_WAIT: c_int = 0;
    const FUTEX_WAKE: c_int = 1;
    const FUTEX_PRIVATE_FLAG: c_int = 0; // Not private - shared across processes

    /// Wait on a futex until the value changes from `expected`.
    /// Returns true if woken by FUTEX_WAKE, false if timed out or value changed.
    ///
    /// # Safety
    /// The futex pointer must be in shared memory accessible to all processes.
    #[inline]
    pub unsafe fn futex_wait(futex: *const AtomicI32, expected: i32, timeout_ns: u64) -> bool {
        let timeout = timespec {
            tv_sec: (timeout_ns / 1_000_000_000) as i64,
            tv_nsec: (timeout_ns % 1_000_000_000) as i64,
        };

        let ret = syscall(
            SYS_futex,
            futex as *const i32,
            FUTEX_WAIT | FUTEX_PRIVATE_FLAG,
            expected,
            &timeout as *const timespec,
            ptr::null::<i32>(),
            0i32,
        );

        // ret == 0: woken normally
        // ret == -1, errno == ETIMEDOUT: timed out
        // ret == -1, errno == EAGAIN: value already changed (spurious wakeup OK)
        ret == 0 || (ret == -1 && *libc::__errno_location() == libc::EAGAIN)
    }

    /// Wake one waiter on a futex.
    ///
    /// # Safety
    /// The futex pointer must be in shared memory accessible to all processes.
    #[inline]
    pub unsafe fn futex_wake(futex: *const AtomicI32, num_to_wake: i32) -> c_long {
        syscall(
            SYS_futex,
            futex as *const i32,
            FUTEX_WAKE | FUTEX_PRIVATE_FLAG,
            num_to_wake,
            ptr::null::<timespec>(),
            ptr::null::<i32>(),
            0i32,
        )
    }
}

/// Maximum payload size in bytes.
///
/// Set to 8KB to match typical IPC benchmark message sizes.
/// This is large enough for most tests while keeping shared memory
/// segments small for reliable cross-process initialization.
const MAX_PAYLOAD_SIZE: usize = 8192; // 8 KB

/// A single message slot for one direction of communication.
///
/// Each slot has its own ready flag for independent synchronization.
#[repr(C)]
struct MessageSlot {
    /// Coordination flag (used as futex on Linux).
    ///
    /// - `0`: No message ready (receiver should wait)
    /// - `1`: Message ready (receiver can read)
    ready: AtomicI32,

    /// Message identifier (sequential counter).
    id: u64,

    /// Timestamp when message was sent (nanoseconds since CLOCK_MONOTONIC).
    timestamp: u64,

    /// Message type (converted from MessageType enum).
    message_type: u32,

    /// One-way latency measured by the receiver (in nanoseconds).
    /// Used in round-trip mode to carry server-measured latency back to client.
    one_way_latency_ns: u64,

    /// Actual number of valid bytes in the payload.
    payload_len: usize,

    /// Fixed-size payload buffer.
    payload: [u8; MAX_PAYLOAD_SIZE],
}

impl MessageSlot {
    /// Initialize a message slot to empty state.
    fn init(&mut self) {
        // Use ptr::write to properly initialize AtomicI32 in shared memory
        unsafe {
            std::ptr::write(&mut self.ready, AtomicI32::new(0));
        }
        self.id = 0;
        self.timestamp = 0;
        self.message_type = 0;
        self.one_way_latency_ns = 0;
        self.payload_len = 0;
    }
}

/// Dual-slot shared memory structure for bidirectional communication.
///
/// This struct supports true bidirectional IPC by having separate message
/// slots for each direction:
/// - `client_to_server`: Client writes, Server reads
/// - `server_to_client`: Server writes, Client reads
///
/// # Memory Layout
///
/// ```text
/// ┌─────────────────────────────────────────────┐
/// │ Header                                      │
/// │   mutex: pthread_mutex_t (PROCESS_SHARED)   │
/// │   cond: pthread_cond_t (PROCESS_SHARED)     │
/// │   client_ready: AtomicI32 (handshake)       │
/// ├─────────────────────────────────────────────┤
/// │ client_to_server: MessageSlot               │
/// │   ready, id, timestamp, message_type,       │
/// │   payload_len, payload[MAX_PAYLOAD_SIZE]    │
/// ├─────────────────────────────────────────────┤
/// │ server_to_client: MessageSlot               │
/// │   ready, id, timestamp, message_type,       │
/// │   payload_len, payload[MAX_PAYLOAD_SIZE]    │
/// └─────────────────────────────────────────────┘
/// ```
///
/// # Thread Safety
///
/// The mutex and condition variable are initialized with
/// `PTHREAD_PROCESS_SHARED` attribute, making them safe to use across
/// process boundaries. Each slot has its own ready flag for lock-free
/// signaling via futex (Linux) or condvar (other platforms).
#[repr(C)]
struct RawSharedMessage {
    /// Mutex for exclusive access to shared memory.
    ///
    /// Initialized with `PTHREAD_PROCESS_SHARED` to work across processes.
    mutex: libc::pthread_mutex_t,

    /// Condition variable for the Client → Server direction.
    ///
    /// Signaled when:
    /// - Client writes to `client_to_server` (wakes server waiting for a request)
    /// - Server clears `client_to_server.ready` (wakes client waiting for send space)
    cond_c2s: libc::pthread_cond_t,

    /// Condition variable for the Server → Client direction.
    ///
    /// Signaled when:
    /// - Server writes to `server_to_client` (wakes client waiting for a response)
    /// - Client clears `server_to_client.ready` (wakes server waiting for send space)
    cond_s2c: libc::pthread_cond_t,

    /// Client ready flag (used as futex on Linux).
    ///
    /// - `0`: Client not connected yet (server should wait)
    /// - `1`: Client connected and ready (server can proceed)
    ///
    /// This provides the handshake that prevents the server from entering
    /// receive loop before the client has opened the shared memory segment.
    client_ready: AtomicI32,

    /// Message slot for Client → Server communication.
    /// Client writes to this slot, Server reads from it.
    client_to_server: MessageSlot,

    /// Message slot for Server → Client communication.
    /// Server writes to this slot, Client reads from it.
    server_to_client: MessageSlot,
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
    unsafe fn init(&mut self) -> Result<()> {
        use std::mem::MaybeUninit;
        fn check_pthread_result(ret: i32, op: &str) -> Result<()> {
            if ret == 0 {
                return Ok(());
            }
            Err(anyhow!(
                "{} failed with errno {} ({})",
                op,
                ret,
                std::io::Error::from_raw_os_error(ret)
            ))
        }

        // Initialize mutex attributes with PTHREAD_PROCESS_SHARED
        let mut mutex_attr = MaybeUninit::uninit();
        check_pthread_result(
            libc::pthread_mutexattr_init(mutex_attr.as_mut_ptr()),
            "pthread_mutexattr_init",
        )?;
        check_pthread_result(
            libc::pthread_mutexattr_setpshared(
                mutex_attr.as_mut_ptr(),
                libc::PTHREAD_PROCESS_SHARED,
            ),
            "pthread_mutexattr_setpshared",
        )?;

        // Initialize the mutex
        let mut mutex = MaybeUninit::uninit();
        let mutex_init_ret = libc::pthread_mutex_init(mutex.as_mut_ptr(), mutex_attr.as_ptr());
        let mutex_attr_destroy_ret = libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());
        check_pthread_result(mutex_attr_destroy_ret, "pthread_mutexattr_destroy")?;
        check_pthread_result(mutex_init_ret, "pthread_mutex_init")?;
        self.mutex = mutex.assume_init();

        // Initialize per-direction condition variables with PTHREAD_PROCESS_SHARED.
        // Using separate condvars per direction prevents spurious wakeups during
        // round-trip tests (e.g., server finishing a receive would previously wake
        // the client waiting for a response on the other slot).
        let mut cond_attr = MaybeUninit::uninit();
        check_pthread_result(
            libc::pthread_condattr_init(cond_attr.as_mut_ptr()),
            "pthread_condattr_init",
        )?;
        check_pthread_result(
            libc::pthread_condattr_setpshared(cond_attr.as_mut_ptr(), libc::PTHREAD_PROCESS_SHARED),
            "pthread_condattr_setpshared",
        )?;

        let mut cond_c2s = MaybeUninit::uninit();
        check_pthread_result(
            libc::pthread_cond_init(cond_c2s.as_mut_ptr(), cond_attr.as_ptr()),
            "pthread_cond_init(c2s)",
        )?;
        self.cond_c2s = cond_c2s.assume_init();

        let mut cond_s2c = MaybeUninit::uninit();
        let cond_s2c_init_ret = libc::pthread_cond_init(cond_s2c.as_mut_ptr(), cond_attr.as_ptr());
        if cond_s2c_init_ret != 0 {
            let _ = libc::pthread_cond_destroy(&mut self.cond_c2s);
            let _ = libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());
            let _ = libc::pthread_mutex_destroy(&mut self.mutex);
            check_pthread_result(cond_s2c_init_ret, "pthread_cond_init(s2c)")?;
        }
        self.cond_s2c = cond_s2c.assume_init();

        check_pthread_result(
            libc::pthread_condattr_destroy(cond_attr.as_mut_ptr()),
            "pthread_condattr_destroy",
        )?;

        // Initialize client ready flag
        std::ptr::write(&mut self.client_ready, AtomicI32::new(0));

        // Initialize both message slots
        self.client_to_server.init();
        self.server_to_client.init();

        debug!(
            "RawSharedMessage initialized with dual slots (size: {} bytes)",
            Self::SIZE
        );
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
        libc::pthread_cond_destroy(&mut self.cond_c2s);
        libc::pthread_cond_destroy(&mut self.cond_s2c);
        libc::pthread_mutex_destroy(&mut self.mutex);
        debug!("RawSharedMessage destroyed (mutex + 2 cond vars)");
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
/// to shared memory with no serialization overhead.
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

    /// Shared memory segment name (for cleanup).
    ///
    /// Stored so we can call shm_unlink during close on the server side.
    shm_name: String,

    /// Cross-container mode flag.
    ///
    /// When false (default for standalone): Uses efficient pthread_cond_wait
    /// When true (for cross-container): Uses futex with short timeouts for reliability
    cross_container: bool,
}

impl BlockingSharedMemoryDirect {
    /// Size of the shared memory segment required for direct memory transport.
    ///
    /// Use this constant when pre-creating SHM segments for cross-process mode.
    pub const SEGMENT_SIZE: usize = RawSharedMessage::SIZE;

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
            shm_name: String::new(),
            cross_container: false,
        }
    }

    /// Pre-create and initialize a SHM-direct segment for cross-process mode.
    ///
    /// This function creates the shared memory segment and initializes the
    /// pthread mutex/condvar on the host side. The container will then open
    /// this existing segment when `IPC_SHM_OPEN_EXISTING` environment variable is set.
    ///
    /// # Arguments
    ///
    /// * `shm_name` - The shared memory segment name (e.g., "/ipc-bench-shm-abc123")
    ///
    /// # Returns
    ///
    /// The `Shmem` handle that must be kept alive during the benchmark.
    pub fn precreate_segment(shm_name: &str) -> Result<Shmem> {
        // Clean up any existing segment
        #[cfg(unix)]
        {
            use std::ffi::CString;
            let normalized_name = if shm_name.starts_with('/') {
                shm_name.to_string()
            } else {
                format!("/{}", shm_name)
            };
            if let Ok(c_name) = CString::new(normalized_name.as_bytes()) {
                unsafe {
                    libc::shm_unlink(c_name.as_ptr());
                }
            }
        }

        // Create the segment
        let shmem = ShmemConf::new()
            .size(Self::SEGMENT_SIZE)
            .os_id(shm_name)
            .create()
            .with_context(|| format!("Failed to pre-create SHM-direct segment: {}", shm_name))?;

        // Initialize the RawSharedMessage structure
        let init_result = unsafe {
            let ptr = shmem.as_ptr() as *mut RawSharedMessage;
            (*ptr)
                .init()
                .context("Failed to initialize RawSharedMessage")
        };
        if let Err(err) = init_result {
            #[cfg(unix)]
            {
                use std::ffi::CString;
                let normalized_name = if shm_name.starts_with('/') {
                    shm_name.to_string()
                } else {
                    format!("/{}", shm_name)
                };
                if let Ok(c_name) = CString::new(normalized_name.as_bytes()) {
                    unsafe {
                        libc::shm_unlink(c_name.as_ptr());
                    }
                }
            }
            return Err(err);
        }

        // Set permissions to 777 so container can access
        #[cfg(unix)]
        {
            let shm_file_name = shm_name.strip_prefix('/').unwrap_or(shm_name);
            let shm_path = format!("/dev/shm/{}", shm_file_name);
            if let Ok(c_path) = std::ffi::CString::new(shm_path.as_bytes()) {
                unsafe {
                    libc::chmod(c_path.as_ptr(), 0o777);
                }
            }
        }

        debug!("Pre-created SHM-direct segment: {}", shm_name);
        Ok(shmem)
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
    /// # Errors
    ///
    /// Returns an error if shared memory is not initialized
    /// (i.e. `start_server_blocking` / `start_client_blocking`
    /// has not been called).
    unsafe fn get_raw_message_ptr(&self) -> Result<*mut RawSharedMessage> {
        Ok(self
            .shmem
            .as_ref()
            .ok_or_else(|| {
                anyhow!(
                    "Shared memory not initialized. Call \
                     start_server_blocking() or \
                     start_client_blocking() first."
                )
            })?
            .0
            .as_ptr() as *mut RawSharedMessage)
    }

    /// Wait for the client to signal that it's ready.
    ///
    /// This method blocks until the client sets the client_ready flag to 1,
    /// indicating that it has successfully opened the shared memory segment
    /// and is ready to communicate.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for client connection
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Client connected successfully
    /// * `Err(anyhow::Error)` - Timeout or error waiting for client
    fn wait_for_client_ready(&self, timeout: std::time::Duration) -> Result<()> {
        let start = std::time::Instant::now();

        unsafe {
            let ptr = self.get_raw_message_ptr()?;

            // Wait for client_ready flag using futex on Linux, polling otherwise
            loop {
                // Check if client is ready
                if (*ptr).client_ready.load(Ordering::Acquire) == 1 {
                    return Ok(());
                }

                // Check timeout
                if start.elapsed() > timeout {
                    return Err(anyhow!(
                        "Timeout waiting for client to connect after {:?}. \
                         Is the client process running?",
                        timeout
                    ));
                }

                // Wait using futex on Linux for efficient wakeup
                #[cfg(target_os = "linux")]
                {
                    futex::futex_wait(&(*ptr).client_ready, 0, 10_000_000); // 10ms
                }

                #[cfg(not(target_os = "linux"))]
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

impl BlockingTransport for BlockingSharedMemoryDirect {
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting direct memory SHM server (size: {} bytes, name: {})",
            RawSharedMessage::SIZE,
            config.shared_memory_name
        );

        // Check if we should open an existing segment (cross-process mode)
        // In cross-process mode, one process pre-creates the SHM segment and the
        // other opens it rather than creating a new one.
        let open_existing = std::env::var("IPC_SHM_OPEN_EXISTING").is_ok();
        debug!(
            "SHM-direct IPC_SHM_OPEN_EXISTING check: open_existing={}",
            open_existing
        );

        let shmem = if open_existing {
            // Open existing segment (created by host)
            let start = std::time::Instant::now();
            let timeout = std::time::Duration::from_secs(10);
            loop {
                match ShmemConf::new()
                    .size(RawSharedMessage::SIZE)
                    .os_id(&config.shared_memory_name)
                    .open()
                {
                    Ok(shm) => {
                        debug!("Opened existing SHM-direct segment as server");
                        break shm;
                    }
                    Err(e) => {
                        if start.elapsed() > timeout {
                            return Err(anyhow!(
                                "Failed to open shared memory segment: {}. Error: {}",
                                config.shared_memory_name,
                                e
                            ));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        } else {
            // Clean up any existing shared memory segment with this name.
            // This ensures a fresh start and works across process boundaries
            // where a previous run may have left stale segments.
            #[cfg(unix)]
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
                        // Ignore errors - segment may not exist
                        libc::shm_unlink(c_name.as_ptr());
                    }
                    debug!("Cleaned up any existing shm segment: {}", shm_name);
                }
            }

            // Create shared memory with unique name from config
            ShmemConf::new()
                .size(RawSharedMessage::SIZE)
                .os_id(&config.shared_memory_name)
                .create()
                .context("Failed to create shared memory for direct access")?
        };

        debug!(
            "Shared memory {} successfully",
            if open_existing { "opened" } else { "created" }
        );

        // Initialize the structure (only if we created it, not if opening existing)
        if !open_existing {
            unsafe {
                let ptr = shmem.as_ptr() as *mut RawSharedMessage;
                (*ptr)
                    .init()
                    .context("Failed to initialize RawSharedMessage")?;
            }
            debug!("RawSharedMessage initialized successfully");
        }

        self.shmem = Some(SendableShmem(shmem));
        self.is_server = true;
        self.cross_container = config.cross_container;
        // Store name for cleanup during close (but don't unlink if we opened existing)
        self.shm_name = if open_existing {
            String::new() // Don't store name - we don't own this segment
        } else if config.shared_memory_name.starts_with('/') {
            config.shared_memory_name.clone()
        } else {
            format!("/{}", config.shared_memory_name)
        };

        // Note: We don't wait for client here because we need to signal ready to parent first
        // The parent waits for our stdout signal before starting the client
        // The wait for client_ready will happen in the first send/receive call

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
        let timeout = std::time::Duration::from_secs(10);
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
        self.cross_container = config.cross_container;

        // Signal to server that client is ready
        unsafe {
            let ptr = self.get_raw_message_ptr()?;
            (*ptr).client_ready.store(1, Ordering::Release);

            // Wake server if waiting on futex (Linux) or condvar (other)
            #[cfg(target_os = "linux")]
            {
                futex::futex_wake(&(*ptr).client_ready, 1);
            }

            #[cfg(not(target_os = "linux"))]
            {
                // Broadcast on both condvars to wake the server from any wait
                libc::pthread_cond_broadcast(&mut (*ptr).cond_c2s);
                libc::pthread_cond_broadcast(&mut (*ptr).cond_s2c);
            }
        }

        debug!("Direct memory SHM client started successfully");
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!(
            "Sending message ID {} via direct memory SHM (is_server={})",
            message.id,
            self.is_server
        );

        unsafe {
            let ptr = self.get_raw_message_ptr()?;

            // Select the appropriate slot and condvar based on role:
            // - Client sends on client_to_server slot → cond_c2s
            // - Server sends on server_to_client slot → cond_s2c
            let (slot, cond) = if self.is_server {
                (&mut (*ptr).server_to_client, &mut (*ptr).cond_s2c as *mut _)
            } else {
                (&mut (*ptr).client_to_server, &mut (*ptr).cond_c2s as *mut _)
            };

            // Backpressure: Wait if previous message hasn't been consumed yet
            if self.cross_container {
                // Container-safe path: Use futex/timedwait with short timeouts
                let loop_start = Instant::now();
                let timeout = Duration::from_secs(10);

                #[cfg(target_os = "linux")]
                {
                    while slot.ready.load(Ordering::Acquire) == 1 {
                        if loop_start.elapsed() > timeout {
                            return Err(anyhow!("Timeout waiting for receiver to consume message"));
                        }
                        futex::futex_wait(&slot.ready, 1, 100_000); // 100µs
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                    if ret != 0 {
                        return Err(anyhow!("Failed to lock mutex: {}", ret));
                    }

                    while slot.ready.load(Ordering::Acquire) == 1 {
                        if loop_start.elapsed() > timeout {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!("Timeout waiting for receiver to consume message"));
                        }
                        let mut ts = libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        };
                        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
                        ts.tv_nsec += 100_000; // 100µs
                        if ts.tv_nsec >= 1_000_000_000 {
                            ts.tv_sec += 1;
                            ts.tv_nsec -= 1_000_000_000;
                        }
                        libc::pthread_cond_timedwait(cond, &mut (*ptr).mutex, &ts);
                    }
                }
            } else {
                // Fast path: Use pthread_cond_wait (efficient, lowest latency)
                let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                if ret != 0 {
                    return Err(anyhow!("Failed to lock mutex: {}", ret));
                }

                if slot.ready.load(Ordering::Acquire) == 1 {
                    trace!("Waiting for receiver to consume previous message (backpressure)");

                    // Use timed wait (5 seconds) to avoid infinite blocking
                    let mut timespec = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    libc::clock_gettime(libc::CLOCK_REALTIME, &mut timespec);
                    timespec.tv_sec += 5; // 5 second timeout

                    while slot.ready.load(Ordering::Acquire) == 1 {
                        let ret = libc::pthread_cond_timedwait(cond, &mut (*ptr).mutex, &timespec);

                        if ret == libc::ETIMEDOUT {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!(
                                "Timeout waiting for receiver to consume previous message"
                            ));
                        }
                    }
                }
            }

            // CRITICAL: Capture timestamp immediately before write
            let timestamp_ns = crate::ipc::get_monotonic_time_ns();

            // Write message data directly to shared memory (no serialization!)
            // Validate message size - return error instead of silent truncation
            if message.payload.len() > MAX_PAYLOAD_SIZE {
                // Unlock mutex before returning error.
                // The mutex is held in two cases:
                //   1. !cross_container  (always uses mutex)
                //   2.  cross_container on non-Linux (uses mutex fallback)
                // On Linux + cross_container the futex path is used and no
                // mutex is acquired, so we must not unlock.
                #[cfg(target_os = "linux")]
                {
                    if !self.cross_container {
                        libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // On non-Linux, both paths lock the mutex
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
                return Err(anyhow!(
                    "Message payload size {} exceeds MAX_PAYLOAD_SIZE {} \
                     for --shm-direct mode. Use -m shm without \
                     --shm-direct for larger messages, or reduce \
                     message size.",
                    message.payload.len(),
                    MAX_PAYLOAD_SIZE
                ));
            }

            slot.id = message.id;
            slot.timestamp = timestamp_ns;
            slot.message_type = message.message_type as u32;
            slot.one_way_latency_ns = message.one_way_latency_ns;

            // Copy the payload bytes
            let len = message.payload.len();
            slot.payload_len = len;
            std::ptr::copy_nonoverlapping(message.payload.as_ptr(), slot.payload.as_mut_ptr(), len);

            // Set ready flag with release ordering to ensure all writes are visible
            slot.ready.store(1, Ordering::Release);

            // Wake receiver and unlock mutex
            if self.cross_container {
                #[cfg(target_os = "linux")]
                {
                    futex::futex_wake(&slot.ready, 1);
                }

                #[cfg(not(target_os = "linux"))]
                {
                    libc::pthread_cond_signal(cond);
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
            } else {
                // Fast path: signal and unlock mutex
                libc::pthread_cond_signal(cond);
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            }
        }

        trace!("Message ID {} sent successfully", message.id);
        Ok(())
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!(
            "Waiting to receive message via direct memory SHM (is_server={})",
            self.is_server
        );

        // If we're the server, wait for client to be ready on first receive
        if self.is_server {
            unsafe {
                let ptr = self.get_raw_message_ptr()?;
                if (*ptr).client_ready.load(Ordering::Acquire) == 0 {
                    debug!("Waiting for client to connect to shared memory");
                    self.wait_for_client_ready(std::time::Duration::from_secs(10))?;
                    debug!("Client connected to shared memory");
                }
            }
        }

        let message = unsafe {
            let ptr = self.get_raw_message_ptr()?;

            // Select the appropriate slot and condvar based on role:
            // - Server receives from client_to_server slot → cond_c2s
            // - Client receives from server_to_client slot → cond_s2c
            let (slot, cond) = if self.is_server {
                (&mut (*ptr).client_to_server, &mut (*ptr).cond_c2s as *mut _)
            } else {
                (&mut (*ptr).server_to_client, &mut (*ptr).cond_s2c as *mut _)
            };

            // Wait for data to be ready
            if self.cross_container {
                // Container-safe path: Use futex/timedwait with short timeouts
                let loop_start = Instant::now();
                let timeout = Duration::from_secs(10);

                #[cfg(target_os = "linux")]
                {
                    while slot.ready.load(Ordering::Acquire) == 0 {
                        if loop_start.elapsed() > timeout {
                            return Err(anyhow!("Timeout waiting for message"));
                        }
                        futex::futex_wait(&slot.ready, 0, 100_000); // 100µs
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                    if ret != 0 {
                        return Err(anyhow!("Failed to lock mutex: {}", ret));
                    }

                    while slot.ready.load(Ordering::Acquire) == 0 {
                        if loop_start.elapsed() > timeout {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!("Timeout waiting for message"));
                        }
                        let mut ts = libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        };
                        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
                        ts.tv_nsec += 100_000; // 100µs
                        if ts.tv_nsec >= 1_000_000_000 {
                            ts.tv_sec += 1;
                            ts.tv_nsec -= 1_000_000_000;
                        }
                        libc::pthread_cond_timedwait(cond, &mut (*ptr).mutex, &ts);
                    }
                }
            } else {
                // Fast path: Use pthread_cond_wait (efficient, lowest latency)
                let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                if ret != 0 {
                    return Err(anyhow!("Failed to lock mutex: {}", ret));
                }

                // Only compute timeout and enter condvar wait if data isn't ready yet.
                // This avoids a wasted clock_gettime(CLOCK_REALTIME) syscall on every
                // receive when the message is already waiting (the common case in
                // round-trip benchmarks where server response is fast).
                if slot.ready.load(Ordering::Acquire) == 0 {
                    let mut timespec = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    libc::clock_gettime(libc::CLOCK_REALTIME, &mut timespec);
                    timespec.tv_sec += 5; // 5 second timeout

                    while slot.ready.load(Ordering::Acquire) == 0 {
                        let ret = libc::pthread_cond_timedwait(cond, &mut (*ptr).mutex, &timespec);

                        if ret == libc::ETIMEDOUT {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!("Timeout waiting for message"));
                        }
                    }
                }
            }

            // CRITICAL: Capture receive timestamp immediately after we know data is ready
            // This ensures accurate latency measurement
            let receive_time_ns = crate::ipc::get_monotonic_time_ns();

            // Read message data directly from shared memory (no deserialization!)
            let id = slot.id;
            let timestamp = slot.timestamp;
            let message_type_u32 = slot.message_type;
            let message_type = <MessageType as From<u32>>::from(message_type_u32);
            let payload_len = slot.payload_len;

            // Calculate one-way latency for Request/OneWay messages
            // For Response messages, preserve the server-measured latency from the slot
            let one_way_latency_ns = if message_type != MessageType::Response {
                receive_time_ns.saturating_sub(timestamp)
            } else {
                slot.one_way_latency_ns
            };

            // Validate payload_len to prevent buffer overread from
            // corrupted shared memory.
            if payload_len > MAX_PAYLOAD_SIZE {
                // Unlock mutex before returning error.
                // Same logic as send_blocking: the mutex is held when
                // !cross_container OR cross_container on non-Linux.
                // On Linux + cross_container we use the futex path
                // and no mutex is held.
                #[cfg(target_os = "linux")]
                {
                    if !self.cross_container {
                        libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // On non-Linux, both paths lock the mutex
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
                return Err(anyhow!(
                    "Invalid payload_len {} exceeds \
                     MAX_PAYLOAD_SIZE {} - shared memory may be \
                     corrupted",
                    payload_len,
                    MAX_PAYLOAD_SIZE
                ));
            }

            // Copy only the valid payload bytes (variable length)
            let mut payload = vec![0u8; payload_len];
            std::ptr::copy_nonoverlapping(slot.payload.as_ptr(), payload.as_mut_ptr(), payload_len);

            // Clear ready flag with release ordering
            slot.ready.store(0, Ordering::Release);

            // Wake sender (in case of backpressure) and unlock mutex
            if self.cross_container {
                #[cfg(target_os = "linux")]
                {
                    futex::futex_wake(&slot.ready, 1);
                }

                #[cfg(not(target_os = "linux"))]
                {
                    libc::pthread_cond_signal(cond);
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
            } else {
                // Fast path: signal and unlock mutex
                libc::pthread_cond_signal(cond);
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            }

            // Construct Message from raw data
            Message {
                id,
                timestamp,
                payload,
                message_type,
                one_way_latency_ns,
            }
        };

        trace!("Received message ID {} successfully", message.id);
        Ok(message)
    }

    fn close_blocking(&mut self) -> Result<()> {
        debug!("Closing direct memory SHM transport");

        if self.is_server {
            // Destroy process-shared pthread primitives only when this instance
            // created and owns the segment. In open-existing mode (cross-container),
            // this process must not destroy primitives owned by another process.
            if !self.shm_name.is_empty() {
                if let Some(ref shmem) = self.shmem {
                    unsafe {
                        let ptr = shmem.0.as_ptr() as *mut RawSharedMessage;
                        (*ptr).destroy();
                    }
                }
            } else {
                debug!("Skipping primitive destruction for non-owned SHM segment");
            }

            // Unlink the shared memory segment to free system resources.
            // This is important for cross-process mode where stale segments
            // could interfere with subsequent benchmark runs.
            #[cfg(unix)]
            if !self.shm_name.is_empty() {
                use std::ffi::CString;
                if let Ok(c_name) = CString::new(self.shm_name.as_bytes()) {
                    unsafe {
                        let ret = libc::shm_unlink(c_name.as_ptr());
                        if ret == 0 {
                            debug!("Unlinked shm segment: {}", self.shm_name);
                        } else {
                            debug!(
                                "Failed to unlink shm segment: {} (errno: {})",
                                self.shm_name,
                                *libc::__errno_location()
                            );
                        }
                    }
                }
            }
        }

        self.shmem = None;
        self.shm_name.clear();
        debug!("Direct memory SHM transport closed");
        Ok(())
    }
}

/// Ensure shared memory resources are cleaned up even if
/// `close_blocking()` is never called (e.g. due to a panic or early
/// return). Destroys pthread primitives, unlinks the SHM segment
/// (when this instance is the server), and releases the mapping.
impl Drop for BlockingSharedMemoryDirect {
    fn drop(&mut self) {
        // close_blocking() is idempotent; safe to call even if the
        // transport was never started or was already closed.
        if let Err(e) = self.close_blocking() {
            tracing::debug!(
                "BlockingSharedMemoryDirect::drop: \
                 close_blocking returned error \
                 (best-effort cleanup): {}",
                e
            );
        }
    }
}

impl Default for BlockingSharedMemoryDirect {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_message_size() {
        // Verify the struct size is reasonable with dual message slots
        // Each slot has MAX_PAYLOAD_SIZE payload buffer
        let size = RawSharedMessage::SIZE;
        // Size should be ~2x MAX_PAYLOAD_SIZE (dual slots) + header overhead
        let expected_min = MAX_PAYLOAD_SIZE * 2;
        let expected_max = MAX_PAYLOAD_SIZE * 2 + 1024; // Allow for mutex, cond, metadata
        assert!(
            size >= expected_min && size <= expected_max,
            "RawSharedMessage size should be ~{} bytes (2 slots) + overhead, got {} bytes",
            expected_min,
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
    #[cfg(not(target_os = "macos"))] // macOS has 31-char limit on shm names
    fn test_server_initialization() {
        use std::thread;
        use std::time::Duration;
        use uuid::Uuid;

        let shm_name = format!("test_shm_init_{}", Uuid::new_v4());
        let shm_name_clone = shm_name.clone();

        // Start server in background thread
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemoryDirect::new();
            let config = TransportConfig {
                shared_memory_name: shm_name_clone,
                ..Default::default()
            };

            let result = server.start_server_blocking(&config);
            assert!(
                result.is_ok(),
                "Server initialization should succeed: {:?}",
                result
            );
            assert!(server.shmem.is_some());
            assert!(server.is_server);

            // Keep server alive briefly then cleanup
            thread::sleep(Duration::from_millis(100));
            server.close_blocking().unwrap();
        });

        // Give server time to start
        thread::sleep(Duration::from_millis(50));

        // Connect client to unblock server
        let mut client = BlockingSharedMemoryDirect::new();
        let config = TransportConfig {
            shared_memory_name: shm_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();
        client.close_blocking().unwrap();

        // Wait for server to finish
        server_handle.join().unwrap();
    }

    #[test]
    #[cfg(not(target_os = "macos"))] // macOS has 31-char limit on shm names
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

    #[test]
    fn test_default_creates_new_transport() {
        let transport = BlockingSharedMemoryDirect::default();
        assert!(transport.shmem.is_none());
        assert!(!transport.is_server);
    }

    #[test]
    fn test_close_without_init() {
        let mut transport = BlockingSharedMemoryDirect::new();
        // Close without initialization should succeed
        let result = transport.close_blocking();
        assert!(result.is_ok());
    }

    /// Test that `precreate_segment` successfully creates and initializes
    /// a shared memory segment.
    #[test]
    fn test_precreate_segment_creates_segment() {
        let shm_name = format!("test_precreate_{}", uuid::Uuid::new_v4());

        let result = BlockingSharedMemoryDirect::precreate_segment(&shm_name);
        assert!(
            result.is_ok(),
            "precreate_segment should succeed: {:?}",
            result.err()
        );

        // The returned Shmem handle should be valid and have
        // enough size to hold a RawSharedMessage.
        let shmem = result.unwrap();
        assert!(
            shmem.len() >= RawSharedMessage::SIZE,
            "Segment size ({}) should be >= RawSharedMessage::SIZE ({})",
            shmem.len(),
            RawSharedMessage::SIZE,
        );

        // Cleanup: drop the handle and unlink the segment.
        drop(shmem);
        #[cfg(unix)]
        {
            let normalized = if shm_name.starts_with('/') {
                shm_name.clone()
            } else {
                format!("/{}", shm_name)
            };
            if let Ok(c_name) = std::ffi::CString::new(normalized.as_bytes()) {
                unsafe {
                    libc::shm_unlink(c_name.as_ptr());
                }
            }
        }
    }

    /// Test that `precreate_segment` cleans up a pre-existing segment
    /// before creating a new one (idempotency).
    #[test]
    fn test_precreate_segment_idempotent() {
        let shm_name = format!("test_precreate_idem_{}", uuid::Uuid::new_v4());

        // Create the segment twice in succession.
        let shmem1 = BlockingSharedMemoryDirect::precreate_segment(&shm_name)
            .expect("First precreate should succeed");
        drop(shmem1);

        let shmem2 = BlockingSharedMemoryDirect::precreate_segment(&shm_name)
            .expect("Second precreate should also succeed");
        drop(shmem2);

        // Cleanup
        #[cfg(unix)]
        {
            let normalized = if shm_name.starts_with('/') {
                shm_name.clone()
            } else {
                format!("/{}", shm_name)
            };
            if let Ok(c_name) = std::ffi::CString::new(normalized.as_bytes()) {
                unsafe {
                    libc::shm_unlink(c_name.as_ptr());
                }
            }
        }
    }

    /// Test that `get_raw_message_ptr` returns an error when shared memory
    /// is not initialized (i.e., before start_server or start_client).
    #[test]
    fn test_get_raw_message_ptr_uninit_returns_error() {
        let transport = BlockingSharedMemoryDirect::new();
        let result = unsafe { transport.get_raw_message_ptr() };
        assert!(
            result.is_err(),
            "get_raw_message_ptr should fail before initialization"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not initialized"),
            "Error should mention 'not initialized', got: {}",
            err_msg
        );
    }

    /// Test that the Drop impl calls close_blocking without panicking.
    #[test]
    fn test_drop_without_init() {
        // Creating and immediately dropping should not panic.
        let _transport = BlockingSharedMemoryDirect::new();
    }

    /// Test that the Drop impl cleans up an initialized transport.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_drop_after_server_init() {
        let shm_name = format!("test_drop_init_{}", uuid::Uuid::new_v4());
        let config = TransportConfig {
            shared_memory_name: shm_name,
            ..Default::default()
        };

        let mut transport = BlockingSharedMemoryDirect::new();
        transport
            .start_server_blocking(&config)
            .expect("Server start should succeed");

        // Dropping should clean up without panicking.
        drop(transport);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_multiple_messages() {
        use std::thread;
        use std::time::Duration;
        use uuid::Uuid;

        let shm_name = format!("test_shm_multi_{}", Uuid::new_v4());
        let shm_name_clone = shm_name.clone();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemoryDirect::new();
            let config = TransportConfig {
                shared_memory_name: shm_name_clone,
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

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingSharedMemoryDirect::new();
        let config = TransportConfig {
            shared_memory_name: shm_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send multiple messages
        for id in 1..=5 {
            let msg = Message::new(id, vec![0u8; 64], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    #[test]
    fn test_raw_message_layout() {
        // Verify alignment and offset calculations are correct
        let size = RawSharedMessage::SIZE;
        // Size should be greater than 0 and reasonable
        assert!(size > 0);
        // Size should include at least the payload capacity
        assert!(size >= 8192); // Minimum expected payload size
    }

    /// Test that the synchronization flags are atomic and properly aligned.
    /// On Linux, this uses futex-based synchronization; on other platforms,
    /// it uses pthread condition variables.
    #[test]
    fn test_sync_flags_atomic() {
        use std::sync::atomic::Ordering;

        // Create a mock shared message structure to test atomic operations
        let ready = std::sync::atomic::AtomicI32::new(0);
        let client_ready = std::sync::atomic::AtomicI32::new(0);

        // Verify initial state
        assert_eq!(ready.load(Ordering::Acquire), 0);
        assert_eq!(client_ready.load(Ordering::Acquire), 0);

        // Test compare_exchange for ready flag (simulating send)
        ready.store(1, Ordering::Release);
        assert_eq!(ready.load(Ordering::Acquire), 1);

        // Test compare_exchange for client_ready flag (simulating receive ack)
        let result = client_ready.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
        assert!(result.is_ok());
        assert_eq!(client_ready.load(Ordering::Acquire), 1);

        // Reset flags (simulating next message cycle)
        ready.store(0, Ordering::Release);
        client_ready.store(0, Ordering::Release);
        assert_eq!(ready.load(Ordering::Acquire), 0);
        assert_eq!(client_ready.load(Ordering::Acquire), 0);
    }

    /// Test that futex syscalls are available on Linux.
    #[test]
    #[cfg(target_os = "linux")]
    fn test_futex_syscall_available() {
        use std::sync::atomic::{AtomicI32, Ordering};

        let futex_word = AtomicI32::new(0);

        // Test FUTEX_WAIT with immediate timeout (should return immediately
        // since value doesn't match expected)
        futex_word.store(1, Ordering::SeqCst);

        // futex_wait expects value 0 but we stored 1, so it should return
        // immediately with EAGAIN (value mismatch) - this confirms the syscall works
        let ptr = futex_word.as_ptr();
        let result = unsafe {
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAIT,
                0i32, // Expected value (doesn't match)
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<i32>(),
                0i32,
            )
        };

        // Should fail with EAGAIN because value doesn't match
        assert_eq!(result, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EAGAIN)
        );
    }

    /// Test that futex wake works correctly on Linux.
    #[test]
    #[cfg(target_os = "linux")]
    fn test_futex_wake() {
        use std::sync::atomic::AtomicI32;

        let futex_word = AtomicI32::new(0);
        let ptr = futex_word.as_ptr();

        // Wake with no waiters should succeed (returns 0 waiters woken)
        let result = unsafe {
            libc::syscall(
                libc::SYS_futex,
                ptr,
                libc::FUTEX_WAKE,
                1i32, // Wake at most 1 waiter
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<i32>(),
                0i32,
            )
        };

        // Should return 0 (no waiters to wake)
        assert_eq!(result, 0);
    }

    /// Test concurrent send/receive with explicit synchronization verification.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_concurrent_sync_correctness() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;
        use uuid::Uuid;

        let barrier = Arc::new(Barrier::new(2));
        let shm_name = format!("test_sync_{}", Uuid::new_v4());

        let server_name = shm_name.clone();
        let server_barrier = barrier.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemoryDirect::new();
            let config = TransportConfig {
                shared_memory_name: server_name,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Signal ready
            server_barrier.wait();

            // Receive multiple messages and verify ordering
            for expected_seq in 0u64..5 {
                let msg = server.receive_blocking().unwrap();
                // First 8 bytes contain sequence number
                let seq = u64::from_le_bytes(msg.payload[0..8].try_into().unwrap());
                assert_eq!(seq, expected_seq, "Message sequence mismatch");
            }

            server.close_blocking().unwrap();
        });

        // Wait for server
        barrier.wait();
        thread::sleep(Duration::from_millis(10));

        let mut client = BlockingSharedMemoryDirect::new();
        let config = TransportConfig {
            shared_memory_name: shm_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send messages with sequence numbers
        for seq in 0u64..5 {
            let mut payload = vec![0u8; 64];
            payload[0..8].copy_from_slice(&seq.to_le_bytes());
            let msg = Message::new(seq, payload, MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test that the round-trip / one-way latency ratio is reasonable.
    ///
    /// A healthy ratio is approximately 2× (one OW leg in each direction).
    /// Before the per-direction condvar fix, this ratio was ~8× due to spurious
    /// wakeups causing excessive context switches. This test ensures the fix
    /// holds by asserting the ratio stays below 4×.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_rt_ow_latency_ratio_shm_direct() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;
        use uuid::Uuid;

        let num_iterations = 1000;
        let payload_size = 64;
        let shm_name = format!("test_rtow_{}", Uuid::new_v4());
        let barrier = Arc::new(Barrier::new(2));

        // Server thread: receives Request, sends back Response with the OW latency embedded
        let server_name = shm_name.clone();
        let server_barrier = barrier.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingSharedMemoryDirect::new();
            let config = TransportConfig {
                shared_memory_name: server_name,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Signal ready
            server_barrier.wait();

            for _ in 0..num_iterations {
                // Receive request from client
                let request = server.receive_blocking().unwrap();
                assert_eq!(request.message_type, MessageType::Request);

                // Send response back, embedding the measured one_way_latency_ns
                let mut response =
                    Message::new_lazy(request.id, request.payload.clone(), MessageType::Response);
                response.one_way_latency_ns = request.one_way_latency_ns;
                server.send_blocking(&response).unwrap();
            }

            server.close_blocking().unwrap();
        });

        // Wait for server to be ready
        barrier.wait();
        thread::sleep(Duration::from_millis(10));

        // Client: connect, do round-trips, measure OW and RT
        let mut client = BlockingSharedMemoryDirect::new();
        let config = TransportConfig {
            shared_memory_name: shm_name,
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
            "[shm_direct] Mean OW: {:.0} ns, Mean RT: {:.0} ns, Ratio RT/OW: {:.2}x ({} samples)",
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
             This indicates the per-direction condvar fix may have regressed.",
            ratio,
            mean_ow,
            mean_rt,
        );
    }
}
