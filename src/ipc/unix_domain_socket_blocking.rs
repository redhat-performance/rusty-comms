//! Blocking Unix Domain Socket transport implementation.
//!
//! This module provides a blocking implementation of the Unix Domain Socket
//! (UDS) transport using standard library types from `std::os::unix::net`.
//!
//! # Platform Support
//!
//! This implementation is only available on Unix platforms (Linux, macOS,
//! BSD). It will not compile on Windows.
//!
//! # Blocking Behavior
//!
//! All operations in this module block the calling thread:
//! - `bind()` and `listen()` block during socket setup
//! - `accept()` blocks until a client connects
//! - `connect()` blocks until connection is established or timeout
//! - `send()` blocks until message is written to OS buffers
//! - `recv()` blocks until message is available or connection closes
//!
//! # Wire Protocol
//!
//! Messages are sent with a simple length-prefixed protocol:
//! 1. Send 4-byte message length (u32, little-endian)
//! 2. Send serialized message bytes (bincode format)
//!
//! This matches the protocol used by the async UDS transport for
//! consistency.
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::ipc::{
//!     BlockingUnixDomainSocket, BlockingTransport, TransportConfig
//! };
//! use ipc_benchmark::get_temp_socket_path;
//!
//! # fn example() -> anyhow::Result<()> {
//! let mut server = BlockingUnixDomainSocket::new();
//! let mut config = TransportConfig::default();
//! config.socket_path = get_temp_socket_path("test.sock");
//!
//! // Server: bind and wait for connection
//! server.start_server_blocking(&config)?;
//!
//! // In another thread/process: client connects
//! // let mut client = BlockingUnixDomainSocket::new();
//! // client.start_client_blocking(&config)?;
//! # Ok(())
//! # }
//! ```

use crate::ipc::{BlockingTransport, Message, TransportConfig};
use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use tracing::{debug, trace};

/// Blocking Unix Domain Socket transport.
///
/// This struct implements the `BlockingTransport` trait using Unix Domain
/// Sockets with standard library blocking I/O operations.
///
/// # Fields
///
/// - `listener`: The server socket listener (only used in server mode)
/// - `stream`: The connected socket stream (used in both client and server
///   mode)
///
/// # Lifecycle
///
/// 1. Create with `new()`
/// 2. Initialize as server with `start_server_blocking()` OR client with
///    `start_client_blocking()`
/// 3. Send/receive messages with `send_blocking()` / `receive_blocking()`
/// 4. Clean up with `close_blocking()`
pub struct BlockingUnixDomainSocket {
    /// Server listener socket.
    /// Only populated in server mode, None in client mode.
    listener: Option<UnixListener>,

    /// Connected socket stream for sending/receiving data.
    /// Populated after accept() in server mode or connect() in client mode.
    stream: Option<UnixStream>,

    /// Socket file path for cleanup on close/drop.
    /// Only populated in server mode (the server owns the socket file).
    socket_path: Option<String>,
}

impl BlockingUnixDomainSocket {
    /// Maximum accepted frame size (matches async transport guard).
    const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

    /// Create a new Unix Domain Socket transport.
    ///
    /// Creates an uninitialized transport. Call `start_server_blocking()` or
    /// `start_client_blocking()` to initialize it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::ipc::BlockingUnixDomainSocket;
    ///
    /// let transport = BlockingUnixDomainSocket::new();
    /// // Now call start_server_blocking() or start_client_blocking()
    /// ```
    pub fn new() -> Self {
        Self {
            listener: None,
            stream: None,
            socket_path: None,
        }
    }

    /// Remove the socket file if it exists.
    /// Called on close and drop to prevent stale socket files.
    fn cleanup_socket(&self) {
        if let Some(ref path) = self.socket_path {
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("Failed to remove socket file {}: {}", path, e);
                }
            }
        }
    }

    /// Accept a connection if we haven't already.
    /// This is called automatically on first send/receive in server mode.
    fn ensure_connection(&mut self) -> Result<()> {
        // If we already have a stream, we're connected
        if self.stream.is_some() {
            return Ok(());
        }

        // If we have a listener, accept a connection
        if let Some(ref listener) = self.listener {
            debug!("Accepting connection on UDS server");
            let (stream, addr) = listener
                .accept()
                .context("Failed to accept connection on Unix domain socket")?;

            // Optimize socket buffer sizes for lower latency
            Self::configure_socket_buffers(&stream);

            debug!("UDS server accepted connection from: {:?}", addr);
            self.stream = Some(stream);
            Ok(())
        } else {
            // Not a server or already connected - this is fine
            Ok(())
        }
    }

    /// Configure socket buffer sizes for optimal latency.
    /// Smaller buffers can reduce latency by avoiding batching delays.
    #[cfg(unix)]
    fn configure_socket_buffers(stream: &UnixStream) {
        use libc::{setsockopt, socklen_t, SOL_SOCKET, SO_RCVBUF, SO_SNDBUF};
        use std::mem::size_of;

        let fd = stream.as_raw_fd();
        // Use smaller buffers (64KB) to reduce batching delay
        // Default is often 128KB+ which can add latency
        let buf_size: libc::c_int = 65536;

        unsafe {
            // Set send buffer size
            let _ = setsockopt(
                fd,
                SOL_SOCKET,
                SO_SNDBUF,
                &buf_size as *const _ as *const libc::c_void,
                size_of::<libc::c_int>() as socklen_t,
            );
            // Set receive buffer size
            let _ = setsockopt(
                fd,
                SOL_SOCKET,
                SO_RCVBUF,
                &buf_size as *const _ as *const libc::c_void,
                size_of::<libc::c_int>() as socklen_t,
            );
        }
        trace!("Configured UDS socket buffers to {} bytes", buf_size);
    }
}

