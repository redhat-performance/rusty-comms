use super::{ConnectionId, IpcError, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tracing::{debug, error, warn};

/// TCP Socket transport implementation with multi-client support
pub struct TcpSocketTransport {
    state: TransportState,
    // Single connection mode (legacy)
    stream: Option<TcpStream>,
    // Multi-connection mode
    listener: Option<TcpListener>,
    connections: Arc<Mutex<HashMap<ConnectionId, TcpStream>>>,
    next_connection_id: Arc<AtomicU64>,
    address: Option<SocketAddr>,
    message_receiver: Option<mpsc::Receiver<(ConnectionId, Message)>>,
    buffer_size: usize,
    has_warned_backpressure: bool,
}

impl Default for TcpSocketTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpSocketTransport {
    /// Create a new TCP Socket transport
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            stream: None,
            listener: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_connection_id: Arc::new(AtomicU64::new(1)),
            address: None,
            message_receiver: None,
            buffer_size: 8192, // Default buffer size
            has_warned_backpressure: false,
        }
    }

    /// Read a message from the TCP stream
    async fn read_message(stream: &mut TcpStream) -> Result<Message> {
        // Read message length (4 bytes)
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await?;
        let message_len = u32::from_le_bytes(len_bytes) as usize;

        // Validate message length
        if message_len > 16 * 1024 * 1024 {
            return Err(anyhow!("Message too large: {} bytes", message_len));
        }

        // Read message data
        let mut message_data = vec![0u8; message_len];
        stream.read_exact(&mut message_data).await?;

        // Deserialize message
        Message::from_bytes(&message_data)
    }

    /// Write a message to the TCP stream
    async fn write_message(stream: &mut TcpStream, message: &Message) -> Result<(), IpcError> {
        const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
        let message_bytes = message.to_bytes().map_err(IpcError::Generic)?;
        let message_len = message_bytes.len() as u32;

        let write_fut = async {
            stream.write_all(&message_len.to_le_bytes()).await?;
            stream.write_all(&message_bytes).await?;
            stream.flush().await?;
            Ok(()) as Result<(), std::io::Error>
        };

        // Justification: Prevent indefinite blocking on send operations when the receiver is unresponsive,
        // which is a clear sign of backpressure. This timeout ensures the system remains responsive.
        match timeout(WRITE_TIMEOUT, write_fut).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(IpcError::Generic(e.into())), // IO error
            Err(_) => Err(IpcError::BackpressureTimeout),   // Timeout error
        }
    }

    /// Handle a single client connection in multi-server mode
    async fn handle_connection(
        connection_id: ConnectionId,
        stream: TcpStream,
        message_sender: mpsc::Sender<(ConnectionId, Message)>,
        connections: Arc<Mutex<HashMap<ConnectionId, TcpStream>>>,
    ) {
        debug!("Handling TCP connection {}", connection_id);

        // Convert to std stream for cloning, then back to tokio
        let std_stream = match stream.into_std() {
            Ok(std_stream) => std_stream,
            Err(e) => {
                error!(
                    "Failed to convert TCP stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        let stream_clone = match std_stream.try_clone() {
            Ok(clone) => clone,
            Err(e) => {
                error!(
                    "Failed to clone TCP stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        // Convert back to tokio streams
        let tokio_stream_clone = match TcpStream::from_std(stream_clone) {
            Ok(tokio_stream) => tokio_stream,
            Err(e) => {
                error!(
                    "Failed to convert cloned TCP stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        let mut stream = match TcpStream::from_std(std_stream) {
            Ok(tokio_stream) => tokio_stream,
            Err(e) => {
                error!(
                    "Failed to convert original TCP stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        // Add to active connections
        {
            let mut conns = connections.lock().await;
            conns.insert(connection_id, tokio_stream_clone);
        }

        // Read messages from this connection
        loop {
            match Self::read_message(&mut stream).await {
                Ok(message) => {
                    debug!(
                        "Received message {} from connection {}",
                        message.id, connection_id
                    );
                    if message_sender.send((connection_id, message)).await.is_err() {
                        debug!("Message receiver closed for connection {}", connection_id);
                        break;
                    }
                }
                Err(e) => {
                    debug!("Connection {} closed: {}", connection_id, e);
                    break;
                }
            }
        }

        // Remove from active connections
        {
            let mut conns = connections.lock().await;
            conns.remove(&connection_id);
        }

        debug!("Connection {} handler finished", connection_id);
    }
}

#[async_trait]
impl IpcTransport for TcpSocketTransport {
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Starting TCP Socket server on: {}", addr);

        self.state = TransportState::Initializing;

        // Create listener
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;
        self.address = Some(local_addr);
        self.listener = Some(listener);
        self.buffer_size = config.buffer_size; // Store buffer size for later use

        debug!("TCP Socket server listening on: {}", local_addr);
        self.state = TransportState::Connected;

        debug!("TCP Socket server listening (not yet connected)");
        Ok(())
    }

    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Starting TCP Socket client connecting to: {}", addr);

        self.state = TransportState::Initializing;

        // Connect to server
        let stream = TcpStream::connect(&addr).await?;
        self.address = Some(stream.peer_addr()?);

        // Configure socket options for low latency
        let std_stream = stream.into_std()?;
        let socket = socket2::Socket::from(std_stream.try_clone()?);
        socket.set_nodelay(true)?;
        socket.set_recv_buffer_size(config.buffer_size)?;
        socket.set_send_buffer_size(config.buffer_size)?;

        self.stream = Some(TcpStream::from_std(std_stream)?);
        self.state = TransportState::Connected;

        debug!("TCP Socket client connected to: {}", addr);
        Ok(())
    }

    /// This implementation detects backpressure heuristically. If a send
    /// operation takes longer than a few milliseconds, it's considered a sign
    /// that the OS send buffer is full. Additionally, a hard timeout prevents
    /// the operation from blocking indefinitely.
    async fn send(&mut self, message: &Message) -> Result<bool> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment for server
        if self.stream.is_none() && self.listener.is_some() {
            debug!("Server accepting connection on first send");
            let (stream, client_addr) = self.listener.as_ref().unwrap().accept().await?;
            debug!(
                "TCP Socket server accepted connection from: {}",
                client_addr
            );

            // Configure socket options for low latency
            let std_stream = stream.into_std()?;
            let socket = socket2::Socket::from(std_stream.try_clone()?);
            socket.set_nodelay(true)?;
            socket.set_recv_buffer_size(self.buffer_size)?;
            socket.set_send_buffer_size(self.buffer_size)?;

            self.stream = Some(TcpStream::from_std(std_stream)?);
        }

        if let Some(ref mut stream) = self.stream {
            let start_time = std::time::Instant::now();
            match Self::write_message(stream, message).await {
                Ok(()) => {
                    let elapsed = start_time.elapsed();
                    let mut backpressure_detected = false;
                    // A send operation taking longer than a few milliseconds is a strong
                    // indicator of the OS send buffer being full.
                    if elapsed > std::time::Duration::from_millis(5) {
                        backpressure_detected = true;
                        if !self.has_warned_backpressure {
                            warn!(
                                "TCP socket backpressure detected (send took {:?}). \
                                This may impact latency and throughput measurements. \
                                Consider increasing the buffer size if this is not the desired scenario.",
                                elapsed
                            );
                            self.has_warned_backpressure = true;
                        }
                    }
                    debug!("Sent message {} via TCP Socket", message.id);
                    Ok(backpressure_detected)
                }
                Err(IpcError::BackpressureTimeout) => {
                    if !self.has_warned_backpressure {
                        warn!(
                            "TCP send timed out due to backpressure. \
                            This will significantly impact latency and throughput measurements."
                        );
                        self.has_warned_backpressure = true;
                    }
                    Err(anyhow!(IpcError::BackpressureTimeout))
                }
                Err(IpcError::Generic(e)) => Err(e),
            }
        } else {
            Err(anyhow!("No active stream available"))
        }
    }

    async fn receive(&mut self) -> Result<Message> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment for server
        if self.stream.is_none() && self.listener.is_some() {
            debug!("Server accepting connection on first receive");
            let (stream, client_addr) = self.listener.as_ref().unwrap().accept().await?;
            debug!(
                "TCP Socket server accepted connection from: {}",
                client_addr
            );

            // Configure socket options for low latency
            let std_stream = stream.into_std()?;
            let socket = socket2::Socket::from(std_stream.try_clone()?);
            socket.set_nodelay(true)?;
            socket.set_recv_buffer_size(self.buffer_size)?;
            socket.set_send_buffer_size(self.buffer_size)?;

            self.stream = Some(TcpStream::from_std(std_stream)?);
        }

        if let Some(ref mut stream) = self.stream {
            let message = Self::read_message(stream).await?;
            debug!("Received message {} via TCP Socket", message.id);
            Ok(message)
        } else {
            Err(anyhow!("No active stream available"))
        }
    }

    async fn close(&mut self) -> Result<()> {
        debug!("Closing TCP Socket transport");

        // Close all connections
        {
            let mut conns = self.connections.lock().await;
            conns.clear();
        }

        self.stream = None;
        self.listener = None;
        self.address = None;
        self.message_receiver = None;
        self.state = TransportState::Disconnected;

        debug!("TCP Socket transport closed");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "TCP Socket"
    }

    fn supports_bidirectional(&self) -> bool {
        true
    }

    fn max_message_size(&self) -> usize {
        16 * 1024 * 1024 // 16MB for TCP
    }

    // NEW MULTI-CLIENT INTERFACE

    fn supports_multiple_connections(&self) -> bool {
        true // TCP supports multiple connections
    }

    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Starting TCP multi-server on: {}", addr);

        self.state = TransportState::Initializing;

        // Create listener
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;
        self.address = Some(local_addr);

        debug!("TCP multi-server listening on: {}", local_addr);

        // Create message channel
        let (message_sender, message_receiver) = mpsc::channel(1000);

        // Clone shared state for the accept loop
        let connections = self.connections.clone();
        let next_connection_id = self.next_connection_id.clone();
        let buffer_size = config.buffer_size; // Clone the value to avoid borrowing issues

        // Start accept loop
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, client_addr)) => {
                        let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
                        debug!(
                            "Accepted TCP connection {} from: {}",
                            connection_id, client_addr
                        );

                        // Configure socket options
                        if let Ok(std_stream) = stream.into_std() {
                            let socket = socket2::Socket::from(std_stream.try_clone().unwrap());
                            let _ = socket.set_nodelay(true);
                            let _ = socket.set_recv_buffer_size(buffer_size);
                            let _ = socket.set_send_buffer_size(buffer_size);

                            if let Ok(tokio_stream) = TcpStream::from_std(std_stream) {
                                // Spawn handler for this connection
                                let handler_sender = message_sender.clone();
                                let handler_connections = connections.clone();

                                tokio::spawn(Self::handle_connection(
                                    connection_id,
                                    tokio_stream,
                                    handler_sender,
                                    handler_connections,
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to accept TCP connection: {}", e);
                        break;
                    }
                }
            }
        });

        self.state = TransportState::Connected;
        Ok(message_receiver)
    }

    async fn send_to_connection(
        &mut self,
        connection_id: ConnectionId,
        message: &Message,
    ) -> Result<()> {
        let mut conns = self.connections.lock().await;

        if let Some(stream) = conns.get_mut(&connection_id) {
            Self::write_message(stream, message).await?;
            debug!(
                "Sent message {} to TCP connection {}",
                message.id, connection_id
            );
            Ok(())
        } else {
            Err(anyhow!("Connection {} not found", connection_id))
        }
    }

    fn get_active_connections(&self) -> Vec<ConnectionId> {
        // Note: This is a blocking operation, should be called from async context with care
        let conns = match self.connections.try_lock() {
            Ok(conns) => conns,
            Err(_) => return vec![], // Return empty if locked
        };
        conns.keys().copied().collect()
    }

    async fn close_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        let mut conns = self.connections.lock().await;

        if conns.remove(&connection_id).is_some() {
            debug!("Closed TCP connection {}", connection_id);
            Ok(())
        } else {
            Err(anyhow!("Connection {} not found", connection_id))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_tcp_socket_communication() {
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port: 9090,
            ..Default::default()
        };

        let mut server = TcpSocketTransport::new();
        let mut client = TcpSocketTransport::new();

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

        // Justification: Give the server task time to start up and bind the port before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(100)).await;

        // Start client and communicate
        client.start_client(&config).await.unwrap();

        let message = Message::new(1, vec![1, 2, 3, 4, 5], MessageType::Request);
        client.send(&message).await.unwrap();

        let response = client.receive().await.unwrap();
        assert_eq!(response.id, 2);
        assert_eq!(response.payload, vec![6, 7, 8]);

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_socket_backpressure() {
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port: 9092,
            buffer_size: 1024, // Small buffer to trigger backpressure easily
            ..Default::default()
        };

        let mut server = TcpSocketTransport::new();
        let mut client = TcpSocketTransport::new();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        // Start server in background, but it won't receive anything, causing the buffer to fill up.
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            // Accept a connection but do nothing with it.
            let (_stream, _addr) = server.listener.as_ref().unwrap().accept().await.unwrap();
            // Wait for the client to signal it's done.
            rx.await.unwrap();
            server.close().await.unwrap();
        });

        // Justification: Give the server task time to start up and bind the port before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(100)).await;

        // Start client
        client.start_client(&config).await.unwrap();

        let mut backpressure_timeout_detected = false;
        let large_payload = vec![0; 512]; // Payload smaller than buffer but will fill it over time.

        // Send messages until a backpressure timeout occurs.
        for i in 0..1000 {
            let message = Message::new(i, large_payload.clone(), MessageType::Request);
            match client.send(&message).await {
                Ok(backpressure_detected) => {
                    if backpressure_detected {
                        println!("Regular backpressure detected, continuing to force a timeout.");
                    }
                }
                Err(e) => {
                    if e.to_string()
                        .contains("Timeout sending message due to backpressure")
                    {
                        backpressure_timeout_detected = true;
                        break;
                    }
                    panic!("An unexpected error occurred: {}", e);
                }
            }
        }

        assert!(
            backpressure_timeout_detected,
            "A backpressure timeout was not detected when the TCP buffer was full"
        );

        // Signal the server to shut down.
        tx.send(()).unwrap();
        server_handle.await.unwrap();
        client.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_multi_client() {
        let config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port: 9091,
            ..Default::default()
        };

        let mut server = TcpSocketTransport::new();

        // Start multi-server
        let mut receiver = server.start_multi_server(&config).await.unwrap();

        // Justification: Give the server task time to start up and bind the port before clients connect.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(100)).await;

        // Start multiple clients
        let mut clients = Vec::new();
        for i in 0..3 {
            let mut client = TcpSocketTransport::new();
            client.start_client(&config).await.unwrap();

            let message = Message::new(i as u64, vec![i as u8; 10], MessageType::OneWay);
            client.send(&message).await.unwrap();

            clients.push(client);
        }

        // Receive messages from multiple clients
        let mut received_count = 0;
        while received_count < 3 {
            match tokio::time::timeout(Duration::from_millis(1000), receiver.recv()).await {
                Ok(Some((connection_id, message))) => {
                    debug!(
                        "Received message {} from connection {}",
                        message.id, connection_id
                    );
                    received_count += 1;
                }
                _ => {
                    break;
                }
            }
        }

        assert_eq!(received_count, 3);

        // Clean up
        for mut client in clients {
            let _ = client.close().await;
        }
        let _ = server.close().await;
    }
}
