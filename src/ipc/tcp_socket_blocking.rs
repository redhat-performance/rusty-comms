//! Blocking TCP socket transport implementation.
//!
//! This module provides a blocking implementation of the TCP socket transport
//! using standard library types from `std::net`.
//!
//! # Platform Support
//!
//! This implementation works on all platforms (Linux, macOS, Windows, BSD).
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
//! This matches the protocol used by the async TCP transport for consistency.
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::ipc::{
//!     BlockingTcpSocket, BlockingTransport, TransportConfig
//! };
//!
//! # fn example() -> anyhow::Result<()> {
//! let mut server = BlockingTcpSocket::new();
//! let mut config = TransportConfig::default();
//! config.host = "127.0.0.1".to_string();
//! config.port = 8080;
//!
//! // Server: bind and wait for connection
//! server.start_server_blocking(&config)?;
//!
//! // In another thread/process: client connects
//! // let mut client = BlockingTcpSocket::new();
//! // client.start_client_blocking(&config)?;
//! # Ok(())
//! # }
//! ```

use crate::ipc::{BlockingTransport, Message, TransportConfig};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use tracing::{debug, trace};

/// Blocking TCP socket transport.
///
/// This struct implements the `BlockingTransport` trait using TCP sockets
/// with standard library blocking I/O operations.
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
pub struct BlockingTcpSocket {
    /// Server listener socket.
    /// Only populated in server mode, None in client mode.
    listener: Option<TcpListener>,

    /// Connected socket stream for sending/receiving data.
    /// Populated after accept() in server mode or connect() in client mode.
    stream: Option<TcpStream>,
}

impl BlockingTcpSocket {
    /// Create a new TCP socket transport.
    ///
    /// Creates an uninitialized transport. Call `start_server_blocking()` or
    /// `start_client_blocking()` to initialize it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::ipc::BlockingTcpSocket;
    ///
    /// let transport = BlockingTcpSocket::new();
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
            debug!("Accepting connection on TCP server");
            let (stream, peer_addr) = listener
                .accept()
                .context("Failed to accept connection on TCP socket")?;

            debug!("TCP server accepted connection from: {}", peer_addr);
            self.stream = Some(stream);
            Ok(())
        } else {
            // Not a server or already connected - this is fine
            Ok(())
        }
    }
}

impl BlockingTransport for BlockingTcpSocket {
    fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Starting blocking TCP server at: {}", addr);

        // Create and bind the listener socket
        let listener = TcpListener::bind(&addr).with_context(|| {
            format!(
                "Failed to bind TCP socket to {}. \
                     Check if port {} is available and not in use.",
                addr, config.port
            )
        })?;

        debug!("TCP server bound and listening on: {}", addr);

        // Store the listener but DON'T accept yet
        // Accept will happen on first send/receive call
        // This matches the async version and allows server to signal
        // readiness before accepting connections
        self.listener = Some(listener);

        Ok(())
    }

    fn start_client_blocking(&mut self, config: &TransportConfig) -> Result<()> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Starting blocking TCP client, connecting to: {}", addr);

        // Connect to server socket (blocks until connected or fails)
        let stream = TcpStream::connect(&addr).with_context(|| {
            format!(
                "Failed to connect to TCP socket at {}. \
                     Is the server running?",
                addr
            )
        })?;

        debug!("TCP client connected successfully");

        self.stream = Some(stream);
        Ok(())
    }

    fn send_blocking(&mut self, message: &Message) -> Result<()> {
        trace!("Sending message ID {} via blocking TCP", message.id);

        // Accept connection if server and not yet accepted
        self.ensure_connection()?;

        let stream = self.stream.as_mut().context(
            "Cannot send: socket not connected. \
                 Call start_server_blocking() or start_client_blocking() first.",
        )?;

        // Serialize message using bincode (same as async version)
        let serialized = bincode::serialize(message).context("Failed to serialize message")?;

        // Send length prefix (4 bytes, little-endian) to match async protocol
        let len_bytes = (serialized.len() as u32).to_le_bytes();
        stream
            .write_all(&len_bytes)
            .context("Failed to write message length")?;

        // Send message data
        stream
            .write_all(&serialized)
            .context("Failed to write message data")?;

        // Flush to ensure data is sent immediately and not buffered
        stream.flush().context("Failed to flush socket")?;

        trace!("Message ID {} sent successfully", message.id);
        Ok(())
    }

    fn receive_blocking(&mut self) -> Result<Message> {
        trace!("Waiting to receive message via blocking TCP");

        // Accept connection if server and not yet accepted
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
        debug!("Closing blocking TCP transport");

        // Close stream (if open). Drop handles cleanup automatically.
        self.stream = None;

        // Close listener (if server). Drop handles cleanup automatically.
        self.listener = None;

        debug!("Blocking TCP transport closed");
        Ok(())
    }
}

// Implement Default for convenience
impl Default for BlockingTcpSocket {
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
        let transport = BlockingTcpSocket::new();
        assert!(transport.listener.is_none());
        assert!(transport.stream.is_none());
    }

    #[test]
    fn test_server_binds_successfully() {
        let port = 18081; // Use unique port to avoid conflicts

        // Server now only binds in start_server_blocking(), doesn't accept yet
        // Accept happens on first send/receive via ensure_connection()
        let handle = thread::spawn(move || {
            let mut server = BlockingTcpSocket::new();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
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
        let mut client = BlockingTcpSocket::new();
        let client_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        client.start_client_blocking(&client_config).unwrap();

        // Send a message so server can receive it
        let msg = Message::new(1, vec![0u8; 10], MessageType::OneWay);
        client.send_blocking(&msg).unwrap();
        client.close_blocking().unwrap();

        // Server should complete
        handle.join().unwrap();
    }

    #[test]
    fn test_client_fails_if_server_not_running() {
        let port = 18082; // Use unique port

        let mut client = BlockingTcpSocket::new();
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let result = client.start_client_blocking(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("server"));
    }

    #[test]
    fn test_send_and_receive_message() {
        let port = 18083; // Use unique port

        // Start server in thread
        let server_handle = thread::spawn(move || {
            let mut server = BlockingTcpSocket::new();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
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
        let mut client = BlockingTcpSocket::new();
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
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
        let port = 18084; // Use unique port

        // Start server
        let server_handle = thread::spawn(move || {
            let mut server = BlockingTcpSocket::new();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
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
        let mut client = BlockingTcpSocket::new();
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
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
        let port = 18085; // Use unique port

        // Test that close() properly cleans up resources
        let mut server = BlockingTcpSocket::new();
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        server.start_server_blocking(&config).unwrap();
        server.close_blocking().unwrap();

        // Verify server fields are None after close
        assert!(server.listener.is_none());
        assert!(server.stream.is_none());

        // Test client cleanup
        let mut client = BlockingTcpSocket::new();
        client.close_blocking().unwrap();

        // Verify client fields are None after close
        assert!(client.listener.is_none());
        assert!(client.stream.is_none());
    }
}