impl BlockingTransport for BlockingUnixDomainSocket {
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting blocking UDS server at: {}", config.socket_path);

        // Remove existing socket file if present to avoid "address in use"
        // errors from previous runs. Ignore errors (file might not exist).
        let _ = std::fs::remove_file(&config.socket_path);

        // Create and bind the listener socket
        let listener = UnixListener::bind(&config.socket_path).with_context(|| {
            format!(
                "Failed to bind Unix domain socket at: {}. \
                     Check permissions and path validity.",
                config.socket_path
            )
        })?;

        debug!("UDS server bound successfully");

        // Set socket permissions to 777 so host can connect when running in container.
        // This is needed because rootless Podman remaps user IDs, making the socket
        // inaccessible to the host user without world-writable permissions.
        #[cfg(unix)]
        {
            use std::ffi::CString;
            if let Ok(c_path) = CString::new(config.socket_path.as_bytes()) {
                let result = unsafe { libc::chmod(c_path.as_ptr(), 0o777) };
                if result == 0 {
                    debug!("Set socket permissions to 777");
                } else {
                    debug!("Failed to set socket permissions: errno={}", unsafe {
                        *libc::__errno_location()
                    });
                }
            }
        }

        // Store the listener and the path (for cleanup on close/drop).
        // The accept() will happen on the first send/receive via ensure_connection().
        // This allows the server to signal readiness before blocking on accept.
        self.listener = Some(listener);
        self.socket_path = Some(config.socket_path.clone());

