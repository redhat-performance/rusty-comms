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
//!
//! # fn example() -> anyhow::Result<()> {
//! let mut server = BlockingUnixDomainSocket::new();
//! let mut config = TransportConfig::default();
//! config.socket_path = "/tmp/test.sock".to_string();
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
use anyhow::{Context, Result};
use std::io::{Read, Write};
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
}

impl BlockingUnixDomainSocket {
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

            debug!("UDS server accepted connection from: {:?}", addr);
            self.stream = Some(stream);
            Ok(())
        } else {
            // Not a server or already connected - this is fine
            Ok(())
        }
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

        // Store the listener but don't accept yet.
        // The accept() will happen on the first send/receive via ensure_connection().
        // This allows the server to signal readiness before blocking on accept.
        self.listener = Some(listener);

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

        // METHODOLOGY CHANGE: Match C benchmark approach where timestamp is captured
        // immediately before IPC syscall with minimal intervening work.
        //
        // Pre-serialize with dummy timestamp to get buffer structure, then update
        // only the timestamp bytes immediately before send. This ensures any
        // scheduling delays between timestamp capture and send are included in
        // the measured latency (matching C programs).
        let mut message_with_timestamp = message.clone();
        message_with_timestamp.timestamp = 0; // Dummy timestamp for pre-serialization
        let mut serialized =
            bincode::serialize(&message_with_timestamp).context("Failed to serialize message")?;

        // Capture timestamp immediately before send and update bytes in buffer
        message_with_timestamp.set_timestamp_now();
        let timestamp_bytes = message_with_timestamp.timestamp.to_le_bytes();
        let ts_offset = Message::timestamp_offset();
        serialized[ts_offset].copy_from_slice(&timestamp_bytes);

        // Send immediately - no intervening work
        let len_bytes = (serialized.len() as u32).to_le_bytes();
        stream
            .write_all(&len_bytes)
            .context("Failed to write message length")?;

        stream
            .write_all(&serialized)
            .context("Failed to write message data")?;

        stream.flush().context("Failed to flush socket")?;

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

        trace!("Receiving message of {} bytes", len);

        // Read message data
        let mut buffer = vec![0u8; len];
        stream
            .read_exact(&mut buffer)
            .context("Failed to read message data")?;

        // Deserialize message
        let message: Message =
            bincode::deserialize(&buffer).context("Failed to deserialize message")?;

        trace!("Received message ID {}", message.id);
        Ok(message)
    }

    fn close_blocking(&mut self) -> Result<()> {
        debug!("Closing blocking UDS transport");

        // Close stream (if open). Drop handles cleanup automatically.
        self.stream = None;

        // Close listener (if server). Drop handles cleanup automatically.
        self.listener = None;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
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
        let socket_path = "/tmp/test_uds_blocking_server.sock";
        let _ = std::fs::remove_file(socket_path);

        // Server now only binds in start_server_blocking(), doesn't accept yet
        // Accept happens on first send/receive via ensure_connection()
        let handle = thread::spawn(move || {
            let mut server = BlockingUnixDomainSocket::new();
            let config = TransportConfig {
                socket_path: socket_path.to_string(),
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
            socket_path: socket_path.to_string(),
            ..Default::default()
        };
        client.start_client_blocking(&client_config).unwrap();

        // Send a message so server can receive it
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Server should complete
        handle.join().unwrap();
        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        let socket_path = "/tmp/test_uds_blocking_no_server.sock";
        let _ = std::fs::remove_file(socket_path);

        let mut client = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.to_string(),
            ..Default::default()
        };

        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("server"));
    }

    #[test]
    fn test_send_and_receive_message() {
        let socket_path = "/tmp/test_uds_blocking_send_recv.sock";
        let _ = std::fs::remove_file(socket_path);

        // Start server in thread
        let server_path = socket_path.to_string();
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
            socket_path: socket_path.to_string(),
            ..Default::default()
        };
        client.start_client_blocking(&config).unwrap();

        let msg = Message::new(42, vec![0u8; 100], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Wait for server
        server_handle.join().unwrap();

        // Cleanup
        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn test_round_trip_communication() {
        let socket_path = "/tmp/test_uds_blocking_round_trip.sock";
        let _ = std::fs::remove_file(socket_path);

        // Start server
        let server_path = socket_path.to_string();
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
            socket_path: socket_path.to_string(),
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

        let _ = std::fs::remove_file(socket_path);
    }

    #[test]
    fn test_close_cleanup() {
        let socket_path = "/tmp/test_uds_blocking_close.sock";
        let _ = std::fs::remove_file(socket_path);

        // Test that close() properly cleans up resources
        let mut server = BlockingUnixDomainSocket::new();
        let config = TransportConfig {
            socket_path: socket_path.to_string(),
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

        let _ = std::fs::remove_file(socket_path);
    }
}
