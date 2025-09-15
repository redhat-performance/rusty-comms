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
//! ```
//! ┌─────────────┐    ┌─────────────────┐    ┌─────────────┐
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

use super::{ConnectionId, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use nix::errno::Errno;
#[cfg(target_os = "linux")]
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
    
    /// Base POSIX message queue name (without suffix, no leading "/")
    base_queue_name: String,
    
    /// Message queue file descriptor used for sending
    send_mq_fd: Option<MqdT>,

    /// Message queue file descriptor used for receiving
    recv_mq_fd: Option<MqdT>,
    
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
    /// ```rust
    /// let mut transport = PosixMessageQueueTransport::new();
    /// transport.start_server(&config).await?; // or start_client()
    /// ```
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            base_queue_name: String::new(),
            send_mq_fd: None,
            recv_mq_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_creator: false,
        }
    }

    /// Open a message queue with specified parameters
    ///
    /// This internal method handles the low-level POSIX message queue opening
    /// operations, including queue creation for servers and connection for clients.
    ///
    /// ## Parameters
    /// - `queue_name`: Name of the queue to open (must start with "/")
    /// - `create`: Whether to create the queue if it doesn't exist
    ///
    /// ## Returns
    /// - `Ok(MqdT)`: Message queue file descriptor ready for I/O
    /// - `Err(anyhow::Error)`: Queue opening failed
    ///
    /// ## Queue Creation vs. Opening
    ///
    /// - **Create Mode**: Used by servers to create new queues with specific attributes
    /// - **Open Mode**: Used by clients to connect to existing queues
    ///
    /// ## Flags and Permissions
    ///
    /// - **O_CREAT**: Create queue if it doesn't exist (server only)
    /// - **O_RDWR**: Read/write access for bidirectional communication
    /// - **User R/W**: Owner read/write permissions for security
    ///
    /// ## Error Conditions
    ///
    /// - Queue name conflicts or invalid names
    /// - Insufficient system resources
    /// - Permission denied
    /// - System queue limits exceeded
    fn open_queue(&self, queue_name: &str, create: bool) -> Result<MqdT> {
        let flags = if create {
            MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR
        } else {
            MQ_OFlag::O_RDWR
        };
        
        let attr = if create {
            Some(MqAttr::new(0, self.max_msg_count as i64, self.max_msg_size as i64, 0))
        } else {
            None
        };
        
        let mq_fd = mq_open(
            queue_name,
            flags,
            Mode::S_IRUSR | Mode::S_IWUSR,
            attr.as_ref(),
        ).map_err(|e| anyhow!("Failed to open queue '{}': {}", queue_name, e))?;

        debug!("Opened message queue '{}' with fd: {:?}", queue_name, mq_fd);
        Ok(mq_fd)
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
        if let Some(fd) = self.send_mq_fd.take() { let _ = mq_close(fd); }
        if let Some(fd) = self.recv_mq_fd.take() { let _ = mq_close(fd); }

        if self.is_creator && !self.base_queue_name.is_empty() {
            let c2s = format!("/{}_c2s", self.base_queue_name);
            let s2c = format!("/{}_s2c", self.base_queue_name);
            let _ = mq_unlink(c2s.as_str());
            let _ = mq_unlink(s2c.as_str());
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
    ///
    /// ## System Integration
    ///
    /// The created queue persists in the system until explicitly unlinked,
    /// allowing clients to connect even if started after the server.
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue server");

        self.base_queue_name = config.message_queue_name.clone();
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = true; // Mark this instance as the creator

        // Open/create the message queue in a blocking task
        let c2s_name = format!("/{}_c2s", self.base_queue_name);
        let s2c_name = format!("/{}_s2c", self.base_queue_name);
        let max_msg_count = self.max_msg_count;
        let max_msg_size = self.max_msg_size;
        
        // Create both queues: client->server (receive), server->client (send)
        let (recv_fd, send_fd) = tokio::task::spawn_blocking(move || {
            let attr = MqAttr::new(0, max_msg_count as i64, max_msg_size as i64, 0);
            let recv_fd = mq_open(
                c2s_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                Mode::from_bits_truncate(0o666),
                Some(&attr),
            ).map_err(|e| anyhow!("Failed to create server recv queue: {}", e))?;
            let send_fd = mq_open(
                s2c_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK,
                Mode::from_bits_truncate(0o666),
                Some(&attr),
            ).map_err(|e| anyhow!("Failed to create server send queue: {}", e))?;
            Ok::<(MqdT, MqdT), anyhow::Error>((recv_fd, send_fd))
        }).await??;

        self.recv_mq_fd = Some(recv_fd);
        self.send_mq_fd = Some(send_fd);
        self.state = TransportState::Connected;
        debug!("POSIX Message Queue server started with queues: /{}_c2s and /{}_s2c", self.base_queue_name, self.base_queue_name);
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

        self.base_queue_name = config.message_queue_name.clone();
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = false; // Mark this instance as a client

        // Open existing message queue with retry logic
        let c2s_name = format!("/{}_c2s", self.base_queue_name);
        let s2c_name = format!("/{}_s2c", self.base_queue_name);
        
        let (recv_fd, send_fd) = tokio::task::spawn_blocking(move || {
            let mut attempts = 0;
            let max_attempts = 30;
            let mut delay_ms = 50;
            let mut open_queue = |name: &str| -> Result<MqdT> {
                loop {
                    match mq_open(name, MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK, Mode::empty(), None) {
                        Ok(fd) => return Ok(fd),
                        Err(Errno::ENOENT) if attempts < max_attempts => {
                            std::thread::sleep(Duration::from_millis(delay_ms));
                            attempts += 1;
                            delay_ms = (delay_ms * 2).min(1000);
                            continue;
                        }
                        Err(e) => return Err(anyhow!("Failed to open queue '{}': {}", name, e)),
                    }
                }
            };
            // Client should receive from server->client (s2c) and send to client->server (c2s)
            let recv_fd = open_queue(s2c_name.as_str())?;
            let send_fd = open_queue(c2s_name.as_str())?;
            Ok::<(MqdT, MqdT), anyhow::Error>((recv_fd, send_fd))
        }).await??;

        self.recv_mq_fd = Some(recv_fd);
        self.send_mq_fd = Some(send_fd);
        self.state = TransportState::Connected;
        debug!("POSIX Message Queue client connected to queues: /{}_c2s and /{}_s2c", self.base_queue_name, self.base_queue_name);
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
    async fn send(&mut self, message: &Message) -> Result<()> {
        let data = message.to_bytes()?;
        let fd_ref = self.send_mq_fd.as_ref().ok_or_else(|| anyhow!("No send queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        
        // Use non-blocking send with exponential backoff for queue-full conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100; // More retries since each one is much faster
        
        for attempt in 0..max_retries {
            let result = tokio::task::spawn_blocking({
                let data = data.clone();
                let raw_fd = raw_fd;
                move || {
                    // Reconstruct MqdT from raw fd for the blocking operation
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    let result = mq_send(&fd, &data, 0);
                    std::mem::forget(fd); // Don't close the fd when this MqdT drops
                    result
                }
            }).await?;
            
            match result {
                Ok(()) => {
                    debug!("Sent message {} bytes via POSIX message queue", data.len());
                    return Ok(());
                }
                Err(Errno::EAGAIN) => {
                    // Queue is full, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!("Send failed after {} attempts - queue consistently full", max_retries));
                    }
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
        let fd_ref = self.recv_mq_fd.as_ref().ok_or_else(|| anyhow!("No recv queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        let max_msg_size = self.max_msg_size;
        
        // Use non-blocking receive with exponential backoff for empty queue conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100;
        
        for attempt in 0..max_retries {
            let result = tokio::task::spawn_blocking({
                let raw_fd = raw_fd;
                let max_msg_size = max_msg_size;
                move || {
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    let mut buffer = vec![0u8; max_msg_size];
                    let mut priority = 0u32;
                    let result = mq_receive(&fd, &mut buffer, &mut priority);
                    std::mem::forget(fd); // Don't close the fd when this MqdT drops
                    result.map(|bytes_read| {
                        buffer.truncate(bytes_read);
                        buffer
                    })
                }
            }).await?;
            
            match result {
                Ok(buffer) => {
                    debug!("Received message {} bytes via POSIX message queue", buffer.len());
                    return Message::from_bytes(&buffer);
                }
                Err(Errno::EAGAIN) => {
                    // Queue is empty, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!("Receive failed after {} attempts - queue consistently empty", max_retries));
                    }
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
        let base = self.base_queue_name.clone();
        let send_fd = self.send_mq_fd.take();
        let recv_fd = self.recv_mq_fd.take();
        let is_creator = self.is_creator;
        
        tokio::task::spawn_blocking(move || {
            if let Some(fd) = send_fd { let _ = mq_close(fd); }
            if let Some(fd) = recv_fd {
                if let Err(e) = mq_close(fd) {
                    warn!("Failed to close message queue: {}", e);
                }
            }
            
            // Only unlink if this instance created the queue
            if is_creator && !base.is_empty() {
                let _ = mq_unlink(format!("/{}_c2s", base).as_str());
                let _ = mq_unlink(format!("/{}_s2c", base).as_str());
            }
        }).await.ok();
        
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
        let fd_ref = self.recv_mq_fd.as_ref().ok_or_else(|| anyhow!("No recv queue available"))?;
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
                        let result = mq_receive(&fd, &mut buffer, &mut priority)
                            .map(|bytes_read| {
                                buffer.truncate(bytes_read);
                                buffer
                            });
                        std::mem::forget(fd); // Don't close the fd when this MqdT drops
                        result
                    }
                }).await;
                
                match result {
                    Ok(Ok(buffer)) => {
                        debug!("Received message {} bytes via POSIX message queue", buffer.len());
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
        self.send(message).await
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