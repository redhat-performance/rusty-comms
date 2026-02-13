//! # POSIX Message Queue Transport Implementation
//!
//! This module implements IPC communication using POSIX Message Queues, a standardized
//! inter-process communication mechanism available on UNIX-like systems. POSIX Message
//! Queues provide a robust, kernel-mediated messaging system with built-in message
//! prioritization, atomic operations, and system-wide persistence.
//!
//! ## Key Features
//!
//! - **Kernel-Mediated**: Messages are managed by the kernel for reliability
//! - **Message Boundaries**: Preserves message boundaries (no partial reads/writes)
//! - **Priority Support**: Messages can be sent with priority levels
//! - **Atomic Operations**: Send/receive operations are atomic and thread-safe
//! - **System-Wide**: Queues persist until explicitly removed or system reboot
//! - **Non-Blocking I/O**: Supports non-blocking operations for high throughput
//! - **Bidirectional**: Dual queue architecture for reliable round-trip communication
//!
//! ## Architecture Overview
//!
//! This implementation uses a dual queue architecture for reliable bidirectional
//! communication. This prevents race conditions where clients might read their own
//! messages or servers might receive responses intended for clients:
//!
//! ```text
//! ┌─────────────┐    Request Queue     ┌─────────────┐
//! │   Client    │─────────────────────▶│   Server    │
//! │  Process    │    ({name}_req)      │  Process    │
//! │             │                      │             │
//! │             │◀─────────────────────│             │
//! └─────────────┘    Response Queue    └─────────────┘
//!                    ({name}_resp)
//! ```
//!
//! - **Request Queue** (`{name}_req`): Client sends requests, Server receives them
//! - **Response Queue** (`{name}_resp`): Server sends responses, Client receives them
//!
//! ## Performance Characteristics
//!
//! - **Latency**: Higher than shared memory due to kernel involvement
//! - **Throughput**: Limited by system message queue limits and kernel overhead
//! - **Reliability**: Very high due to kernel management and persistence
//! - **Scalability**: Limited by system-wide queue limits and memory usage
//!
//! ## System Configuration
//!
//! POSIX Message Queues are subject to system-wide limits that may need tuning:
//! - `/proc/sys/fs/mqueue/queues_max`: Maximum number of queues
//! - `/proc/sys/fs/mqueue/msg_max`: Maximum messages per queue
//! - `/proc/sys/fs/mqueue/msgsize_max`: Maximum message size
//!
//! ## Implementation Notes
//!
//! - **Queue Naming**: Uses "/" prefix for portable queue names, with `_req`/`_resp` suffixes
//! - **Creation vs. Opening**: Server creates both queues, clients open existing ones
//! - **Cleanup**: Only servers unlink queues on close
//! - **Non-Blocking**: Uses O_NONBLOCK with retry logic for throughput
//! - **Resource Management**: Proper cleanup prevents queue leaks
//!
//! ## Limitations
//!
//! - **Dual Queues**: Uses two queues per connection (counts against system queue limit)
//! - **Message Size**: Limited by system configuration (typically 8KB default)
//! - **Queue Depth**: Limited by system configuration (typically 10 messages)
//! - **Platform**: Linux only (this transport is only selectable on Linux builds)
//! - **Permissions**: Requires appropriate system permissions for queue operations