        Ok(())
    }

    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting blocking UDS client, connecting to: {}",
            config.socket_path
        );

        // Connect to server socket (blocks until connected or fails)
        let stream = UnixStream::connect(&config.socket_path).with_context(|| {
            format!(
                "Failed to connect to Unix domain socket at: {}. \
                     Is the server running?",
                config.socket_path
            )
        })?;

        // Optimize socket buffer sizes for lower latency
        Self::configure_socket_buffers(&stream);

        debug!("UDS client connected successfully");

        self.stream = Some(stream);
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!("Sending message ID {} via blocking UDS", message.id);

        // Ensure we have a connection (accept if server, no-op if client)
        self.ensure_connection()?;

        let stream = self.stream.as_mut().context(
            "Cannot send: socket not connected. \
                 Call start_server_blocking() or start_client_blocking() first.",
        )?;

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

        // Capture timestamp immediately before send and update bytes in buffer
        message_with_timestamp.set_timestamp_now();
        let timestamp_bytes = message_with_timestamp.timestamp.to_le_bytes();
        let ts_offset = Message::timestamp_offset();
        if serialized.len() >= ts_offset.end {
            serialized[ts_offset].copy_from_slice(&timestamp_bytes);
        }

        // Build a single contiguous buffer: 4-byte length prefix + serialized data
        // This avoids writev partial-write edge cases where the fallback logic
        // must track which iov segment was partially written.
        let len_bytes = (serialized.len() as u32).to_le_bytes();
        let mut send_buf = Vec::with_capacity(4 + serialized.len());
        send_buf.extend_from_slice(&len_bytes);
        send_buf.extend_from_slice(&serialized);

        stream
            .write_all(&send_buf)
            .context("Failed to write message")?;

        trace!("Message ID {} sent successfully", message.id);
        Ok(())
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via blocking UDS");

        // Ensure we have a connection (accept if server, no-op if client)
        self.ensure_connection()?;

        let stream = self.stream.as_mut().context(
            "Cannot receive: socket not connected. \
                 Call start_server_blocking() or start_client_blocking() first.",
        )?;

        // Read length prefix (4 bytes, little-endian) to match async protocol
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).context(
            "Failed to read message length. \
                 Connection may be closed or peer disconnected.",
        )?;
        let len = u32::from_le_bytes(len_bytes) as usize;
        if len == 0 || len > Self::MAX_MESSAGE_SIZE {
            return Err(anyhow!(
                "Invalid message length: {} bytes (allowed: 1..={})",
                len,
                Self::MAX_MESSAGE_SIZE
            ));
        }

        trace!("Receiving message of {} bytes", len);

        // Read message data
        let mut buffer = vec![0u8; len];
        stream
            .read_exact(&mut buffer)
            .context("Failed to read message data")?;

        // CRITICAL: Capture receive timestamp immediately after read
        // This ensures accurate latency measurement
        let receive_time_ns = crate::ipc::get_monotonic_time_ns();

        // Deserialize message
        let mut message: Message =
            bincode::deserialize(&buffer).context("Failed to deserialize message")?;

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
        debug!("Closing blocking UDS transport");

        // Close stream (if open). Drop handles cleanup automatically.
        self.stream = None;

        // Close listener (if server). Drop handles cleanup automatically.
        self.listener = None;

        // Remove socket file to prevent stale files from accumulating
        self.cleanup_socket();

        debug!("Blocking UDS transport closed");
        Ok(())
    }
}

// Implement Default for convenience
impl Default for BlockingUnixDomainSocket {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BlockingUnixDomainSocket {
    fn drop(&mut self) {
        // Clean up socket file on drop to prevent stale files
        self.cleanup_socket();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
    use crate::utils::get_temp_socket_path;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_new_creates_empty_transport() {
        let transport = BlockingUnixDomainSocket::new();
        assert!(transport.listener.is_none());
        assert!(transport.stream.is_none());
    }

    #[test]
    fn test_server_binds_successfully() {
        let socket_path = get_temp_socket_path("test_uds_blocking_server.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Server now only binds in start_server_blocking(), doesn't accept yet
        // Accept happens on first send/receive via ensure_connection()
        let server_socket_path = socket_path.clone();
        let handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_socket_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Trigger accept by receiving a message
            let _msg = server.receive_blocking().unwrap();
            server.close_blocking().unwrap();
        });

        // Give server time to bind
        thread::sleep(Duration::from_millis(100));

        // Connect from client
        let mut client = BlockingUnixDomainSocket::new();
        let client_config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&client_config).unwrap();

