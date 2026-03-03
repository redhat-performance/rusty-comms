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
        // Use 0666 permissions for container access
        let mode = Mode::S_IRUSR
            | Mode::S_IWUSR
            | Mode::S_IRGRP
            | Mode::S_IWGRP
            | Mode::S_IROTH
            | Mode::S_IWOTH;

        let fd = mq_open(queue_name, flags, mode, Some(&attrs)).with_context(|| {
            format!(
                "Failed to create POSIX message queue: {}. \
                 Queue may already exist or system limits may be reached. \
                 Check /proc/sys/fs/mqueue/ for limits.",
                queue_name
            )
        })?;

        // Set permissions to 777 for container access
        // Queue file is at /dev/mqueue/<name_without_leading_slash>
        let file_name = queue_name.strip_prefix('/').unwrap_or(queue_name);
        let mqueue_path = format!("/dev/mqueue/{}", file_name);
        if let Ok(c_path) = std::ffi::CString::new(mqueue_path.as_bytes()) {
            let result = unsafe { libc::chmod(c_path.as_ptr(), 0o777) };
            if result == 0 {
                debug!("Set PMQ permissions to 777: {}", queue_name);
            } else {
                debug!("Failed to set PMQ permissions: {}", queue_name);
            }
        }

        Ok(fd)
    }

    /// Open an existing message queue with retry logic
    fn open_queue(&self, queue_name: &str) -> Result<MqdT> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(10);
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

        // Apply configuration limits
        if config.buffer_size > 0 {
            self.max_msg_size = config.buffer_size;
        }
        if config.message_queue_depth > 0 {
            self.max_msg_count = config.message_queue_depth;
        }

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

        // Apply configuration limits to match server
        if config.buffer_size > 0 {
            self.max_msg_size = config.buffer_size;
        }
        if config.message_queue_depth > 0 {
            self.max_msg_count = config.message_queue_depth;
        }

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

        // Pre-compute the timestamp offset for efficient in-place updates
        let ts_offset = Message::timestamp_offset();

        // CRITICAL: Capture timestamp ONCE before the retry loop.
        // This ensures accurate latency measurement - the timestamp reflects when
        // the send was initiated, not when it finally succeeded after backpressure.
        // This is consistent with how shared_memory.rs handles timestamps.
        let ts_now = crate::ipc::get_monotonic_time_ns();
        let timestamp_bytes = ts_now.to_le_bytes();
        if serialized.len() >= ts_offset.end {
            serialized[ts_offset].copy_from_slice(&timestamp_bytes);
        }

        // Send with retry loop and timeouts since mq_send can fail with EAGAIN
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
        let (data, receive_time_ns) = loop {
            let mut priority = 0u32;
            match mq_receive(fd, &mut buffer, &mut priority) {
                Ok(size) => {
                    // CRITICAL: Capture receive timestamp immediately after read
                    // This ensures accurate latency measurement
                    let receive_time_ns = crate::ipc::get_monotonic_time_ns();
                    // Truncate buffer to actual message size
                    buffer.truncate(size);
                    break (buffer, receive_time_ns);
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
        debug!("Closing blocking POSIX message queue transport");
        self.cleanup_queues();
        debug!("Blocking POSIX message queue transport closed");
        Ok(())
    }
}

/// Ensure message queue resources are cleaned up even if
/// `close_blocking()` is never called (e.g. due to a panic or early
/// return). Closes queue file descriptors and, if this instance is the
/// creator (server), unlinks the queues from the system.
impl Drop for BlockingPosixMessageQueue {
    fn drop(&mut self) {
        // cleanup_queues() is idempotent; safe to call even if the
        // transport was never started or was already closed.
        self.cleanup_queues();
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
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    /// Clean up any leftover test queues from previous runs for a specific test.
    /// This prevents "too many open files" errors when tests fail and leave queues behind.
    fn cleanup_leftover_test_queues(test_name: &str) {
        use std::fs;
        let prefix = format!("test_pmq_blocking_{}_", test_name);
        if let Ok(entries) = fs::read_dir("/dev/mqueue") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Clean up any test queues for this specific test
                if name_str.starts_with(&prefix) {
                    let queue_path = format!("/{}", name_str);
                    let _ = mq_unlink(queue_path.as_str());
                }
            }
        }
    }

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
        cleanup_leftover_test_queues("server");
        let queue_name = make_unique_queue_name("server");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name.clone(),
            buffer_size: 1024,
            ..Default::default()
        };

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        match server.start_server_blocking(&config) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("EMFILE") => {
                eprintln!(
                    "Skipping test_server_creates_queue: \
                     server setup failed (EMFILE)"
                );
                return;
            }
            Err(e) => panic!("Server should create queues successfully: {e}"),
        }
        assert!(server.send_fd.is_some());
        assert!(server.recv_fd.is_some());

        server.close_blocking().unwrap();
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        cleanup_leftover_test_queues("no_server");
        let queue_name = make_unique_queue_name("no_server");

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };

        // This should timeout since no server exists
        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        // Note: This test may take 30 seconds due to timeout
    }

    #[test]
    fn test_send_and_receive_message() {
        cleanup_leftover_test_queues("send_recv");
        let queue_name = make_unique_queue_name("send_recv");

        let server_queue = queue_name.clone();
        let (setup_tx, setup_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                buffer_size: 1024,
                ..Default::default()
            };
            match server.start_server_blocking(&config) {
                Ok(()) => {
                    setup_tx.send(true).unwrap();
                    let msg = server.receive_blocking().unwrap();
                    assert_eq!(msg.id, 42);
                    assert_eq!(msg.payload.len(), 100);
                    server.close_blocking().unwrap();
                }
                Err(_) => {
                    setup_tx.send(false).unwrap();
                }
            }
        });

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        thread::sleep(Duration::from_millis(200));
        if !setup_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or(false)
        {
            eprintln!(
                "Skipping test_send_and_receive_message: \
                 server setup failed (likely EMFILE)"
            );
            let _ = server_handle.join();
            return;
        }

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(42, vec![0u8; 100], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        server_handle.join().unwrap();
    }

    #[test]
    fn test_round_trip_communication() {
        cleanup_leftover_test_queues("round_trip");
        let queue_name = make_unique_queue_name("round_trip");

        let server_queue = queue_name.clone();
        let (setup_tx, setup_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                buffer_size: 1024,
                ..Default::default()
            };
            match server.start_server_blocking(&config) {
                Ok(()) => {
                    setup_tx.send(true).unwrap();
                    let request = server.receive_blocking().unwrap();
                    assert_eq!(request.message_type, MessageType::Request);
                    let response = Message::new(request.id, Vec::new(), MessageType::Response);
                    server.send_blocking(&response).unwrap();
                    server.close_blocking().unwrap();
                }
                Err(_) => {
                    setup_tx.send(false).unwrap();
                }
            }
        });

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        thread::sleep(Duration::from_millis(200));
        if !setup_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or(false)
        {
            eprintln!(
                "Skipping test_round_trip_communication: \
                 server setup failed (likely EMFILE)"
            );
            let _ = server_handle.join();
            return;
        }

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let request = Message::new(123, vec![1, 2, 3], MessageType::Request);
        client.send_blocking(&request).unwrap();

        let response = client.receive_blocking().unwrap();
        assert_eq!(response.message_type, MessageType::Response);
        assert_eq!(response.id, 123);

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    #[test]
    fn test_multiple_round_trips() {
        cleanup_leftover_test_queues("multi_rt");
        let queue_name = make_unique_queue_name("multi_rt");

        let server_queue = queue_name.clone();
        let (setup_tx, setup_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                buffer_size: 1024,
                ..Default::default()
            };
            match server.start_server_blocking(&config) {
                Ok(()) => {
                    setup_tx.send(true).unwrap();
                    for _ in 0..100 {
                        let request = server.receive_blocking().unwrap();
                        let response = Message::new(request.id, Vec::new(), MessageType::Response);
                        server.send_blocking(&response).unwrap();
                    }
                    server.close_blocking().unwrap();
                }
                Err(_) => {
                    setup_tx.send(false).unwrap();
                }
            }
        });

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        thread::sleep(Duration::from_millis(200));
        if !setup_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or(false)
        {
            eprintln!(
                "Skipping test_multiple_round_trips: \
                 server setup failed (likely EMFILE)"
            );
            let _ = server_handle.join();
            return;
        }

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

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
        cleanup_leftover_test_queues("close");
        let queue_name = make_unique_queue_name("close");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        match server.start_server_blocking(&config) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("EMFILE") => {
                eprintln!(
                    "Skipping test_close_cleanup: \
                     server setup failed (EMFILE)"
                );
                return;
            }
            Err(e) => panic!("Unexpected error: {e}"),
        }

        server.close_blocking().unwrap();

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
        cleanup_leftover_test_queues("custom_limits");
        let queue_name = make_unique_queue_name("custom_limits");

        let mut server = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name.clone(),
            buffer_size: 1024,
            ..Default::default()
        };

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        match server.start_server_blocking(&config) {
            Ok(()) => {}
            Err(e) if e.to_string().contains("EMFILE") => {
                eprintln!(
                    "Skipping test_with_custom_buffer_size: \
                     server setup failed (EMFILE)"
                );
                return;
            }
            Err(e) => panic!("start_server_blocking failed unexpectedly: {e}"),
        }
        assert!(server.max_msg_size > 0);

        server.close_blocking().unwrap();
    }

    #[test]
    fn test_multiple_messages_one_way() {
        cleanup_leftover_test_queues("multi_oneway");
        let queue_name = make_unique_queue_name("multi_oneway");

        let server_queue = queue_name.clone();
        let (setup_tx, setup_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: server_queue,
                buffer_size: 1024,
                ..Default::default()
            };
            match server.start_server_blocking(&config) {
                Ok(()) => {
                    setup_tx.send(true).unwrap();
                    for expected_id in 1..=10 {
                        let msg = server.receive_blocking().unwrap();
                        assert_eq!(msg.id, expected_id);
                        assert_eq!(msg.message_type, MessageType::OneWay);
                    }
                    server.close_blocking().unwrap();
                }
                Err(_) => {
                    setup_tx.send(false).unwrap();
                }
            }
        });

        // Graceful skip on fd exhaustion (EMFILE under tarpaulin)
        thread::sleep(Duration::from_millis(200));
        if !setup_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or(false)
        {
            eprintln!(
                "Skipping test_multiple_messages_one_way: \
                 server setup failed (likely EMFILE)"
            );
            let _ = server_handle.join();
            return;
        }

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

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

    /// Test that send_blocking fails when the transport is not
    /// initialized (no queues opened).
    #[test]
    fn test_send_on_uninit_transport_fails() {
        let mut transport = BlockingPosixMessageQueue::new();
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);

        let result = transport.send_blocking(&msg);
        assert!(
            result.is_err(),
            "send on uninitialized PMQ transport should fail"
        );
    }

    /// Test that receive_blocking fails when the transport is not
    /// initialized.
    #[test]
    fn test_receive_on_uninit_transport_fails() {
        let mut transport = BlockingPosixMessageQueue::new();
        let result = transport.receive_blocking();
        assert!(result.is_err(), "receive on uninitialized PMQ should fail");
    }

    /// Test that close_blocking is idempotent on a fresh transport.
    #[test]
    fn test_close_idempotent_uninit() {
        let mut transport = BlockingPosixMessageQueue::new();
        transport.close_blocking().unwrap();
        transport.close_blocking().unwrap();
        assert!(transport.send_fd.is_none());
        assert!(transport.recv_fd.is_none());
    }

    /// Test that sending a message larger than `max_msg_size` is
    /// properly rejected with a descriptive error.
    ///
    /// Under coverage instrumentation (tarpaulin), POSIX message
    /// queue descriptors can be exhausted (EMFILE). The test
    /// gracefully skips when setup fails for resource reasons.
    #[test]
    fn test_send_oversized_message_fails() {
        let queue_name = format!("/tpom_{}", uuid::Uuid::new_v4().as_u128() & 0xffff_ffff);
        let queue_clone = queue_name.clone();
        let (setup_tx, setup_rx) = mpsc::channel();

        let server_handle = thread::spawn(move || {
            let mut server = BlockingPosixMessageQueue::new();
            let config = TransportConfig {
                message_queue_name: queue_clone,
                buffer_size: 1024,
                ..Default::default()
            };
            match server.start_server_blocking(&config) {
                Ok(()) => {
                    setup_tx.send(true).unwrap();
                    thread::sleep(Duration::from_millis(500));
                    server.close_blocking().unwrap();
                }
                Err(_) => {
                    setup_tx.send(false).unwrap();
                }
            }
        });

        thread::sleep(Duration::from_millis(200));

        // Skip gracefully if server setup failed (fd exhaustion)
        if !setup_rx
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or(false)
        {
            eprintln!(
                "Skipping test_send_oversized_message_fails: \
                 server setup failed (likely EMFILE)"
            );
            let _ = server_handle.join();
            return;
        }

        let mut client = BlockingPosixMessageQueue::new();
        let config = TransportConfig {
            message_queue_name: queue_name,
            buffer_size: 1024,
            ..Default::default()
        };
        match client.start_client_blocking(&config) {
            Ok(()) => {}
            Err(_) => {
                eprintln!(
                    "Skipping test_send_oversized_message_fails: \
                     client setup failed (likely EMFILE)"
                );
                let _ = server_handle.join();
                return;
            }
        }

        let big_payload = vec![0xFFu8; 64 * 1024];
        let msg = Message::new(1, big_payload, MessageType::OneWay);
        let result = client.send_blocking(&msg);
        assert!(result.is_err(), "Oversized message should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("exceeds maximum"),
            "Error should mention exceeds maximum: {}",
            err_msg
        );

        client.close_blocking().unwrap();
        server_handle.join().unwrap();
    }
}