use super::{ConnectionId, IpcError, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use nix::errno::Errno;
use nix::mqueue::{mq_close, mq_open, mq_receive, mq_send, mq_unlink, MQ_OFlag, MqAttr, MqdT};
use nix::sys::stat::Mode;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Thin wrapper around a raw file descriptor that implements `AsRawFd`
/// without owning the descriptor (does not close on drop).
/// Used to register POSIX message queue FDs with tokio's `AsyncFd`.
struct RawFdWrapper(RawFd);

impl AsRawFd for RawFdWrapper {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

/// POSIX Message Queue transport implementation
///
/// This transport provides IPC communication using POSIX Message Queues, which are
/// kernel-managed message passing facilities that provide reliable, atomic message
/// transmission between processes. The implementation handles queue lifecycle,
/// non-blocking I/O, and proper resource cleanup.
///
/// ## Design Philosophy
///
/// - **Reliability First**: Leverages kernel management for maximum reliability
/// - **Resource Safety**: Careful management of queue creation and cleanup
/// - **Performance Optimization**: Non-blocking I/O with intelligent retry logic
/// - **System Integration**: Follows POSIX standards for portability
///
/// ## Dual Queue Architecture
///
/// To support reliable bidirectional (round-trip) communication, this transport
/// uses two separate queues:
///
/// ```text
/// ┌─────────────┐    Request Queue     ┌─────────────┐
/// │   Client    │─────────────────────▶│   Server    │
/// │             │                      │             │
/// │             │◀─────────────────────│             │
/// └─────────────┘    Response Queue    └─────────────┘
/// ```
///
/// - **Request Queue** (`_req`): Client sends requests, Server receives them
/// - **Response Queue** (`_resp`): Server sends responses, Client receives them
///
/// This separation prevents race conditions where the client might read its own
/// messages or the server might read responses intended for the client.
///
/// ## Queue Management
///
/// The transport distinguishes between queue creators (servers) and queue users
/// (clients). Only the creator is responsible for queue cleanup to prevent
/// premature resource deallocation.
///
/// ## Message Protocol
///
/// Messages are serialized using bincode and transmitted atomically through
/// the message queue. Message boundaries are preserved by the kernel, ensuring
/// complete messages are always delivered.
///
/// ## Performance Considerations
///
/// - **Kernel Overhead**: Each send/receive involves kernel syscalls
/// - **Memory Copying**: Messages are copied to/from kernel space
/// - **Queue Limits**: System limits affect maximum throughput and latency
/// - **Non-Blocking I/O**: Reduces blocking but requires retry logic
pub struct PosixMessageQueueTransport {
    /// Current connection state of the transport
    state: TransportState,

    /// Base POSIX message queue name (with "/" prefix)
    queue_name: String,

    /// Request queue file descriptor (Client → Server)
    /// Client writes to this, Server reads from this
    request_queue_fd: Option<MqdT>,

    /// Response queue file descriptor (Server → Client)
    /// Server writes to this, Client reads from this
    response_queue_fd: Option<MqdT>,

    /// Maximum size of individual messages in bytes
    max_msg_size: usize,

    /// Maximum number of messages that can be queued
    max_msg_count: usize,

    /// Whether this instance is the server (creates queues) or client (opens them)
    ///
    /// - Server: Creates both queues, reads from request queue, writes to response queue
    /// - Client: Opens both queues, writes to request queue, reads from response queue
    is_server: bool,

    /// Flag to ensure the backpressure warning is only logged once.
    has_warned_backpressure: bool,

    /// Transport configuration, stored after initialization.
    ///
    /// This holds a copy of the `TransportConfig` used to initialize the
    /// transport. It is `None` until `start_server` or `start_client` is
    /// called. Storing the configuration allows the transport to access
    /// parameters like `pmq_priority` during its operation.
    config: Option<TransportConfig>,

    /// AsyncFd wrapper for the send queue (request queue for client, response queue for server).
    /// Allows epoll-based async waiting instead of polling with sleep.
    async_send_fd: Option<AsyncFd<RawFdWrapper>>,

    /// AsyncFd wrapper for the receive queue (response queue for client, request queue for server).
    /// Allows epoll-based async waiting instead of polling with sleep.
    async_recv_fd: Option<AsyncFd<RawFdWrapper>>,
}

impl Default for PosixMessageQueueTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl PosixMessageQueueTransport {
    /// Create a new POSIX Message Queue transport
    ///
    /// Initializes a new transport instance in an uninitialized state.
    /// The transport must be configured as either a server or client
    /// before it can be used for communication.
    ///
    /// ## Returns
    /// New transport instance with default configuration
    ///
    /// ## Default Configuration
    ///
    /// - **State**: Uninitialized (requires explicit setup)
    /// - **Message Size**: 8KB (reasonable default for most use cases)
    /// - **Queue Depth**: 10 messages (typical system default)
    /// - **Server Status**: False (determined during initialization)
    ///
    /// ## Usage Pattern
    ///
    /// ```rust,no_run
    /// # use ipc_benchmark::ipc::{posix_message_queue::PosixMessageQueueTransport, TransportConfig, IpcTransport};
    /// # #[tokio::main]
    /// # async fn main() -> anyhow::Result<()> {
    /// let mut transport = PosixMessageQueueTransport::new();
    /// let config = TransportConfig::default();
    /// transport.start_server(&config).await?; // or start_client()
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            queue_name: String::new(),
            request_queue_fd: None,
            response_queue_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_server: false,
            has_warned_backpressure: false,
            config: None,
            async_send_fd: None,
            async_recv_fd: None,
        }
    }

    /// Clean up message queue resources
    ///
    /// Performs proper cleanup of message queue resources, including closing
    /// both queue file descriptors and unlinking queues if this instance created them.
    /// This method is idempotent and safe to call multiple times.
    ///
    /// ## Cleanup Strategy
    ///
    /// 1. **Close File Descriptors**: Release both queue file descriptors
    /// 2. **Conditional Unlink**: Only servers unlink queues from the system
    /// 3. **Error Tolerance**: Continues cleanup even if individual steps fail
    ///
    /// ## Resource Ownership
    ///
    /// Only queue creators (servers) unlink queues to prevent:
    /// - Premature queue destruction while clients are still connected
    /// - Race conditions between multiple clients
    /// - Resource leaks when clients disconnect improperly
    ///
    /// ## Error Handling
    ///
    /// Cleanup errors are logged as warnings rather than causing failures,
    /// ensuring that cleanup proceeds even if individual operations fail.
    fn cleanup_queue(&mut self) {
        debug!("Cleaning up POSIX message queues");

        // Drop AsyncFd wrappers first to deregister from epoll before closing FDs
        self.async_send_fd.take();
        self.async_recv_fd.take();

        // Close request queue
        if let Some(fd) = self.request_queue_fd.take() {
            debug!("Closing request queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close request queue: {}", e);
            } else {
                debug!("Closed request queue");
            }
        }

        // Close response queue
        if let Some(fd) = self.response_queue_fd.take() {
            debug!("Closing response queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close response queue: {}", e);
            } else {
                debug!("Closed response queue");
            }
        }

        // Only unlink the queues if this instance created them (server)
        if self.is_server && !self.queue_name.is_empty() {
            let request_queue_name = format!("{}_req", self.queue_name);
            let response_queue_name = format!("{}_resp", self.queue_name);

            // Unlink request queue
            match mq_unlink(request_queue_name.as_str()) {
                Ok(()) => debug!("Unlinked request queue: {}", request_queue_name),
                Err(nix::Error::ENOENT) => {
                    debug!("Request queue already unlinked: {}", request_queue_name);
                }
                Err(e) => warn!(
                    "Failed to unlink request queue '{}': {}",
                    request_queue_name, e
                ),
            }

            // Unlink response queue
            match mq_unlink(response_queue_name.as_str()) {
                Ok(()) => debug!("Unlinked response queue: {}", response_queue_name),
                Err(nix::Error::ENOENT) => {
                    debug!("Response queue already unlinked: {}", response_queue_name);
                }
                Err(e) => warn!(
                    "Failed to unlink response queue '{}': {}",
                    response_queue_name, e
                ),
            }
        }
    }
}

impl Drop for PosixMessageQueueTransport {
    /// Automatic cleanup when transport is dropped
    ///
    /// Ensures that message queue resources are properly cleaned up even
    /// if explicit close() is not called. This provides a safety net for
    /// resource management and prevents queue leaks.
    ///
    /// ## Safety Guarantee
    ///
    /// The Drop implementation ensures that:
    /// - File descriptors are always closed
    /// - Created queues are unlinked from the system
    /// - No resources are leaked during abnormal termination
    ///
    /// ## Performance Note
    ///
    /// While Drop provides safety, explicit close() is preferred for
    /// better error handling and deterministic resource cleanup timing.
    fn drop(&mut self) {
        debug!("Dropping POSIX Message Queue transport");
        self.cleanup_queue();
        debug!("POSIX Message Queue transport dropped");
    }
}

