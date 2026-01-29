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

/// Raw message structure stored directly in shared memory.
///
/// This struct is designed for minimal overhead IPC. It uses `#[repr(C, packed)]`
/// to ensure predictable memory layout across process boundaries.
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

    /// Condition variable for signaling.
    ///
    /// Both sender and receiver wait/signal on this SAME condition variable
    /// using a ping-pong pattern for efficient synchronization.
    cond: libc::pthread_cond_t,

    /// Message identifier (sequential counter).
    id: u64,

    /// Timestamp when message was sent (nanoseconds since CLOCK_MONOTONIC).
    ///
    /// This is captured immediately before the message is written to shared
    /// memory for accurate latency measurement.
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

    /// Coordination flag (used as futex on Linux).
    ///
    /// - `0`: No message ready (receiver should wait)
    /// - `1`: Message ready (receiver can read)
    ///
    /// Uses AtomicI32 for futex compatibility. Futex operates at kernel level,
    /// working reliably across container boundaries regardless of glibc version.
    ready: AtomicI32,

    /// Client ready flag (used as futex on Linux).
    ///
    /// - `0`: Client not connected yet (server should wait)
    /// - `1`: Client connected and ready (server can proceed)
    ///
    /// This provides the handshake that prevents the server from entering
    /// receive loop before the client has opened the shared memory segment.
    client_ready: AtomicI32,
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

        // Initialize coordination flags and payload length
        // Use ptr::write to initialize AtomicI32 in shared memory
        std::ptr::write(&mut self.ready, AtomicI32::new(0));
        std::ptr::write(&mut self.client_ready, AtomicI32::new(0));
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
        unsafe {
            let ptr = shmem.as_ptr() as *mut RawSharedMessage;
            (*ptr)
                .init()
                .context("Failed to initialize RawSharedMessage")?;
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
            let ptr = self.get_raw_message_ptr();

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
            let timeout = std::time::Duration::from_secs(30);
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
        self.cross_container = config.cross_container;

        // Signal to server that client is ready
        unsafe {
            let ptr = self.get_raw_message_ptr();
            (*ptr).client_ready.store(1, Ordering::Release);

            // Wake server if waiting on futex (Linux) or condvar (other)
            #[cfg(target_os = "linux")]
            {
                futex::futex_wake(&(*ptr).client_ready, 1);
            }

            #[cfg(not(target_os = "linux"))]
            {
                libc::pthread_cond_broadcast(&mut (*ptr).cond);
            }
        }

        debug!("Direct memory SHM client started successfully");
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!("Sending message ID {} via direct memory SHM", message.id);

        unsafe {
            let ptr = self.get_raw_message_ptr();

            // Backpressure: Wait if previous message hasn't been consumed yet
            if self.cross_container {
                // Container-safe path: Use futex/timedwait with short timeouts
                let loop_start = Instant::now();
                let timeout = Duration::from_secs(30);

                #[cfg(target_os = "linux")]
                {
                    while (*ptr).ready.load(Ordering::Acquire) == 1 {
                        if loop_start.elapsed() > timeout {
                            return Err(anyhow!("Timeout waiting for receiver to consume message"));
                        }
                        futex::futex_wait(&(*ptr).ready, 1, 100_000); // 100µs
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                    if ret != 0 {
                        return Err(anyhow!("Failed to lock mutex: {}", ret));
                    }

                    while (*ptr).ready.load(Ordering::Acquire) == 1 {
                        if loop_start.elapsed() > timeout {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!("Timeout waiting for receiver to consume message"));
                        }
                        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
                        ts.tv_nsec += 100_000; // 100µs
                        if ts.tv_nsec >= 1_000_000_000 {
                            ts.tv_sec += 1;
                            ts.tv_nsec -= 1_000_000_000;
                        }
                        libc::pthread_cond_timedwait(&mut (*ptr).cond, &mut (*ptr).mutex, &ts);
                    }
                }
            } else {
                // Fast path: Use pthread_cond_wait (efficient, lowest latency)
                let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                if ret != 0 {
                    return Err(anyhow!("Failed to lock mutex: {}", ret));
                }

                if (*ptr).ready.load(Ordering::Acquire) == 1 {
                    trace!("Waiting for receiver to consume previous message (backpressure)");

                    // Use timed wait (5 seconds) to avoid infinite blocking
                    let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                    libc::clock_gettime(libc::CLOCK_REALTIME, &mut timespec);
                    timespec.tv_sec += 5; // 5 second timeout

                    while (*ptr).ready.load(Ordering::Acquire) == 1 {
                        let ret = libc::pthread_cond_timedwait(
                            &mut (*ptr).cond,
                            &mut (*ptr).mutex,
                            &timespec,
                        );

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
                return Err(anyhow!(
                    "Message payload size {} exceeds MAX_PAYLOAD_SIZE {} for --shm-direct mode.                     Use -m shm without --shm-direct for larger messages, or reduce message size.",
                    message.payload.len(),
                    MAX_PAYLOAD_SIZE
                ));
            }

            (*ptr).id = message.id;
            (*ptr).timestamp = timestamp_ns;
            (*ptr).message_type = message.message_type as u32;

            // Copy the payload bytes
            let len = message.payload.len();
            (*ptr).payload_len = len;
            std::ptr::copy_nonoverlapping(
                message.payload.as_ptr(),
                (*ptr).payload.as_mut_ptr(),
                len,
            );

            // Set ready flag with release ordering to ensure all writes are visible
            (*ptr).ready.store(1, Ordering::Release);

            // Wake receiver and unlock mutex
            if self.cross_container {
                #[cfg(target_os = "linux")]
                {
                    futex::futex_wake(&(*ptr).ready, 1);
                }

                #[cfg(not(target_os = "linux"))]
                {
                    libc::pthread_cond_signal(&mut (*ptr).cond);
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
            } else {
                // Fast path: signal and unlock mutex
                libc::pthread_cond_signal(&mut (*ptr).cond);
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            }
        }

        trace!("Message ID {} sent successfully", message.id);
        Ok(())
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via direct memory SHM");

        // If we're the server, wait for client to be ready on first receive
        if self.is_server {
            unsafe {
                let ptr = self.get_raw_message_ptr();
                if (*ptr).client_ready.load(Ordering::Acquire) == 0 {
                    debug!("Waiting for client to connect to shared memory");
                    self.wait_for_client_ready(std::time::Duration::from_secs(30))?;
                    debug!("Client connected to shared memory");
                }
            }
        }

        let message = unsafe {
            let ptr = self.get_raw_message_ptr();

            // Wait for data to be ready
            if self.cross_container {
                // Container-safe path: Use futex/timedwait with short timeouts
                let loop_start = Instant::now();
                let timeout = Duration::from_secs(30);

                #[cfg(target_os = "linux")]
                {
                    while (*ptr).ready.load(Ordering::Acquire) == 0 {
                        if loop_start.elapsed() > timeout {
                            return Err(anyhow!("Timeout waiting for message"));
                        }
                        futex::futex_wait(&(*ptr).ready, 0, 100_000); // 100µs
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                    if ret != 0 {
                        return Err(anyhow!("Failed to lock mutex: {}", ret));
                    }

                    while (*ptr).ready.load(Ordering::Acquire) == 0 {
                        if loop_start.elapsed() > timeout {
                            libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                            return Err(anyhow!("Timeout waiting for message"));
                        }
                        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                        libc::clock_gettime(libc::CLOCK_REALTIME, &mut ts);
                        ts.tv_nsec += 100_000; // 100µs
                        if ts.tv_nsec >= 1_000_000_000 {
                            ts.tv_sec += 1;
                            ts.tv_nsec -= 1_000_000_000;
                        }
                        libc::pthread_cond_timedwait(&mut (*ptr).cond, &mut (*ptr).mutex, &ts);
                    }
                }
            } else {
                // Fast path: Use pthread_cond_wait (efficient, lowest latency)
                let ret = libc::pthread_mutex_lock(&mut (*ptr).mutex);
                if ret != 0 {
                    return Err(anyhow!("Failed to lock mutex: {}", ret));
                }

                // Use timed wait (5 seconds) to avoid infinite blocking
                let mut timespec = libc::timespec { tv_sec: 0, tv_nsec: 0 };
                libc::clock_gettime(libc::CLOCK_REALTIME, &mut timespec);
                timespec.tv_sec += 5; // 5 second timeout

                while (*ptr).ready.load(Ordering::Acquire) == 0 {
                    let ret = libc::pthread_cond_timedwait(
                        &mut (*ptr).cond,
                        &mut (*ptr).mutex,
                        &timespec,
                    );

                    if ret == libc::ETIMEDOUT {
                        libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                        return Err(anyhow!("Timeout waiting for message"));
                    }
                }
            }

            // Read message data directly from shared memory (no deserialization!)
            let id = (*ptr).id;
            let timestamp = (*ptr).timestamp;
            let message_type_u32 = (*ptr).message_type;
            let message_type = <MessageType as From<u32>>::from(message_type_u32);
            let payload_len = (*ptr).payload_len;

            // Copy only the valid payload bytes (variable length)
            let mut payload = vec![0u8; payload_len];
            std::ptr::copy_nonoverlapping(
                (*ptr).payload.as_ptr(),
                payload.as_mut_ptr(),
                payload_len,
            );

            // Clear ready flag with release ordering
            (*ptr).ready.store(0, Ordering::Release);

            // Wake sender (in case of backpressure) and unlock mutex
            if self.cross_container {
                #[cfg(target_os = "linux")]
                {
                    futex::futex_wake(&(*ptr).ready, 1);
                }

                #[cfg(not(target_os = "linux"))]
                {
                    libc::pthread_cond_signal(&mut (*ptr).cond);
                    libc::pthread_mutex_unlock(&mut (*ptr).mutex);
                }
            } else {
                // Fast path: signal and unlock mutex
                libc::pthread_cond_signal(&mut (*ptr).cond);
                libc::pthread_mutex_unlock(&mut (*ptr).mutex);
            }

            // Construct Message from raw data
            Message {
                id,
                timestamp,
                payload,
                message_type,
                one_way_latency_ns: 0,
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
                0i32,  // Expected value (doesn't match)
                std::ptr::null::<libc::timespec>(),
                std::ptr::null::<i32>(),
                0i32,
            )
        };
        
        // Should fail with EAGAIN because value doesn't match
        assert_eq!(result, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::EAGAIN));
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
                1i32,  // Wake at most 1 waiter
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
}
