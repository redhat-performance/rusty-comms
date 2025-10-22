//! Blocking POSIX Message Queue transport implementation.
//!
//! This module provides a blocking implementation of IPC using POSIX Message
//! Queues, which are kernel-managed message passing facilities.
//!
//! # Platform Support
//!
//! This implementation is **Linux only** (requires kernel with POSIX mqueue
//! support).
//!
//! # Blocking Behavior
//!
//! All operations block the calling thread:
//! - `mq_open()` with O_CREAT blocks during queue creation
//! - `mq_send()` blocks if queue is full (with timeout)
//! - `mq_receive()` blocks until message available (with timeout)
//!
//! # Queue Protocol
//!
//! Messages are bincode serialized and sent atomically through POSIX message
//! queues. The kernel preserves message boundaries.
//!
//! # System Configuration
//!
//! POSIX Message Queues are subject to system-wide limits:
//! - `/proc/sys/fs/mqueue/msg_max`: Max messages per queue (default: 10)
//! - `/proc/sys/fs/mqueue/msgsize_max`: Max message size (default: 8KB)
//! - `/proc/sys/fs/mqueue/queues_max`: Max number of queues
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::ipc::{
//!     BlockingPosixMessageQueue, BlockingTransport, TransportConfig
//! };
//!
//! # fn example() -> anyhow::Result<()> {
//! let mut server = BlockingPosixMessageQueue::new();
//! let mut config = TransportConfig::default();
//! config.message_queue_name = "/test_pmq".to_string();
//!
//! // Server: create message queue
//! server.start_server_blocking(&config)?;
//!
//! // In another process: client opens queue
//! // let mut client = BlockingPosixMessageQueue::new();
//! // client.start_client_blocking(&config)?;
//! # Ok(())
//! # }
//! ```

use crate::ipc::{BlockingTransport, Message, TransportConfig};
use anyhow::{anyhow, Context, Result};
use nix::errno::Errno;
use nix::mqueue::{mq_close, mq_open, mq_receive, mq_send, mq_unlink, MQ_OFlag, MqAttr, MqdT};
use nix::sys::stat::Mode;
use std::time::Duration;
use tracing::{debug, trace, warn};

/// Blocking POSIX Message Queue transport.
///
/// This struct implements the `BlockingTransport` trait using POSIX message
/// queues with blocking I/O operations.
///
/// # Lifecycle
///
/// 1. Create with `new()`
/// 2. Initialize as server with `start_server_blocking()` (creates queue) OR
///    client with `start_client_blocking()` (opens existing queue)
/// 3. Send/receive messages with `send_blocking()` / `receive_blocking()`
/// 4. Clean up with `close_blocking()`
///
/// # Resource Management
///
/// Only queue creators (servers) unlink queues during cleanup to prevent
/// premature resource deallocation.
pub struct BlockingPosixMessageQueue {
    /// POSIX message queue name (with "/" prefix)
    queue_name: String,

    /// Message queue file descriptor for I/O operations
    mq_fd: Option<MqdT>,

    /// Maximum size of individual messages in bytes
    max_msg_size: usize,

    /// Maximum number of messages that can be queued
    max_msg_count: usize,

    /// Whether this instance created the queue (server)
    is_creator: bool,

    /// Message priority for sends (0-31, higher = higher priority)
    priority: u32,
}

impl BlockingPosixMessageQueue {
    /// Create a new blocking POSIX message queue transport.
    ///
    /// Creates an uninitialized transport. Call `start_server_blocking()` or
    /// `start_client_blocking()` to initialize it.
    ///
    /// # Default Configuration
    ///
    /// - **Message Size**: 8KB (typical default)
    /// - **Queue Depth**: 10 messages (typical default)
    /// - **Priority**: 0 (lowest priority)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::ipc::BlockingPosixMessageQueue;
    ///
    /// let transport = BlockingPosixMessageQueue::new();
    /// // Now call start_server_blocking() or start_client_blocking()
    /// ```
    pub fn new() -> Self {
        Self {
            queue_name: String::new(),
            mq_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_creator: false,
            priority: 0,
        }
    }

    /// Clean up message queue resources.
    ///
    /// Closes the queue file descriptor and unlinks the queue if this
    /// instance created it (server). This method is idempotent.
    ///
    /// # Resource Ownership
    ///
    /// Only queue creators (servers) unlink queues to prevent race
    /// conditions and premature resource destruction.
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
                Ok(_) => debug!("Unlinked message queue: {}", self.queue_name),
                Err(e) => warn!("Failed to unlink message queue: {}", e),
            }
        }
    }
}