#[async_trait]
impl IpcTransport for PosixMessageQueueTransport {
    /// Initialize the transport as a server
    ///
    /// Creates two POSIX message queues (request and response) with the specified
    /// configuration and prepares them for bidirectional communication. The server
    /// is responsible for queue lifecycle management including creation and cleanup.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration containing queue parameters
    ///
    /// ## Returns
    /// - `Ok(())`: Server initialized and ready for communication
    /// - `Err(anyhow::Error)`: Server initialization failed
    ///
    /// ## Initialization Process
    ///
    /// 1. **Configure Parameters**: Set queue name, size limits, and attributes
    /// 2. **Create Request Queue**: For receiving messages from clients
    /// 3. **Create Response Queue**: For sending responses to clients
    /// 4. **Set Server Flag**: Mark this instance as responsible for cleanup
    /// 5. **Enable Non-Blocking**: Configure for high-throughput operation
    ///
    /// ## Dual Queue Architecture
    ///
    /// - **Request Queue** (`{name}_req`): Client → Server messages
    /// - **Response Queue** (`{name}_resp`): Server → Client messages
    ///
    /// ## Configuration Mapping
    ///
    /// - `message_queue_name` → Base queue identifier with "/" prefix
    /// - `message_queue_depth` → Maximum queued messages per queue
    /// - `buffer_size` → Maximum message size (minimum 1KB)
    ///
    /// ## Error Conditions
    ///
    /// - Queue name already exists and cannot be overwritten
    /// - Insufficient system resources for queue creation
    /// - System limits exceeded (max queues, memory limits)
    /// - Permission denied for queue creation
    /// - Invalid arguments, e.g., a `buffer_size` greater than the system's `msgsize_max`.
    ///
    /// ## System Integration
    ///
    /// The created queues persist in the system until explicitly unlinked,
    /// allowing clients to connect even if started after the server.
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue server with dual queues");

        self.config = Some(config.clone());
        // Normalize to exactly one leading '/'
        self.queue_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_server = true; // Mark this instance as the server

        let base_name = self.queue_name.clone();
        let max_msg_count = self.max_msg_count;
        let max_msg_size = self.max_msg_size;

        // Create both queues in a blocking task
        let (request_fd, response_fd) = tokio::task::spawn_blocking(move || {
            let request_queue_name = format!("{}_req", base_name);
            let response_queue_name = format!("{}_resp", base_name);
            let attr = MqAttr::new(0, max_msg_count as i64, max_msg_size as i64, 0);

            // Create request queue (server reads from this)
            debug!("Server creating request queue '{}'...", request_queue_name);
            let request_fd = match mq_open(
                request_queue_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                Mode::S_IRUSR | Mode::S_IWUSR,
                Some(&attr),
            ) {
                Ok(fd) => {
                    debug!(
                        "Server created request queue '{}' with fd: {:?}",
                        request_queue_name, fd
                    );
                    fd
                }
                Err(nix::Error::EINVAL) => {
                    return Err(anyhow!(
                        "Failed to create request queue: Invalid argument (EINVAL). \
                         Buffer size ({}) may exceed system limit.",
                        max_msg_size
                    ));
                }
                Err(e) => return Err(anyhow!("Failed to create request queue: {}", e)),
            };

            // Create response queue (server writes to this)
            debug!(
                "Server creating response queue '{}'...",
                response_queue_name
            );
            let response_fd = match mq_open(
                response_queue_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                Mode::S_IRUSR | Mode::S_IWUSR,
                Some(&attr),
            ) {
                Ok(fd) => {
                    debug!(
                        "Server created response queue '{}' with fd: {:?}",
                        response_queue_name, fd
                    );
                    fd
                }
                Err(nix::Error::EINVAL) => {
                    // Clean up request queue before returning error
                    let _ = mq_close(request_fd);
                    let _ = mq_unlink(request_queue_name.as_str());
                    return Err(anyhow!(
                        "Failed to create response queue: Invalid argument (EINVAL). \
                         Buffer size ({}) may exceed system limit.",
                        max_msg_size
                    ));
                }
                Err(e) => {
                    // Clean up request queue before returning error
                    let _ = mq_close(request_fd);
                    let _ = mq_unlink(request_queue_name.as_str());
                    return Err(anyhow!("Failed to create response queue: {}", e));
                }
            };

            Ok((request_fd, response_fd))
        })
        .await??;

        self.request_queue_fd = Some(request_fd);
        self.response_queue_fd = Some(response_fd);

        // Register FDs with tokio's epoll reactor for efficient async I/O.
        // Server sends on response queue and receives from request queue.
        let send_raw_fd = self.response_queue_fd.as_ref().unwrap().as_raw_fd();
        let recv_raw_fd = self.request_queue_fd.as_ref().unwrap().as_raw_fd();
        self.async_send_fd = Some(AsyncFd::new(RawFdWrapper(send_raw_fd))?);
        self.async_recv_fd = Some(AsyncFd::new(RawFdWrapper(recv_raw_fd))?);

