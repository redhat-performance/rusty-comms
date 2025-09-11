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
//!
//! ## Architecture Overview
//!
//! POSIX Message Queues operate as named system objects that can be accessed by
//! multiple processes. The kernel manages message storage, ordering, and delivery:
//!
//! ```text
//! ┌─────────────┐    ┌─────────────────┐    ┌────────���────┐
//! │   Client    │───▶│  Kernel Queue   │───▶│   Server    │
//! │  Process    │    │   (FIFO with    │    │  Process    │
//! │             │◀───│   priorities)   │◀───│             │
//! └─────────────┘    └─────────────────┘    └─────────────┘
//! ```
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
//! - **Queue Naming**: Uses "/" prefix for portable queue names
//! - **Creation vs. Opening**: Server creates queues, clients open existing ones
//! - **Cleanup**: Only queue creators (servers) unlink queues on close
//! - **Non-Blocking**: Uses O_NONBLOCK with retry logic for throughput
//! - **Resource Management**: Proper cleanup prevents queue leaks
//!
//! ## Limitations
//!
//! - **Single Queue**: Uses one bidirectional queue (no true multi-client support)
//! - **Message Size**: Limited by system configuration (typically 8KB default)
//! - **Queue Depth**: Limited by system configuration (typically 10 messages)
//! - **Platform**: UNIX-like systems only (Linux, macOS, BSD)
//! - **Permissions**: Requires appropriate system permissions for queue operations

