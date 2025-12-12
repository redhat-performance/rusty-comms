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
//! # Two-Queue Architecture
//!
//! This implementation uses **two separate queues** for bidirectional communication
//! to avoid race conditions in round-trip scenarios:
//!
//! ```text
//! Client → [Request Queue]  → Server
//! Client ← [Response Queue] ← Server
//! ```
//!
//! This ensures reliable round-trip communication where the server cannot
//! accidentally receive its own responses.
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
//! // Server: create message queues (creates /test_pmq_req and /test_pmq_resp)
//! server.start_server_blocking(&config)?;
//!
//! // In another process: client opens queues
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
/// # Two-Queue Design
///
/// Uses separate queues for each direction to prevent race conditions:
/// - **Request Queue** (`{name}_req`): Client sends, Server receives
/// - **Response Queue** (`{name}_resp`): Server sends, Client receives
///
/// # Lifecycle
///
/// 1. Create with `new()`
/// 2. Initialize as server with `start_server_blocking()` (creates both queues) OR
///    client with `start_client_blocking()` (opens both queues)
/// 3. Send/receive messages with `send_blocking()` / `receive_blocking()`
/// 4. Clean up with `close_blocking()`
///
/// # Resource Management
///
/// Only queue creators (servers) unlink queues during cleanup to prevent
/// premature resource deallocation.
pub struct BlockingPosixMessageQueue {
    /// Base POSIX message queue name (with "/" prefix)
    queue_name_base: String,

    /// Message queue file descriptor for sending
    send_fd: Option<MqdT>,

    /// Message queue file descriptor for receiving
    recv_fd: Option<MqdT>,

    /// Maximum size of individual messages in bytes
    max_msg_size: usize,

    /// Maximum number of messages that can be queued
    max_msg_count: usize,