        self.state = TransportState::Connected;
        debug!(
            "POSIX Message Queue server started with dual queues: {}_req, {}_resp",
            self.queue_name, self.queue_name
        );
        Ok(())
    }

    /// Initialize the transport as a client
    ///
    /// Connects to existing POSIX message queues (request and response) created
    /// by a server. The client waits for queue availability and establishes
    /// connections for bidirectional communication.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration containing queue parameters
    ///
    /// ## Returns
    /// - `Ok(())`: Client connected and ready for communication
    /// - `Err(anyhow::Error)`: Client connection failed
    ///
    /// ## Connection Process
    ///
    /// 1. **Configure Parameters**: Set queue name and size limits
    /// 2. **Retry Logic**: Wait for queue creation with exponential backoff
    /// 3. **Open Request Queue**: For sending messages to server
    /// 4. **Open Response Queue**: For receiving responses from server
    /// 5. **Set Client Flag**: Mark this instance as a queue user (not creator)
    ///
    /// ## Retry Strategy
    ///
    /// Uses exponential backoff to handle race conditions where clients
    /// start before servers have created queues:
    /// - **Initial Delay**: 10ms for quick server startup
    /// - **Exponential Growth**: Doubles delay each attempt (capped at 1s)
    /// - **Maximum Attempts**: 10 attempts over ~10 seconds
    ///
    /// ## Configuration Consistency
    ///
    /// Client configuration should match server parameters:
    /// - Queue name must exactly match server's queue base name
    /// - Message size limits should be compatible
    /// - Queue depth affects client send behavior
    ///
    /// ## Error Conditions
    ///
    /// - Server not started or queues not created
    /// - Permission denied for queue access
    /// - Configuration mismatch with server
    /// - System resource limitations
    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue client with dual queues");

        self.config = Some(config.clone());
        // Normalize to exactly one leading '/'
        self.queue_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_server = false; // Mark this instance as a client

        let base_name = self.queue_name.clone();

        // Open both queues with retry logic
        let (request_fd, response_fd) = tokio::task::spawn_blocking(move || {
            let request_queue_name = format!("{}_req", base_name);
            let response_queue_name = format!("{}_resp", base_name);

            // Retry opening the queues with exponential backoff
            let mut attempts = 0;
            let max_attempts = 10;
            let mut delay_ms = 10;

            // First, open request queue (client writes to this)
            let request_fd = loop {
                match mq_open(
                    request_queue_name.as_str(),
                    MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                    Mode::empty(),
                    None,
                ) {
                    Ok(fd) => {
                        debug!(
                            "Client opened request queue '{}' with fd: {:?} after {} attempts",
                            request_queue_name,
                            fd,
                            attempts + 1
                        );
                        break fd;
                    }
                    Err(Errno::ENOENT) if attempts < max_attempts => {
                        debug!(
                            "Request queue '{}' not ready, retrying in {}ms (attempt {}/{})",
                            request_queue_name,
                            delay_ms,
                            attempts + 1,
                            max_attempts
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        attempts += 1;
                        delay_ms = (delay_ms * 2).min(1000);
                        continue;
                    }
                    Err(e) => {
                        return Err(anyhow!(
                            "Failed to open request queue after {} attempts: {}",
                            attempts + 1,
                            e
                        ));
                    }
                }
            };

            // Reset retry counters for response queue
            attempts = 0;
            delay_ms = 10;

            // Open response queue (client reads from this)
            let response_fd = loop {
                match mq_open(
                    response_queue_name.as_str(),
                    MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                    Mode::empty(),
                    None,
                ) {
                    Ok(fd) => {
                        debug!(
                            "Client opened response queue '{}' with fd: {:?} after {} attempts",
                            response_queue_name,
                            fd,
                            attempts + 1
                        );
                        break fd;
                    }
                    Err(Errno::ENOENT) if attempts < max_attempts => {
                        debug!(
                            "Response queue '{}' not ready, retrying in {}ms (attempt {}/{})",
                            response_queue_name,
                            delay_ms,
                            attempts + 1,
                            max_attempts
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        attempts += 1;
                        delay_ms = (delay_ms * 2).min(1000);
                        continue;
                    }
                    Err(e) => {
                        // Clean up request queue before returning error
                        let _ = mq_close(request_fd);
                        return Err(anyhow!(
                            "Failed to open response queue after {} attempts: {}",
                            attempts + 1,
                            e
                        ));
                    }
                }
            };

            Ok((request_fd, response_fd))
        })
        .await??;

        self.request_queue_fd = Some(request_fd);
        self.response_queue_fd = Some(response_fd);

        // Register FDs with tokio's epoll reactor for efficient async I/O.
        // Client sends on request queue and receives from response queue.
        let send_raw_fd = self.request_queue_fd.as_ref().unwrap().as_raw_fd();
        let recv_raw_fd = self.response_queue_fd.as_ref().unwrap().as_raw_fd();
        self.async_send_fd = Some(AsyncFd::new(RawFdWrapper(send_raw_fd))?);
        self.async_recv_fd = Some(AsyncFd::new(RawFdWrapper(recv_raw_fd))?);

        self.state = TransportState::Connected;
        debug!(
            "POSIX Message Queue client connected to dual queues: {}_req, {}_resp",
            self.queue_name, self.queue_name
        );
        Ok(())
    }

    /// Send a message through the appropriate message queue
    ///
    /// Serializes and transmits a message through the correct POSIX message queue
    /// based on the role of this transport instance:
    /// - **Client**: Sends to request queue (client → server)
    /// - **Server**: Sends to response queue (server → client)
    ///
    /// Uses non-blocking I/O with intelligent retry logic for queue-full
    /// conditions. Messages are sent atomically and preserve boundaries.
    ///
    /// ## Parameters
    /// - `message`: Message to transmit through the queue
    ///
    /// ## Returns
    /// - `Ok(bool)`: Message sent successfully, bool indicates backpressure detected
    /// - `Err(anyhow::Error)`: Send operation failed
    ///
    /// ## Send Process
    ///
    /// 1. **Select Queue**: Choose request or response queue based on role
    /// 2. **Serialize Message**: Convert message to binary format using bincode
    /// 3. **Non-Blocking Send**: Attempt immediate send without blocking
    /// 4. **Retry Logic**: Handle queue-full conditions with backoff
    /// 5. **Error Handling**: Distinguish between temporary and permanent failures
    ///
    /// ## Queue-Full Handling
    ///
    /// When the queue is full (EAGAIN error):
    /// - **Fast Retry**: Initial 1ms delay for quick queue clearing
    /// - **Exponential Backoff**: Doubles delay each attempt (max 10ms)
    /// - **High Retry Count**: 100 attempts for sustained throughput
    /// - **Timeout Behavior**: Eventually fails if queue remains full
    ///
    /// ## Message Atomicity
    ///
    /// POSIX message queues guarantee atomic message transmission:
    /// - Messages are never partially sent
    /// - Message boundaries are preserved
    /// - No interleaving of concurrent sends
    ///
    /// ## Timestamp Accuracy
    ///
    /// To ensure accurate one-way latency measurement, the timestamp is updated
    /// immediately before the `mq_send` syscall. Since the queue is non-blocking
    /// and we call `mq_send` directly (no spawn_blocking), there is minimal
    /// overhead between timestamp capture and the actual kernel send.
    async fn send(&mut self, message: &Message) -> Result<bool> {
        // Pre-serialize the message
        let mut data = message.to_bytes()?;

        // Get the priority from the config, which was set during transport creation.
        let priority = self.config.as_ref().map_or(0, |c| c.pmq_priority);

        // Pre-compute timestamp offset for efficient in-place updates
        let ts_offset = Message::timestamp_offset();

        let async_fd = self
            .async_send_fd
            .as_ref()
            .ok_or_else(|| anyhow!("Send queue not initialized"))?;

        let raw_fd = async_fd.get_ref().as_raw_fd();

        let mut backpressure_detected = false;
        let start_time = std::time::Instant::now();
        let send_timeout = Duration::from_secs(5);

        loop {
            // CRITICAL: Update timestamp immediately before mq_send syscall.
            // No spawn_blocking overhead between timestamp and kernel send.
            let ts_now = crate::ipc::get_monotonic_time_ns();
            let ts_bytes = ts_now.to_le_bytes();
            if data.len() >= ts_offset.end {
                data[ts_offset.clone()].copy_from_slice(&ts_bytes);
            }

            // Call mq_send directly -- the queue is O_NONBLOCK so this never blocks.
            let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
            let result = mq_send(&fd, &data, priority);

            match result {
                Ok(()) => {
                    debug!("Sent message {} bytes via POSIX message queue", data.len());
                    return Ok(backpressure_detected);
                }
                Err(Errno::EAGAIN) => {
                    // Queue is full -- use epoll to wait for space efficiently
                    backpressure_detected = true;
                    if !self.has_warned_backpressure {
                        warn!(
                            "POSIX Message Queue is full; backpressure is occurring. \
                            This may impact latency and throughput measurements."
                        );
                        self.has_warned_backpressure = true;
                    }
                    let remaining = send_timeout.checked_sub(start_time.elapsed());
                    match remaining {
                        None | Some(Duration::ZERO) => {
                            return Err(anyhow!(IpcError::BackpressureTimeout));
                        }
                        Some(remaining) => {
                            // Wait for the queue to become writable via epoll,
                            // bounded by the remaining time to avoid indefinite hangs.
                            match tokio::time::timeout(remaining, async_fd.writable()).await {
                                Ok(Ok(mut guard)) => guard.clear_ready(),
                                Ok(Err(e)) => {
                                    return Err(anyhow!("Writable wait error: {}", e));
                                }
                                Err(_) => {
                                    return Err(anyhow!(IpcError::BackpressureTimeout));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(anyhow!("Failed to send message: {}", e));
                }
            }
        }
    }

    /// Receive a message from the appropriate message queue
    ///
    /// Waits for and receives a message from the correct POSIX message queue
    /// based on the role of this transport instance:
    /// - **Client**: Receives from response queue (server → client)
    /// - **Server**: Receives from request queue (client → server)
    ///
    /// Uses non-blocking I/O with retry logic for empty queue conditions. Messages
    /// are received atomically with preserved boundaries.
    ///
    /// ## Returns
    /// - `Ok(Message)`: Successfully received and deserialized message
    /// - `Err(anyhow::Error)`: Receive operation failed
    ///
    /// ## Receive Process
    ///
    /// 1. **Select Queue**: Choose request or response queue based on role
    /// 2. **Non-Blocking Receive**: Attempt immediate receive without blocking
    /// 3. **Buffer Management**: Use appropriately sized receive buffer
    /// 4. **Retry Logic**: Handle empty queue conditions with backoff
    /// 5. **Deserialization**: Convert received bytes back to Message struct
    ///
    /// ## Empty Queue Handling
    ///
    /// When the queue is empty (EAGAIN error):
    /// - **Fast Retry**: Initial 100µs delay for quick message arrival
    /// - **Exponential Backoff**: Doubles delay each attempt (max 500µs)
    /// - **High Retry Count**: 1000 attempts for responsiveness
    /// - **Timeout Behavior**: Eventually fails if no messages arrive
    ///
    /// ## Message Integrity
    ///
    /// POSIX message queues guarantee message integrity:
    /// - Complete messages are always received
    /// - Message boundaries are preserved
    /// - No partial or corrupted messages
    /// - FIFO ordering within priority levels
    ///
    /// ## Timestamp Accuracy
    ///
    /// To ensure accurate one-way latency measurement, the receive timestamp is
    /// captured immediately after `mq_receive` completes. Since `mq_receive` is
    /// called directly (no spawn_blocking), there is zero async scheduling overhead
    /// between the kernel returning the message and the timestamp capture.
    async fn receive(&mut self) -> Result<Message> {
        let async_fd = self
            .async_recv_fd
            .as_ref()
            .ok_or_else(|| anyhow!("Receive queue not initialized"))?;

        let raw_fd = async_fd.get_ref().as_raw_fd();
        let max_msg_size = self.max_msg_size;

        // Pre-allocate a receive buffer -- reused across retries
        let mut buffer = vec![0u8; max_msg_size];
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();

        loop {
            // Call mq_receive directly -- the queue is O_NONBLOCK so this never blocks.
            let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
            let mut priority = 0u32;
            let result = mq_receive(&fd, &mut buffer, &mut priority);

            match result {
                Ok(bytes_read) => {
                    // CRITICAL: Capture receive timestamp immediately after mq_receive.
                    // No spawn_blocking overhead -- this runs on the async task itself.
                    let receive_time_ns = crate::ipc::get_monotonic_time_ns();

                    debug!(
                        "Received message {} bytes via POSIX message queue",
                        bytes_read
                    );
                    let mut message = Message::from_bytes(&buffer[..bytes_read])?;

                    // Calculate and store one-way latency
                    // Only for Request/OneWay messages - Response messages already have
                    // one_way_latency_ns set by the server, so don't overwrite it
                    if message.message_type != crate::ipc::MessageType::Response {
                        message.one_way_latency_ns =
                            receive_time_ns.saturating_sub(message.timestamp);
                    }

                    return Ok(message);
                }
                Err(Errno::EAGAIN) => {
                    // Queue is empty -- use epoll to wait for data efficiently
                    let remaining = timeout.checked_sub(start.elapsed());
                    match remaining {
                        None | Some(Duration::ZERO) => {
                            return Err(anyhow!(
                                "Receive timed out after {:?} - queue consistently empty",
                                timeout
                            ));
                        }
                        Some(remaining) => {
                            // Wait for the queue to become readable via epoll,
                            // bounded by the remaining time to avoid indefinite hangs.
                            match tokio::time::timeout(remaining, async_fd.readable()).await {
                                Ok(Ok(mut guard)) => guard.clear_ready(),
                                Ok(Err(e)) => {
                                    return Err(anyhow!("Readable wait error: {}", e));
                                }
                                Err(_) => {
                                    return Err(anyhow!(
                                        "Receive timed out after {:?} - queue consistently empty",
                                        timeout
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    return Err(anyhow!("Failed to receive message: {}", e));
                }
            }
        }
    }

    /// Close the transport and clean up resources
    ///
    /// Performs graceful shutdown of the POSIX message queue transport,
    /// including closing both queue file descriptors and unlinking queues if
    /// this instance created them. This operation is idempotent and safe to
    /// call multiple times.
    ///
    /// ## Returns
    /// - `Ok(())`: Transport closed successfully
    /// - `Err(anyhow::Error)`: Close operation failed (rare)
    ///
    /// ## Shutdown Process
    ///
    /// 1. **Extract Resources**: Take ownership of both queue descriptors
    /// 2. **Close Descriptors**: Release both message queue file descriptors
    /// 3. **Conditional Unlink**: Remove queues from system if this is the server
    /// 4. **Update State**: Mark transport as disconnected
    ///
    /// ## Resource Ownership
    ///
    /// Only servers unlink queues during close:
    /// - **Server**: Unlinks both queues, making them unavailable to new clients
    /// - **Client**: Closes descriptors but leaves queues available for others
    ///
    /// ## Async Blocking Operation
    ///
    /// Queue operations are performed in a blocking task to avoid blocking
    /// the async runtime. This ensures proper cleanup even under load.
    ///
    /// ## Error Handling
    ///
    /// Close operations continue even if individual steps fail, ensuring
    /// maximum cleanup and state consistency. Errors are logged but don't
    /// prevent other cleanup operations.
    ///
    /// ## State Management
    ///
    /// The transport state is updated to Disconnected regardless of close
    /// operation success, preventing further use of an invalid transport.
    async fn close(&mut self) -> Result<()> {
        debug!("Closing POSIX Message Queue transport");

        // Drop AsyncFd wrappers FIRST to deregister from epoll before closing
        // the underlying file descriptors.
        self.async_send_fd.take();
        self.async_recv_fd.take();

        let queue_name = self.queue_name.clone();
        let request_fd = self.request_queue_fd.take();
        let response_fd = self.response_queue_fd.take();
        let is_server = self.is_server;

        tokio::task::spawn_blocking(move || {
            // Close request queue
            if let Some(fd) = request_fd {
                if let Err(e) = mq_close(fd) {
                    warn!("Failed to close request queue: {}", e);
                }
            }

            // Close response queue
            if let Some(fd) = response_fd {
                if let Err(e) = mq_close(fd) {
                    warn!("Failed to close response queue: {}", e);
                }
            }

            // Only unlink if this is the server
            if is_server && !queue_name.is_empty() {
                let request_queue_name = format!("{}_req", queue_name);
                let response_queue_name = format!("{}_resp", queue_name);

                if let Err(e) = mq_unlink(request_queue_name.as_str()) {
                    if e != nix::Error::ENOENT {
                        warn!(
                            "Failed to unlink request queue '{}': {}",
                            request_queue_name, e
                        );
                    }
                }

                if let Err(e) = mq_unlink(response_queue_name.as_str()) {
                    if e != nix::Error::ENOENT {
                        warn!(
                            "Failed to unlink response queue '{}': {}",
                            response_queue_name, e
                        );
                    }
                }
            }
        })
        .await
        .ok();

        self.state = TransportState::Disconnected;
        debug!("POSIX Message Queue transport closed");
        Ok(())
    }

    /// Get the transport name for identification
    ///
    /// Returns a human-readable name identifying this transport type.
    /// Used in logging, error messages, and benchmark result output.
    fn name(&self) -> &'static str {
        "POSIX Message Queue"
    }

    /// Check if transport supports bidirectional communication
    ///
    /// POSIX message queues support bidirectional communication through
    /// a single queue, allowing both send and receive operations on the
    /// same queue descriptor.
    ///
    /// ## Returns
    /// Always returns `true` - POSIX message queues support bidirectional I/O
    fn supports_bidirectional(&self) -> bool {
        true
    }

    /// Get maximum message size supported by this transport
    ///
    /// Returns the maximum size of individual messages that can be sent
    /// through this message queue. This is determined by the queue
    /// configuration and system limits.
    ///
    /// ## Returns
    /// Maximum message size in bytes (configured during queue creation)
    fn max_message_size(&self) -> usize {
        self.max_msg_size
    }

    /// Check if transport supports multiple concurrent connections
    ///
    /// POSIX message queues use a single shared queue for all communication,
    /// so they don't support true multiple connections in the same way as
    /// socket-based transports.
    ///
    /// ## Returns
    /// Always returns `false` - POSIX queues use single shared queue model
    ///
    /// ## Design Limitation
    ///
    /// While multiple processes can access the same queue, there's no way
    /// to distinguish between different clients or route messages to specific
    /// connections. All clients share the same queue namespace.
    fn supports_multiple_connections(&self) -> bool {
        false
    }

    /// Start multi-client server mode (dual queue simulation)
    ///
    /// Implements the multi-client interface for POSIX message queues by
    /// using the dual queue architecture and simulating multiple connections
    /// through message routing. All clients share the same queues.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration for queue setup
    ///
    /// ## Returns
    /// - `Ok(Receiver)`: Channel receiving (connection_id, message) pairs
    /// - `Err(anyhow::Error)`: Multi-server setup failed
    ///
    /// ## Implementation Strategy
    ///
    /// 1. **Start Standard Server**: Initialize dual message queues
    /// 2. **Create Message Channel**: Set up internal message routing
    /// 3. **Spawn Receiver Task**: Continuously read from request queue
    /// 4. **Assign Connection ID**: Use fixed connection ID for all messages
    ///
    /// ## Multi-Client Simulation
    ///
    /// Since POSIX queues don't distinguish between clients:
    /// - All clients share the same queues
    /// - Messages are assigned a fixed connection ID (1)
    /// - No true isolation between different clients
    /// - All responses go to the shared response queue
    ///
    /// ## Performance Characteristics
    ///
    /// - **Throughput**: Limited by queue capacity
    /// - **Latency**: No routing overhead beyond queue operations
    /// - **Scalability**: Poor due to shared queue contention
    /// - **Reliability**: High due to kernel message management
    ///
    /// ## Error Handling
    ///
    /// The receiver task handles errors by:
    /// - Logging receive failures and terminating
    /// - Handling message deserialization errors gracefully
    /// - Breaking on channel send failures (receiver disconnected)
    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        debug!("POSIX Message Queue multi-server mode - using dual queues");

        self.start_server(config).await?;

        let (tx, rx) = mpsc::channel(1000);
        // Server receives from request queue
        let fd_ref = self
            .request_queue_fd
            .as_ref()
            .ok_or_else(|| anyhow!("No request queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        let max_msg_size = self.max_msg_size;

        tokio::spawn(async move {
            let connection_id = 1;

            loop {
                let result = tokio::task::spawn_blocking({
                    let raw_fd_copy = raw_fd;
                    move || {
                        let fd = unsafe { MqdT::from_raw_fd(raw_fd_copy) };
                        let mut buffer = vec![0u8; max_msg_size];
                        let mut priority = 0u32;
                        mq_receive(&fd, &mut buffer, &mut priority).map(|bytes_read| {
                            buffer.truncate(bytes_read);
                            buffer
                        })
                    }
                })
                .await;

                match result {
                    Ok(Ok(buffer)) => {
                        debug!(
                            "Received message {} bytes via POSIX message queue",
                            buffer.len()
                        );
                        if let Ok(message) = Message::from_bytes(&buffer) {
                            if tx.send((connection_id, message)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Failed to receive message: {}", e);
                        break;
                    }
                    Err(e) => {
                        error!("Task join error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// Send message to specific connection (queue-shared implementation)
    ///
    /// In the POSIX message queue implementation, all clients share the same
    /// queue, so this method ignores the connection ID and sends to the
    /// shared queue. All connected clients can potentially receive the message.
    ///
    /// ## Parameters
    /// - `_connection_id`: Ignored (no true connection isolation)
    /// - `message`: Message to send to the shared queue
    ///
    /// ## Returns
    /// - `Ok(())`: Message sent to shared queue successfully
    /// - `Err(anyhow::Error)`: Send operation failed
    ///
    /// ## Design Limitation
    ///
    /// POSIX message queues don't support connection-specific routing:
    /// - Messages go to a shared queue accessible by all clients
    /// - No way to target specific clients
    /// - First client to read gets the message
    /// - Broadcast behavior requires message duplication
    async fn send_to_connection(
        &mut self,
        _connection_id: ConnectionId,
        message: &Message,
    ) -> Result<()> {
        self.send(message).await?;
        Ok(())
    }

    /// Get list of active connection IDs
    ///
    /// Returns a fixed connection ID since POSIX message queues use a
    /// shared queue model rather than individual connections. All clients
    /// are treated as a single logical connection.
    ///
    /// ## Returns
    /// Vector containing single connection ID (1) representing the shared queue
    fn get_active_connections(&self) -> Vec<ConnectionId> {
        vec![1]
    }

    /// Close specific connection (shared queue close)
    ///
    /// Since POSIX message queues use a shared queue model, closing a
    /// "connection" means closing the entire queue, which affects all
    /// connected clients.
    ///
    /// ## Parameters
    /// - `_connection_id`: Ignored (no connection isolation)
    ///
    /// ## Returns
    /// - `Ok(())`: Queue closed successfully
    /// - `Err(anyhow::Error)`: Close operation failed
    ///
    /// ## Impact
    ///
    /// Closing any connection closes the entire queue for all clients:
    /// - All clients lose access to the queue
    /// - Server unlinks the queue from the system
    /// - No selective client disconnection possible
    async fn close_connection(&mut self, _connection_id: ConnectionId) -> Result<()> {
        self.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
    use tokio::time::{sleep, Duration};
    use uuid::Uuid;

    #[tokio::test]
    async fn test_pmq_communication() {
        let queue_name = format!("test-pmq-{}", Uuid::new_v4().as_simple());
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };

        let mut server = PosixMessageQueueTransport::new();
        let mut client = PosixMessageQueueTransport::new();

        // Start server in background
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();

            // Receive message
            let message = server.receive().await.unwrap();
            assert_eq!(message.id, 1);
            assert_eq!(message.payload, vec![1, 2, 3, 4, 5]);

            // Send response
            let response = Message::new(2, vec![6, 7, 8], MessageType::Response);
            server.send(&response).await.unwrap();

            server.close().await.unwrap();
        });

        // Justification: Give the server task time to start up and create the message queue before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(100)).await;

        // Start client and communicate
        client.start_client(&config).await.unwrap();

        let message = Message::new(1, vec![1, 2, 3, 4, 5], MessageType::Request);
        client.send(&message).await.unwrap();

        // Justification: Allow a brief moment for the message to be processed by the server.
        // This is necessary for single-queue transports to avoid the client reading its own message.
        sleep(Duration::from_millis(100)).await;

        let response = client.receive().await.unwrap();
        assert_eq!(response.id, 2);
        assert_eq!(response.payload, vec![6, 7, 8]);

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_pmq_backpressure() {
        let queue_name = format!("test-pmq-backpressure-{}", Uuid::new_v4().as_simple());
        let config = TransportConfig {
            message_queue_name: queue_name,
            message_queue_depth: 2, // Small queue to trigger backpressure easily
            buffer_size: 1024,
            ..Default::default()
        };

        let mut server = PosixMessageQueueTransport::new();
        let mut client = PosixMessageQueueTransport::new();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        // Start server in background, but it won't receive anything.
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            // Wait for the client to signal it's done.
            rx.await.unwrap();
            server.close().await.unwrap();
        });

        // Justification: Give the server task time to start up and create the message queue before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(100)).await;

        // Start client
        client.start_client(&config).await.unwrap();

        let mut backpressure_detected = false;
        let payload = vec![0; 512];

        // Send messages until a backpressure timeout occurs.
        for i in 0..5 {
            let message = Message::new(i, payload.clone(), MessageType::Request);
            match client.send(&message).await {
                Ok(bp_detected) => {
                    if bp_detected {
                        // This is expected for the first few full-queue sends.
                        println!("Regular backpressure detected, continuing to force a timeout.");
                    }
                }
                Err(e) => {
                    // We expect a specific error related to backpressure timeout.
                    if e.to_string()
                        .contains("Timeout sending message due to backpressure")
                    {
                        backpressure_detected = true;
                        break;
                    }
                    panic!("An unexpected error occurred: {}", e);
                }
            }
        }

        assert!(
            backpressure_detected,
            "Backpressure was not detected when the PMQ was full"
        );

        // Signal the server to shut down.
        tx.send(()).unwrap();
        server_handle.await.unwrap();
        client.close().await.unwrap();
    }

    /// Test that messages are sent with the specified priority.
    #[tokio::test]
    async fn test_pmq_priority_is_applied() {
        let queue_name = format!("test-pmq-priority-{}", Uuid::new_v4().as_simple());
        let priority = 5;
        let config = TransportConfig {
            message_queue_name: queue_name,
            pmq_priority: priority,
            ..Default::default()
        };

        let mut server = PosixMessageQueueTransport::new();
        let mut client = PosixMessageQueueTransport::new();

        // Use a oneshot channel to signal when the server is ready.
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        // Start server in background
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            // Signal that the server is ready.
            tx.send(()).unwrap();

            // Receive message and check its priority, with retries for EAGAIN.
            // Server receives from request queue
            let fd = server.request_queue_fd.as_ref().unwrap();
            let mut buffer = vec![0u8; server.max_msg_size];
            let mut received_priority = 0u32;

            // Retry receiving for a short period to handle timing variations.
            for _ in 0..10 {
                let result = mq_receive(fd, &mut buffer, &mut received_priority);
                if let Ok(bytes_read) = result {
                    buffer.truncate(bytes_read);
                    break;
                } else if result == Err(nix::Error::EAGAIN) {
                    // Queue is empty, wait briefly and retry.
                    sleep(Duration::from_millis(10)).await;
                    continue;
                }
                // For any other error, fail the test.
                result.unwrap();
            }

            assert_eq!(received_priority, priority);

            let message = Message::from_bytes(&buffer).unwrap();
            assert_eq!(message.id, 1);

            server.close().await.unwrap();
        });

        // Wait for the server to be ready before starting the client.
        rx.await.unwrap();

        // Start client and send a message
        client.start_client(&config).await.unwrap();
        let message = Message::new(1, vec![1, 2, 3], MessageType::Request);
        client.send(&message).await.unwrap();

        // Wait for server to finish its work.
        server_handle.await.unwrap();
        client.close().await.unwrap();
    }

    /// Test PMQ round-trip: client sends Request, server echoes
    /// back Response with the same payload.
    #[tokio::test]
    async fn test_pmq_round_trip() {
        let queue_name = format!("test-pmq-rt-{}", Uuid::new_v4().as_simple());
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };

        let mut server = PosixMessageQueueTransport::new();
        let mut client = PosixMessageQueueTransport::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            tx.send(()).unwrap();

            for expected_id in 0u64..3 {
                let msg = server.receive().await.unwrap();
                assert_eq!(msg.id, expected_id);
                assert_eq!(msg.message_type, MessageType::Request);

                let resp = Message::new(msg.id, msg.payload.clone(), MessageType::Response);
                server.send(&resp).await.unwrap();
            }

            server.close().await.unwrap();
        });

        rx.await.unwrap();
        client.start_client(&config).await.unwrap();

        for id in 0u64..3 {
            let payload = vec![id as u8; 64];
            let msg = Message::new(id, payload.clone(), MessageType::Request);
            client.send(&msg).await.unwrap();

            // Justification: Allow server to process the message
            // and write its response to the response queue. PMQ
            // is single-queue per direction, so a brief wait
            // avoids reading our own message.
            sleep(Duration::from_millis(50)).await;

            let resp = client.receive().await.unwrap();
            assert_eq!(resp.id, id);
            assert_eq!(resp.payload, payload);
            assert_eq!(resp.message_type, MessageType::Response);
        }

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }

    /// Test PMQ with various message sizes.
    #[tokio::test]
    async fn test_pmq_various_message_sizes() {
        let sizes: Vec<usize> = vec![1, 32, 128, 512];
        let sizes_clone = sizes.clone();

        let queue_name = format!("test-pmq-sz-{}", Uuid::new_v4().as_simple());
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };

        let mut server = PosixMessageQueueTransport::new();
        let mut client = PosixMessageQueueTransport::new();

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            tx.send(()).unwrap();

            for (i, &expected_size) in sizes_clone.iter().enumerate() {
                let msg = server.receive().await.unwrap();
                assert_eq!(msg.id, i as u64);
                assert_eq!(
                    msg.payload.len(),
                    expected_size,
                    "Size mismatch for message {}",
                    i
                );
            }

            server.close().await.unwrap();
        });

        rx.await.unwrap();
        client.start_client(&config).await.unwrap();

        for (i, &size) in sizes.iter().enumerate() {
            let payload = vec![0xCD_u8; size];
            let msg = Message::new(i as u64, payload, MessageType::OneWay);
            client.send(&msg).await.unwrap();
        }

        // Justification: Allow time for server to drain all
        // messages before we close.
        sleep(Duration::from_millis(200)).await;

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }
}