impl BlockingTransport for BlockingPosixMessageQueue {
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking POSIX message queue server: {}",
            config.message_queue_name
        );

        // Ensure queue name starts with "/"
        let queue_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };

        self.queue_name = queue_name.clone();
        self.priority = config.pmq_priority;

        // Set queue attributes
        let attrs = MqAttr::new(
            0,                         // flags (not used for create)
            self.max_msg_count as i64, // max messages
            self.max_msg_size as i64,  // max message size
            0,                         // current messages (ignored)
        );

        // Create the message queue
        // O_CREAT | O_EXCL | O_RDWR: Create new queue, fail if exists, read-write
        let flags = MQ_OFlag::O_CREAT | MQ_OFlag::O_EXCL | MQ_OFlag::O_RDWR;
        let mode = Mode::S_IRUSR | Mode::S_IWUSR; // 0600 permissions

        let fd = mq_open(queue_name.as_str(), flags, mode, Some(&attrs)).with_context(|| {
            format!(
                "Failed to create POSIX message queue: {}. \
                 Queue may already exist or system limits may be reached. \
                 Check /proc/sys/fs/mqueue/ for limits.",
                queue_name
            )
        })?;

        self.mq_fd = Some(fd);
        self.is_creator = true;

        debug!("POSIX message queue server created: {}", queue_name);
        Ok(())
    }

    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking POSIX message queue client, connecting to: {}",
            config.message_queue_name
        );

        // Ensure queue name starts with "/"
        let queue_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };

        self.queue_name = queue_name.clone();
        self.priority = config.pmq_priority;

        // Open existing message queue (retry with timeout)
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let flags = MQ_OFlag::O_RDWR; // Read-write, no create

        let fd = loop {
            match mq_open(queue_name.as_str(), flags, Mode::empty(), None) {
                Ok(fd) => break fd,
                Err(Errno::ENOENT) => {
                    // Queue doesn't exist yet
                    if start.elapsed() > timeout {
                        return Err(anyhow!(
                            "Failed to open POSIX message queue: {}. \
                             Queue does not exist. Is the server running?",
                            queue_name
                        ));
                    }
                    // Wait a bit and retry
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    return Err(anyhow!(
                        "Failed to open POSIX message queue: {}. Error: {}",
                        queue_name,
                        e
                    ));
                }
            }
        };

        self.mq_fd = Some(fd);
        self.is_creator = false;

        debug!("Client connected to POSIX message queue: {}", queue_name);
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!(
            "Sending message ID {} via blocking POSIX message queue",
            message.id
        );

        let fd = self.mq_fd.as_ref().ok_or_else(|| {
            anyhow!(
                "Cannot send: message queue not initialized. \
                 Call start_server_blocking() or start_client_blocking() first."
            )
        })?;

        // Serialize message
        let serialized = bincode::serialize(message).context("Failed to serialize message")?;

        if serialized.len() > self.max_msg_size {
            return Err(anyhow!(
                "Message size {} exceeds maximum {}",
                serialized.len(),
                self.max_msg_size
            ));
        }

        // Send message (blocks until space available or timeout)
        // Use a retry loop with timeouts since mq_send can fail with EAGAIN
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(5);

        loop {
            match mq_send(fd, &serialized, self.priority) {
                Ok(()) => {
                    trace!("Message ID {} sent successfully", message.id);
                    return Ok(());
                }
                Err(Errno::EAGAIN) => {
                    // Queue is full
                    if start.elapsed() > timeout {
                        return Err(anyhow!(
                            "Timeout: message queue full, possible backpressure"
                        ));
                    }
                    // Yield and retry
                    std::thread::yield_now();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to send message: {}", e));
                }
            }
        }
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via blocking POSIX message queue");

        let fd = self.mq_fd.as_ref().ok_or_else(|| {
            anyhow!(
                "Cannot receive: message queue not initialized. \
                 Call start_server_blocking() or start_client_blocking() first."
            )
        })?;

        // Allocate buffer for receiving
        let mut buffer = vec![0u8; self.max_msg_size];

        // Receive message (blocks until message available)
        // Use a retry loop for EAGAIN errors
        let data = loop {
            let mut priority = 0u32;
            match mq_receive(fd, &mut buffer, &mut priority) {
                Ok(size) => {
                    // Truncate buffer to actual message size
                    buffer.truncate(size);
                    break buffer;
                }
                Err(Errno::EAGAIN) => {
                    // No message available (shouldn't happen in blocking mode)
                    std::thread::yield_now();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    return Err(anyhow!("Failed to receive message: {}", e));
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
        debug!("Closing blocking POSIX message queue transport");
        self.cleanup_queue();
        debug!("Blocking POSIX message queue transport closed");
        Ok(())
    }
}

// Implement Default for convenience
impl Default for BlockingPosixMessageQueue {
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

    fn make_unique_queue_name(test_name: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        format!("/test_pmq_blocking_{}_{}", test_name, timestamp)
    }

    #[test]
    fn test_new_creates_empty_transport() {
        let transport = BlockingPosixMessageQueue::new();
        assert!(transport.queue_name.is_empty());
        assert!(transport.mq_fd.is_none());
        assert!(!transport.is_creator);
        assert_eq!(transport.max_msg_size, 8192);
        assert_eq!(transport.max_msg_count, 10);
    }

    #[test]
    fn test_server_creates_queue_successfully() {
        let queue_name = make_unique_queue_name("server");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name.clone(),
            ..Default::default()
        };

        let result = server.start_server_blocking(&config);
        assert!(result.is_ok(), "Server should create queue successfully");

        // Cleanup
        server.close_blocking().unwrap();
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        let queue_name = make_unique_queue_name("no_server");

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };

        // This should timeout since no server exists
        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        // Note: This test may take 30 seconds due to timeout
    }

    #[test]
    fn test_send_and_receive_message() {
        let queue_name = make_unique_queue_name("send_recv");

        // Start server in thread
        let server_queue = queue_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
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
        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
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
    fn test_round_trip_communication() {
        let queue_name = make_unique_queue_name("round_trip");

        // Start server
        let server_queue = queue_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
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
        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
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
        let queue_name = make_unique_queue_name("close");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();

        // Close immediately
        server.close_blocking().unwrap();

        // Verify fields are None after close
        assert!(server.mq_fd.is_none());
    }
}