    /// Whether this instance created the queues (server)
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
            queue_name_base: String::new(),
            send_fd: None,
            recv_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_creator: false,
            priority: 0,
        }
    }

    /// Get the request queue name (client→server)
    fn request_queue_name(&self) -> String {
        format!("{}_req", self.queue_name_base)
    }

    /// Get the response queue name (server→client)
    fn response_queue_name(&self) -> String {
        format!("{}_resp", self.queue_name_base)
    }

    /// Clean up message queue resources.
    ///
    /// Closes both queue file descriptors and unlinks the queues if this
    /// instance created them (server). This method is idempotent.
    ///
    /// # Resource Ownership
    ///
    /// Only queue creators (servers) unlink queues to prevent race
    /// conditions and premature resource destruction.
    fn cleanup_queues(&mut self) {
        debug!("Cleaning up POSIX message queues");

        // Close send queue
        if let Some(fd) = self.send_fd.take() {
            debug!("Closing send queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close send queue: {}", e);
            } else {
                debug!("Closed send queue");
            }
        }

        // Close receive queue
        if let Some(fd) = self.recv_fd.take() {
            debug!("Closing receive queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close receive queue: {}", e);
            } else {
                debug!("Closed receive queue");
            }
        }

        // Only unlink the queues if this instance created them (server)
        if self.is_creator && !self.queue_name_base.is_empty() {
            let req_name = self.request_queue_name();
            let resp_name = self.response_queue_name();

            match mq_unlink(req_name.as_str()) {
                Ok(_) => debug!("Unlinked request queue: {}", req_name),
                Err(e) => warn!("Failed to unlink request queue: {}", e),
            }

            match mq_unlink(resp_name.as_str()) {
                Ok(_) => debug!("Unlinked response queue: {}", resp_name),
                Err(e) => warn!("Failed to unlink response queue: {}", e),
            }
        }
    }

    /// Create a single message queue with the given name
    fn create_queue(&self, queue_name: &str) -> Result<MqdT> {
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

        mq_open(queue_name, flags, mode, Some(&attrs)).with_context(|| {
            format!(
                "Failed to create POSIX message queue: {}. \
                 Queue may already exist or system limits may be reached. \
                 Check /proc/sys/fs/mqueue/ for limits.",
                queue_name
            )
        })
    }

    /// Open an existing message queue with retry logic
    fn open_queue(&self, queue_name: &str) -> Result<MqdT> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(30);
        let flags = MQ_OFlag::O_RDWR; // Read-write, no create

        loop {
            match mq_open(queue_name, flags, Mode::empty(), None) {
                Ok(fd) => return Ok(fd),
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
        let base_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };

        self.queue_name_base = base_name;
        self.priority = config.pmq_priority;

        let req_name = self.request_queue_name();
        let resp_name = self.response_queue_name();

        // Try to clean up any existing queues (from previous runs)
        // Best-effort cleanup - ignore errors
        let _ = mq_unlink(req_name.as_str());
        let _ = mq_unlink(resp_name.as_str());

        // Create both queues
        // Server receives from request queue, sends to response queue
        let recv_fd = self.create_queue(&req_name)?;
        debug!("Created request queue: {}", req_name);

        let send_fd = self.create_queue(&resp_name)?;
        debug!("Created response queue: {}", resp_name);

        self.recv_fd = Some(recv_fd);
        self.send_fd = Some(send_fd);
        self.is_creator = true;

        debug!(
            "POSIX message queue server created with queues: {}, {}",
            req_name, resp_name
        );
        Ok(())
    }

    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking POSIX message queue client, connecting to: {}",
            config.message_queue_name
        );

        // Ensure queue name starts with "/"
        let base_name = if config.message_queue_name.starts_with('/') {
            config.message_queue_name.clone()
        } else {
            format!("/{}", config.message_queue_name)
        };

        self.queue_name_base = base_name;
        self.priority = config.pmq_priority;

        let req_name = self.request_queue_name();
        let resp_name = self.response_queue_name();

        // Open both queues
        // Client sends to request queue, receives from response queue
        let send_fd = self.open_queue(&req_name)?;
        debug!("Opened request queue for sending: {}", req_name);

        let recv_fd = self.open_queue(&resp_name)?;
        debug!("Opened response queue for receiving: {}", resp_name);

        self.send_fd = Some(send_fd);
        self.recv_fd = Some(recv_fd);
        self.is_creator = false;

        debug!(
            "Client connected to POSIX message queues: {}, {}",
            req_name, resp_name
        );
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!(
            "Sending message ID {} via blocking POSIX message queue",
            message.id
        );

        let fd = self.send_fd.as_ref().ok_or_else(|| {
            anyhow!(
                "Cannot send: message queue not initialized. \
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

        if serialized.len() > self.max_msg_size {
            return Err(anyhow!(
                "Message size {} exceeds maximum {}",
                serialized.len(),
                self.max_msg_size
            ));
        }

        // Capture timestamp immediately before send and update bytes in buffer
        message_with_timestamp.set_timestamp_now();
        let timestamp_bytes = message_with_timestamp.timestamp.to_le_bytes();
        let ts_offset = Message::timestamp_offset();
        serialized[ts_offset].copy_from_slice(&timestamp_bytes);

        // Send immediately - no intervening work
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

        let fd = self.recv_fd.as_ref().ok_or_else(|| {
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
        self.cleanup_queues();
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
        assert!(transport.queue_name_base.is_empty());
        assert!(transport.send_fd.is_none());
        assert!(transport.recv_fd.is_none());
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
        assert!(result.is_ok(), "Server should create queues successfully");
        assert!(server.send_fd.is_some());
        assert!(server.recv_fd.is_some());

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
    fn test_multiple_round_trips() {
        // Test that multiple rapid round-trips work without race conditions
        let queue_name = make_unique_queue_name("multi_rt");

        let server_queue = queue_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Handle 100 round-trips
            for _ in 0..100 {
                let request = server.receive_blocking().unwrap();
                let response = Message::new(request.id, Vec::new(), MessageType::Response);
                server.send_blocking(&response).unwrap();
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Perform 100 round-trips
        for i in 0..100u64 {
            let request = Message::new(i, vec![1, 2, 3], MessageType::Request);
            client.send_blocking(&request).unwrap();
            let response = client.receive_blocking().unwrap();
            assert_eq!(response.id, i);
            assert_eq!(response.message_type, MessageType::Response);
        }

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
        assert!(server.send_fd.is_none());
        assert!(server.recv_fd.is_none());
    }

    #[test]
    fn test_default_creates_new_transport() {
        let transport = BlockingPosixMessageQueue::default();
        assert!(transport.queue_name_base.is_empty());
        assert!(transport.send_fd.is_none());
        assert!(transport.recv_fd.is_none());
        assert!(!transport.is_creator);
    }

    #[test]
    fn test_with_custom_buffer_size() {
        let queue_name = make_unique_queue_name("custom_limits");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name.clone(),
            buffer_size: 4096,
            ..Default::default()
        };

        let result = server.start_server_blocking(&config);
        assert!(result.is_ok());
        // max_msg_size should be set based on buffer_size
        assert!(server.max_msg_size > 0);

        server.close_blocking().unwrap();
    }

    #[test]
    fn test_multiple_messages_one_way() {
        let queue_name = make_unique_queue_name("multi_oneway");

        let server_queue = queue_name.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive 10 one-way messages
            for expected_id in 1..=10 {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, expected_id);
                assert_eq!(msg.message_type, MessageType::OneWay);
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(200));

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send 10 one-way messages
        for id in 1..=10 {
            let msg = Message::new(id, vec![0u8; 64], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    #[test]
    fn test_client_close_without_connect() {
        let mut client = BlockingPosixMessageQueue::new();
        // Close without ever connecting should succeed
        let result = client.close_blocking();
        assert!(result.is_ok());
    }
}