        // Send a message so server can receive it
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Server should complete
        handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        let socket_path = get_temp_socket_path("test_uds_blocking_no_server.sock");
        let _ = std::fs::remove_file(&socket_path);

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path,
            ..Default::default()
        };

        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("server"));
    }

    #[test]
    fn test_send_and_receive_message() {
        let socket_path = get_temp_socket_path("test_uds_blocking_send_recv.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Start server in thread
        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive message
            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 42);
            assert_eq!(msg.payload.len(), 100);

            server.close_blocking().unwrap();
        });

        // Give server time to start. This sleep is necessary for the
        // server to reach accept() before client tries to connect.
        thread::sleep(Duration::from_millis(100));

        // Connect client and send
        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(42, vec![0u8; 100], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Wait for server
        server_handle.join().unwrap();

        // Cleanup
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_round_trip_communication() {
        let socket_path = get_temp_socket_path("test_uds_blocking_round_trip.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Start server
        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
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

        // Give server time to start and reach accept()
        thread::sleep(Duration::from_millis(100));

        // Client
        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
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

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_close_cleanup() {
        let socket_path = get_temp_socket_path("test_uds_blocking_close.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Test that close() properly cleans up resources
        let mut server = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();
        server.close_blocking().unwrap();

        // Verify server fields are None after close
        assert!(server.listener.is_none());
        assert!(server.stream.is_none());

        // Test client cleanup
        let mut client = BlockingUnixDomainSocket::new();
        client.close_blocking().unwrap();

        // Verify client fields are None after close
        assert!(client.listener.is_none());
        assert!(client.stream.is_none());

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_default_creates_new_transport() {
        let transport = BlockingUnixDomainSocket::default();
        assert!(transport.listener.is_none());
        assert!(transport.stream.is_none());
    }

    #[test]
    fn test_multiple_messages() {
        let socket_path = get_temp_socket_path("test_uds_blocking_multi.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
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

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
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

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_large_message() {
        let socket_path = get_temp_socket_path("test_uds_blocking_large.sock");
        let _ = std::fs::remove_file(&socket_path);
        let large_size = 10000; // 10KB

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 1);
            assert_eq!(msg.payload.len(), large_size);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, vec![0xAB; large_size], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_client_connection_refused() {
        let socket_path = get_temp_socket_path("test_uds_blocking_no_server.sock");
        let _ = std::fs::remove_file(&socket_path);

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };

        let result = client.start_client_blocking(&config);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_socket_path_cleanup_on_server_start() {
        let socket_path = get_temp_socket_path("test_uds_blocking_cleanup.sock");

        // Create a dummy file at the socket path
        std::fs::write(&socket_path, b"dummy").unwrap();
        assert!(std::path::Path::new(&socket_path).exists());

        // Server should clean up existing file
        let mut server = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();
        server.close_blocking().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_send_without_connection_fails() {
        // Test that send fails when no connection exists
        let mut transport = BlockingUnixDomainSocket::new();
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);

        let result = transport.send_blocking(&msg);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("socket not connected"));
    }

    #[test]
    fn test_receive_without_connection_fails() {
        // Test that receive fails when no connection exists
        let mut transport = BlockingUnixDomainSocket::new();

        let result = transport.receive_blocking();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("socket not connected"));
    }

    #[test]
    fn test_ping_pong_message_types() {
        let socket_path = get_temp_socket_path("test_uds_blocking_ping_pong.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Receive Ping
            let ping = server.receive_blocking().unwrap();
            assert_eq!(ping.message_type, MessageType::Ping);
            assert_eq!(ping.id, 100);

            // Send Pong
            let pong = Message::new(ping.id, Vec::new(), MessageType::Pong);
            server.send_blocking(&pong).unwrap();
            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send Ping
        let ping = Message::new(100, Vec::new(), MessageType::Ping);
        client.send_blocking(&ping).unwrap();

        // Receive Pong
        let pong = client.receive_blocking().unwrap();
        assert_eq!(pong.message_type, MessageType::Pong);
        assert_eq!(pong.id, 100);

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_empty_payload_message() {
        let socket_path = get_temp_socket_path("test_uds_blocking_empty.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 1);
            assert!(msg.payload.is_empty());

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send message with empty payload
        let msg = Message::new(1, Vec::new(), MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_very_large_message() {
        // Test message larger than socket buffer (65KB)
        let socket_path = get_temp_socket_path("test_uds_blocking_very_large.sock");
        let _ = std::fs::remove_file(&socket_path);
        let very_large_size = 100_000; // 100KB

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 1);
            assert_eq!(msg.payload.len(), very_large_size);
            // Verify payload content
            assert!(msg.payload.iter().all(|&b| b == 0xCD));

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, vec![0xCD; very_large_size], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_payload_content_integrity() {
        let socket_path = get_temp_socket_path("test_uds_blocking_integrity.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Create payload with distinct pattern
        let payload: Vec<u8> = (0..256).map(|i| i as u8).collect();
        let payload_clone = payload.clone();

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.payload, payload_clone);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, payload, MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_server_sends_to_client() {
        // Test bidirectional: server initiates send to client
        let socket_path = get_temp_socket_path("test_uds_blocking_server_sends.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Server sends first
            let msg = Message::new(999, vec![1, 2, 3, 4, 5], MessageType::OneWay);
            server.send_blocking(&msg).unwrap();

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Client receives message from server
        let msg = client.receive_blocking().unwrap();
        assert_eq!(msg.id, 999);
        assert_eq!(msg.payload, vec![1, 2, 3, 4, 5]);

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_multiple_round_trips_same_connection() {
        let socket_path = get_temp_socket_path("test_uds_blocking_multi_rt.sock");
        let _ = std::fs::remove_file(&socket_path);
        let num_round_trips = 10;

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for expected_id in 1..=num_round_trips {
                // Receive request
                let request = server.receive_blocking().unwrap();
                assert_eq!(request.id, expected_id);
                assert_eq!(request.message_type, MessageType::Request);

                // Send response
                let response =
                    Message::new(expected_id, request.payload.clone(), MessageType::Response);
                server.send_blocking(&response).unwrap();
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        for id in 1..=num_round_trips {
            // Send request
            let payload = vec![id as u8; 10];
            let request = Message::new(id, payload.clone(), MessageType::Request);
            client.send_blocking(&request).unwrap();

            // Receive response
            let response = client.receive_blocking().unwrap();
            assert_eq!(response.id, id);
            assert_eq!(response.message_type, MessageType::Response);
            assert_eq!(response.payload, payload);
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_ensure_connection_idempotent() {
        // Test that ensure_connection can be called multiple times safely
        let socket_path = get_temp_socket_path("test_uds_blocking_idempotent.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Multiple sends should work (ensure_connection called each time)
            for i in 1..=3 {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, i);
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Send multiple messages (ensure_connection called internally each time)
        for i in 1..=3u64 {
            let msg = Message::new(i, vec![0u8; 10], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_message_timestamp_is_set() {
        let socket_path = get_temp_socket_path("test_uds_blocking_timestamp.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            // Timestamp should be non-zero (set by send)
            assert!(msg.timestamp > 0);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_all_message_types() {
        let socket_path = get_temp_socket_path("test_uds_blocking_all_types.sock");
        let _ = std::fs::remove_file(&socket_path);

        let all_types = vec![
            MessageType::OneWay,
            MessageType::Request,
            MessageType::Response,
            MessageType::Ping,
            MessageType::Pong,
        ];
        let types_clone = all_types.clone();

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for (i, expected_type) in types_clone.iter().enumerate() {
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, i as u64);
                assert_eq!(msg.message_type, *expected_type);
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        for (i, msg_type) in all_types.iter().enumerate() {
            let msg = Message::new(i as u64, vec![i as u8; 10], *msg_type);
            client.send_blocking(&msg).unwrap();
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_server_rebind_after_close() {
        let socket_path = get_temp_socket_path("test_uds_blocking_rebind.sock");
        let _ = std::fs::remove_file(&socket_path);

        // First server session
        {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: socket_path.clone(),
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();
            server.close_blocking().unwrap();
        }

        // Second server session should succeed (socket file should be cleaned up)
        {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: socket_path.clone(),
                ..Default::default()
            };
            // This should succeed because start_server_blocking cleans up existing socket
            server.start_server_blocking(&config).unwrap();
            server.close_blocking().unwrap();
        }

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_server_bind_fails_on_invalid_path() {
        // Test that bind fails with appropriate error for non-existent directory
        let mut server = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: "/nonexistent_dir_12345/socket.sock".to_string(),
            ..Default::default()
        };

        let result = server.start_server_blocking(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to bind") || err_msg.contains("No such file"),
            "Expected bind error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_receive_detects_peer_disconnect() {
        let socket_path = get_temp_socket_path("test_uds_blocking_disconnect.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Try to receive - client will disconnect before sending
            let result = server.receive_blocking();
            // Should get an error when peer disconnects
            assert!(result.is_err());

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Close immediately without sending anything
        client.close_blocking().unwrap();

        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_extremely_large_message() {
        // Test message larger than typical kernel buffer (1MB)
        // This may exercise partial write paths
        let socket_path = get_temp_socket_path("test_uds_blocking_huge.sock");
        let _ = std::fs::remove_file(&socket_path);
        let huge_size = 1_000_000; // 1MB

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, 1);
            assert_eq!(msg.payload.len(), huge_size);
            // Verify first and last bytes
            assert_eq!(msg.payload[0], 0xAA);
            assert_eq!(msg.payload[huge_size - 1], 0xAA);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(1, vec![0xAA; huge_size], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_bidirectional_interleaved_messages() {
        // Test alternating sends from both sides
        let socket_path = get_temp_socket_path("test_uds_blocking_bidir.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            for i in 0..5u64 {
                // Receive from client
                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, i * 2); // Even IDs from client

                // Send to client
                let response = Message::new(i * 2 + 1, vec![0u8; 10], MessageType::OneWay);
                server.send_blocking(&response).unwrap();
            }

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        for i in 0..5u64 {
            // Send to server
            let msg = Message::new(i * 2, vec![0u8; 10], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();

            // Receive from server
            let response = client.receive_blocking().unwrap();
            assert_eq!(response.id, i * 2 + 1); // Odd IDs from server
        }

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_client_receive_after_server_close() {
        let socket_path = get_temp_socket_path("test_uds_blocking_server_close.sock");
        let _ = std::fs::remove_file(&socket_path);

        use std::sync::{Arc, Barrier};
        let barrier = Arc::new(Barrier::new(2));

        let server_path = socket_path.clone();
        let server_barrier = Arc::clone(&barrier);
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Wait for client to connect and be ready to receive
            server_barrier.wait();

            // Close without sending - client should get error
            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Signal server we're ready
        barrier.wait();

        // Small delay to let server close
        thread::sleep(Duration::from_millis(50));

        // Try to receive - should fail because server closed
        let result = client.receive_blocking();
        assert!(result.is_err());

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_message_with_max_u64_id() {
        let socket_path = get_temp_socket_path("test_uds_blocking_max_id.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            let msg = server.receive_blocking().unwrap();
            assert_eq!(msg.id, u64::MAX);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(u64::MAX, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_rapid_connect_disconnect_cycles() {
        let socket_path = get_temp_socket_path("test_uds_blocking_rapid.sock");
        let _ = std::fs::remove_file(&socket_path);

        // Multiple rapid connect/disconnect cycles
        for cycle in 0..3 {
            let server_path = socket_path.clone();
            let server_handle = thread::spawn(move || {
                let mut server = BlockingUnixDomainSocket::new();
                let config = TransportConfig {
                    socket_path: server_path,
                    ..Default::default()
                };
                server.start_server_blocking(&config).unwrap();

                let msg = server.receive_blocking().unwrap();
                assert_eq!(msg.id, cycle);

                server.close_blocking().unwrap();
            });

            thread::sleep(Duration::from_millis(50));

            let mut client = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: socket_path.clone(),
                ..Default::default()
            };
            client.start_client_blocking(&config).unwrap();

            let msg = Message::new(cycle, vec![0u8; 10], MessageType::OneWay);
            client.send_blocking(&msg).unwrap();

            client.close_blocking().unwrap();
            server_handle.join().unwrap();
        }

        let _ = std::fs::remove_file(&socket_path);
    }

    #[test]
    fn test_close_is_idempotent() {
        // Calling close multiple times should not panic
        let mut transport = BlockingUnixDomainSocket::new();

        // Close without ever connecting
        transport.close_blocking().unwrap();
        transport.close_blocking().unwrap();
        transport.close_blocking().unwrap();

        // Verify state
        assert!(transport.listener.is_none());
        assert!(transport.stream.is_none());
    }

    #[test]
    fn test_server_can_send_before_receiving() {
        // Server sends first without receiving anything
        let socket_path = get_temp_socket_path("test_uds_blocking_server_first.sock");
        let _ = std::fs::remove_file(&socket_path);

        let server_path = socket_path.clone();
        let server_handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: server_path,
                ..Default::default()
            };
            server.start_server_blocking(&config).unwrap();

            // Send immediately after accept (triggered by send)
            let msg = Message::new(42, vec![1, 2, 3], MessageType::OneWay);
            server.send_blocking(&msg).unwrap();

            // Then receive response
            let response = server.receive_blocking().unwrap();
            assert_eq!(response.id, 43);

            server.close_blocking().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        // Receive from server first
        let msg = client.receive_blocking().unwrap();
        assert_eq!(msg.id, 42);
        assert_eq!(msg.payload, vec![1, 2, 3]);

        // Then send response
        let response = Message::new(43, Vec::new(), MessageType::Response);
        client.send_blocking(&response).unwrap();

        client.close_blocking().unwrap();
        server_handle.join().unwrap();

        let _ = std::fs::remove_file(&socket_path);
    }
}