use super::{ConnectionId, IpcError, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use nix::errno::Errno;
use nix::mqueue::{mq_close, mq_open, mq_receive, mq_send, mq_unlink, MQ_OFlag, MqAttr, MqdT};
use nix::sys::stat::Mode;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

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

    /// POSIX message queue name (with "/" prefix)
    queue_name: String,

    /// Message queue file descriptor for I/O operations
    mq_fd: Option<MqdT>,

    /// Maximum size of individual messages in bytes
    max_msg_size: usize,

    /// Maximum number of messages that can be queued
    max_msg_count: usize,

    /// Whether this instance created the queue (server) vs opened it (client)
    ///
    /// Only queue creators are responsible for unlinking the queue during cleanup.
    /// This prevents clients from accidentally destroying queues that other
    /// processes may still be using.
    is_creator: bool,

    /// Flag to ensure the backpressure warning is only logged once.
    has_warned_backpressure: bool,

    /// Transport configuration, stored after initialization.
    ///
    /// This holds a copy of the `TransportConfig` used to initialize the
    /// transport. It is `None` until `start_server` or `start_client` is
    /// called. Storing the configuration allows the transport to access
    /// parameters like `pmq_priority` during its operation.
    config: Option<TransportConfig>,
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
    /// - **Creator Status**: False (determined during initialization)
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
            mq_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_creator: false,
            has_warned_backpressure: false,
            config: None,
        }
    }

    /// Clean up message queue resources
    ///
    /// Performs proper cleanup of message queue resources, including closing
    /// the file descriptor and unlinking the queue if this instance created it.
    /// This method is idempotent and safe to call multiple times.
    ///
    /// ## Cleanup Strategy
    ///
    /// 1. **Close File Descriptor**: Release the queue file descriptor
    /// 2. **Conditional Unlink**: Only creators unlink queues from the system
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
        debug!("Cleaning up POSIX message queue");

        if let Some(fd) = self.mq_fd.take() {
            debug!("Closing message queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close message queue: {}", e);
            } else {
                debug!("Closed message queue");
            }
        }

        // Only unlink the queue if this instance created it (server)
        if self.is_creator && !self.queue_name.is_empty() {
            match mq_unlink(self.queue_name.as_str()) {
                Ok(()) => debug!("Unlinked message queue: {}", self.queue_name),
                Err(nix::Error::ENOENT) => {
                    // This is fine, the queue was already gone.
                    debug!("Message queue already unlinked: {}", self.queue_name);
                }
                Err(e) => warn!(
                    "Failed to unlink message queue '{}': {}",
                    self.queue_name, e
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
    /// Creates a new POSIX message queue with the specified configuration
    /// and prepares it to receive messages from clients. The server is
    /// responsible for queue lifecycle management including creation and cleanup.
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
    /// 2. **Create Queue**: Create new message queue with specified attributes
    /// 3. **Set Creator Flag**: Mark this instance as responsible for cleanup
    /// 4. **Enable Non-Blocking**: Configure for high-throughput operation
    ///
    /// ## Configuration Mapping
    ///
    /// - `message_queue_name` → Queue identifier with "/" prefix
    /// - `message_queue_depth` → Maximum queued messages
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
    /// The created queue persists in the system until explicitly unlinked,
    /// allowing clients to connect even if started after the server.
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue server");

        self.config = Some(config.clone());
        self.queue_name = format!("/{}", config.message_queue_name);
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = true; // Mark this instance as the creator

        // Open/create the message queue in a blocking task
        let queue_name = self.queue_name.clone();
        let max_msg_count = self.max_msg_count;
        let max_msg_size = self.max_msg_size;

        let mq_fd = tokio::task::spawn_blocking(move || {
            debug!("Server creating message queue '{}'...", queue_name);
            let attr = MqAttr::new(0, max_msg_count as i64, max_msg_size as i64, 0);
            let result = match mq_open(
                queue_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                Mode::S_IRUSR | Mode::S_IWUSR,
                Some(&attr),
            ) {
                Ok(fd) => Ok(fd),
                Err(nix::Error::EINVAL) => Err(anyhow!(
                    "Failed to create server queue: Invalid argument (EINVAL). \\
                     This may be due to a buffer size ({}) greater than the system limit.",
                    max_msg_size
                )),
                Err(e) => Err(anyhow!("Failed to create server queue: {}", e)),
            };

            match &result {
                Ok(fd) => debug!(
                    "Server successfully created queue '{}' with fd: {:?}",
                    queue_name, fd
                ),
                Err(e) => debug!("Server failed to create queue '{}': {}", queue_name, e),
            }

            result
        })
        .await??;

        self.mq_fd = Some(mq_fd);
        self.state = TransportState::Connected;
        debug!(
            "POSIX Message Queue server started with queue: {}",
            self.queue_name
        );
        Ok(())
    }

    /// Initialize the transport as a client
    ///
    /// Connects to an existing POSIX message queue created by a server.
    /// The client waits for queue availability and establishes a connection
    /// for bidirectional communication.
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
    /// 3. **Open Queue**: Connect to existing queue in read/write mode
    /// 4. **Set Client Flag**: Mark this instance as a queue user (not creator)
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
    /// - Queue name must exactly match server's queue
    /// - Message size limits should be compatible
    /// - Queue depth affects client send behavior
    ///
    /// ## Error Conditions
    ///
    /// - Server not started or queue not created
    /// - Permission denied for queue access
    /// - Configuration mismatch with server
    /// - System resource limitations
    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue client");

        self.config = Some(config.clone());
        self.queue_name = format!("/{}", config.message_queue_name);
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = false; // Mark this instance as a client

        // Open existing message queue with retry logic
        let queue_name = self.queue_name.clone();

        let mq_fd = tokio::task::spawn_blocking(move || {
            // Retry opening the queue with exponential backoff
            let mut attempts = 0;
            let max_attempts = 10;
            let mut delay_ms = 10;

            loop {
                match mq_open(
                    queue_name.as_str(),
                    MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK, // Add O_NONBLOCK
                    Mode::empty(),
                    None,
                ) {
                    Ok(fd) => {
                        debug!(
                            "Client successfully opened queue '{}' with fd: {:?} after {} attempts",
                            queue_name,
                            fd,
                            attempts + 1
                        );
                        return Ok(fd);
                    }
                    Err(Errno::ENOENT) if attempts < max_attempts => {
                        debug!(
                            "Queue '{}' not ready yet, retrying in {}ms (attempt {}/{})",
                            queue_name,
                            delay_ms,
                            attempts + 1,
                            max_attempts
                        );
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        attempts += 1;
                        delay_ms = (delay_ms * 2).min(1000); // Cap at 1 second
                        continue;
                    }
                    Err(e) => {
                        return Err(anyhow!(
                            "Failed to open client queue after {} attempts: {}",
                            attempts + 1,
                            e
                        ));
                    }
                }
            }
        })
        .await??;

        self.mq_fd = Some(mq_fd);
        self.state = TransportState::Connected;
        debug!(
            "POSIX Message Queue client connected to queue: {}",
            self.queue_name
        );
        Ok(())
    }

    /// Send a message through the message queue
    ///
    /// Serializes and transmits a message through the POSIX message queue
    /// using non-blocking I/O with intelligent retry logic for queue-full
    /// conditions. Messages are sent atomically and preserve boundaries.
    ///
    /// ## Parameters
    /// - `message`: Message to transmit through the queue
    ///
    /// ## Returns
    /// - `Ok(())`: Message sent successfully
    /// - `Err(anyhow::Error)`: Send operation failed
    ///
    /// ## Send Process
    ///
    /// 1. **Serialize Message**: Convert message to binary format using bincode
    /// 2. **Non-Blocking Send**: Attempt immediate send without blocking
    /// 3. **Retry Logic**: Handle queue-full conditions with backoff
    /// 4. **Error Handling**: Distinguish between temporary and permanent failures
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
    /// ## Performance Optimization
    ///
    /// - **Non-Blocking I/O**: Prevents thread blocking on full queues
    /// - **Fast Retries**: Optimized for high-throughput scenarios
    /// - **File Descriptor Reuse**: Efficient handling of queue descriptors
    ///
    /// ## Error Categories
    ///
    /// - **Temporary**: Queue full (EAGAIN) - retried automatically
    /// - **Permanent**: Invalid parameters, permissions, or system errors
    /// - **Resource**: No queue available or transport not initialized
    /// ## Backpressure Detection
    ///
    /// Backpressure is detected when a send operation returns an `EAGAIN`
    /// error, indicating that the message queue is full. The transport will
    /// retry with a backoff, and a warning will be logged on the first
    /// occurrence.
    async fn send(&mut self, message: &Message) -> Result<bool> {
        let data = message.to_bytes()?;
        let fd_ref = self
            .mq_fd
            .as_ref()
            .ok_or_else(|| anyhow!("No message queue available"))?;

        // Since MqdT is just a wrapper around an fd, we can extract its raw fd
        let raw_fd = fd_ref.as_raw_fd();
        let mut backpressure_detected = false;

        // Get the priority from the config, which was set during transport creation.
        let priority = self.config.as_ref().map_or(0, |c| c.pmq_priority);

        // Use non-blocking send with exponential backoff for queue-full conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100; // More retries since each one is much faster

        for attempt in 0..max_retries {
            let start_time = std::time::Instant::now();
            let result = tokio::task::spawn_blocking({
                let data = data.clone();
                move || {
                    // Reconstruct MqdT from raw fd for the blocking operation
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    // std::mem::forget(fd); // Don't close the fd when this MqdT drops
                    mq_send(&fd, &data, priority)
                }
            })
            .await?;

            match result {
                Ok(()) => {
                    let elapsed = start_time.elapsed();
                    // A send operation taking longer than a few milliseconds is a strong
                    // indicator of the OS send buffer being full.
                    if elapsed > std::time::Duration::from_millis(5) {
                        backpressure_detected = true;
                        if !self.has_warned_backpressure {
                            warn!(
                                "PMQ backpressure detected (send took {:?}). \n                                This may impact latency and throughput measurements.",
                                elapsed
                            );
                            self.has_warned_backpressure = true;
                        }
                    }
                    debug!("Sent message {} bytes via POSIX message queue", data.len());
                    return Ok(backpressure_detected);
                }
                Err(Errno::EAGAIN) => {
                    backpressure_detected = true;
                    if !self.has_warned_backpressure {
                        warn!(
                            "POSIX Message Queue is full; backpressure is occurring. \n                            This may impact latency and throughput measurements."
                        );
                        self.has_warned_backpressure = true;
                    }
                    // Queue is full, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!(IpcError::BackpressureTimeout));
                    }
                    // Justification: Short, exponentially increasing delay to wait for space to become available
                    // in a full queue without busy-waiting.
                    tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                    retry_delay_ms = (retry_delay_ms * 2).min(10); // Cap at 10ms for faster throughput
                }
                Err(e) => {
                    return Err(anyhow!("Failed to send message: {}", e));
                }
            }
        }

        Err(anyhow!("Send failed after {} attempts", max_retries))
    }

    /// Receive a message from the message queue
    ///
    /// Waits for and receives a message from the POSIX message queue using
    /// non-blocking I/O with retry logic for empty queue conditions. Messages
    /// are received atomically with preserved boundaries.
    ///
    /// ## Returns
    /// - `Ok(Message)`: Successfully received and deserialized message
    /// - `Err(anyhow::Error)`: Receive operation failed
    ///
    /// ## Receive Process
    ///
    /// 1. **Non-Blocking Receive**: Attempt immediate receive without blocking
    /// 2. **Buffer Management**: Use appropriately sized receive buffer
    /// 3. **Retry Logic**: Handle empty queue conditions with backoff
    /// 4. **Deserialization**: Convert received bytes back to Message struct
    ///
    /// ## Empty Queue Handling
    ///
    /// When the queue is empty (EAGAIN error):
    /// - **Fast Retry**: Initial 1ms delay for quick message arrival
    /// - **Exponential Backoff**: Doubles delay each attempt (max 10ms)
    /// - **High Retry Count**: 100 attempts for responsiveness
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
    /// ## Buffer Management
    ///
    /// - **Size Allocation**: Buffer sized to maximum message size
    /// - **Dynamic Truncation**: Buffer truncated to actual message size
    /// - **Memory Efficiency**: Buffers allocated per operation, not cached
    ///
    /// ## Performance Considerations
    ///
    /// - **Non-Blocking I/O**: Prevents thread blocking on empty queues
    /// - **Fast Polling**: Optimized retry timing for low latency
    /// - **Zero-Copy**: Minimal data copying during receive operations
    ///
    /// ## Error Categories
    ///
    /// - **Temporary**: Queue empty (EAGAIN) - retried automatically
    /// - **Permanent**: Invalid queue state or deserialization failure
    /// - **Resource**: No queue available or transport not initialized
    async fn receive(&mut self) -> Result<Message> {
        let fd_ref = self
            .mq_fd
            .as_ref()
            .ok_or_else(|| anyhow!("No message queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        let max_msg_size = self.max_msg_size;

        // Use non-blocking receive with exponential backoff for empty queue conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100;

        for attempt in 0..max_retries {
            let result = tokio::task::spawn_blocking({
                move || {
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    let mut buffer = vec![0u8; max_msg_size];
                    let mut priority = 0u32;
                    // std::mem::forget(fd); // Don't close the fd when this M-q-dT drops
                    mq_receive(&fd, &mut buffer, &mut priority).map(|bytes_read| {
                        buffer.truncate(bytes_read);
                        buffer
                    })
                }
            })
            .await?;

            match result {
                Ok(buffer) => {
                    debug!(
                        "Received message {} bytes via POSIX message queue",
                        buffer.len()
                    );
                    return Message::from_bytes(&buffer);
                }
                Err(Errno::EAGAIN) => {
                    // Queue is empty, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!(
                            "Receive failed after {} attempts - queue consistently empty",
                            max_retries
                        ));
                    }
                    // Justification: Short, exponentially increasing delay to wait for a message to arrive
                    // in an empty queue without busy-waiting.
                    tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                    retry_delay_ms = (retry_delay_ms * 2).min(10); // Cap at 10ms
                }
                Err(e) => {
                    return Err(anyhow!("Failed to receive message: {}", e));
                }
            }
        }

        Err(anyhow!("Receive failed after {} attempts", max_retries))
    }

    /// Close the transport and clean up resources
    ///
    /// Performs graceful shutdown of the POSIX message queue transport,
    /// including closing file descriptors and unlinking queues if this
    /// instance created them. This operation is idempotent and safe to
    /// call multiple times.
    ///
    /// ## Returns
    /// - `Ok(())`: Transport closed successfully
    /// - `Err(anyhow::Error)`: Close operation failed (rare)
    ///
    /// ## Shutdown Process
    ///
    /// 1. **Extract Resources**: Take ownership of queue descriptor and state
    /// 2. **Close Descriptor**: Release the message queue file descriptor
    /// 3. **Conditional Unlink**: Remove queue from system if this instance created it
    /// 4. **Update State**: Mark transport as disconnected
    ///
    /// ## Resource Ownership
    ///
    /// Only queue creators (servers) unlink queues during close:
    /// - **Server**: Unlinks queue, making it unavailable to new clients
    /// - **Client**: Closes descriptor but leaves queue available for others
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
        let queue_name = self.queue_name.clone();
        let mq_fd = self.mq_fd.take();
        let is_creator = self.is_creator;

        tokio::task::spawn_blocking(move || {
            if let Some(fd) = mq_fd {
                if let Err(e) = mq_close(fd) {
                    warn!("Failed to close message queue: {}", e);
                }
            }

            // Only unlink if this instance created the queue
            if is_creator && !queue_name.is_empty() {
                if let Err(e) = mq_unlink(queue_name.as_str()) {
                    if e != nix::Error::ENOENT {
                        warn!("Failed to unlink message queue '{}': {}", queue_name, e);
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

    /// Start multi-client server mode (single queue simulation)
    ///
    /// Implements the multi-client interface for POSIX message queues by
    /// using a single shared queue and simulating multiple connections
    /// through message routing. All clients share the same queue.
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
    /// 1. **Start Standard Server**: Initialize single message queue
    /// 2. **Create Message Channel**: Set up internal message routing
    /// 3. **Spawn Receiver Task**: Continuously read from queue and forward messages
    /// 4. **Assign Connection ID**: Use fixed connection ID for all messages
    ///
    /// ## Multi-Client Simulation
    ///
    /// Since POSIX queues don't distinguish between clients:
    /// - All clients share the same queue
    /// - Messages are assigned a fixed connection ID (1)
    /// - No true isolation between different clients
    /// - All responses go to the shared queue
    ///
    /// ## Performance Characteristics
    ///
    /// - **Throughput**: Limited by single queue capacity
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
        debug!("POSIX Message Queue multi-server mode - using single connection");

        self.start_server(config).await?;

        let (tx, rx) = mpsc::channel(1000);
        let fd_ref = self
            .mq_fd
            .as_ref()
            .ok_or_else(|| anyhow!("No message queue available"))?;
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
                        // std::mem::forget(fd); // Don't close the fd when this MqdT drops
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
            let fd = server.mq_fd.as_ref().unwrap();
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
}
