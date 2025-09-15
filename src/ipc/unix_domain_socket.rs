use super::{ConnectionId, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, warn};

/// Unix Domain Socket transport implementation with multi-client support
pub struct UnixDomainSocketTransport {
    state: TransportState,
    // Single connection mode (legacy)
    stream: Option<UnixStream>,
    // Multi-connection mode
    listener: Option<UnixListener>,
    connections: Arc<Mutex<HashMap<ConnectionId, UnixStream>>>,
    next_connection_id: Arc<AtomicU64>,
    socket_path: String,
    // True if this instance created/bound the socket file (server side)
    // Only the owning server should unlink the socket during cleanup.
    owns_socket_file: bool,
    message_receiver: Option<mpsc::Receiver<(ConnectionId, Message)>>,
}

impl UnixDomainSocketTransport {
    /// Create a new Unix Domain Socket transport
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            stream: None,
            listener: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_connection_id: Arc::new(AtomicU64::new(1)),
            socket_path: String::new(),
            owns_socket_file: false,
            message_receiver: None,
        }
    }

    /// Read a message from the Unix stream
    async fn read_message(stream: &mut UnixStream) -> Result<Message> {
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

    /// Write a message to the Unix stream
    async fn write_message(stream: &mut UnixStream, message: &Message) -> Result<()> {
        let message_bytes = message.to_bytes()?;
        let message_len = message_bytes.len() as u32;

        // Write message length and data
        stream.write_all(&message_len.to_le_bytes()).await?;
        stream.write_all(&message_bytes).await?;
        stream.flush().await?;

        Ok(())
    }

    /// Clean up socket file
    fn cleanup_socket(&self) -> Result<()> {
        if self.owns_socket_file && !self.socket_path.is_empty() {
            if let Err(e) = std::fs::remove_file(&self.socket_path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!("Failed to remove socket file {}: {}", self.socket_path, e);
                }
            }
        }
        Ok(())
    }

    /// Handle a single client connection in multi-server mode
    async fn handle_connection(
        connection_id: ConnectionId,
        stream: UnixStream,
        message_sender: mpsc::Sender<(ConnectionId, Message)>,
        connections: Arc<Mutex<HashMap<ConnectionId, UnixStream>>>,
    ) {
        debug!("Handling Unix Domain Socket connection {}", connection_id);

        // Convert to std stream for cloning, then back to tokio
        let std_stream = match stream.into_std() {
            Ok(std_stream) => std_stream,
            Err(e) => {
                error!(
                    "Failed to convert Unix stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        let stream_clone = match std_stream.try_clone() {
            Ok(clone) => clone,
            Err(e) => {
                error!(
                    "Failed to clone Unix stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        // Convert back to tokio streams
        let tokio_stream_clone = match UnixStream::from_std(stream_clone) {
            Ok(tokio_stream) => tokio_stream,
            Err(e) => {
                error!(
                    "Failed to convert cloned Unix stream for connection {}: {}",
                    connection_id, e
                );
                return;
            }
        };

        let mut stream = match UnixStream::from_std(std_stream) {
            Ok(tokio_stream) => tokio_stream,
            Err(e) => {
                error!(
                    "Failed to convert original Unix stream for connection {}: {}",
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
impl IpcTransport for UnixDomainSocketTransport {
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting Unix Domain Socket server on: {}",
            config.socket_path
        );

        self.socket_path = config.socket_path.clone();
        self.state = TransportState::Initializing;

        // Server owns the socket file. Best-effort remove if stale exists.
        self.owns_socket_file = true;
        let _ = std::fs::remove_file(&config.socket_path);

        // Create listener
        let listener = UnixListener::bind(&config.socket_path)?;
        // Relax permissions so host and container users can connect
        #[cfg(unix)]
        {
            use std::fs;
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o666));
        }
        self.listener = Some(listener);
        self.state = TransportState::Connected;

        debug!("Unix Domain Socket server listening (not yet connected)");
        Ok(())
    }

    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting Unix Domain Socket client connecting to: {}",
            config.socket_path
        );

        self.socket_path = config.socket_path.clone();
        self.state = TransportState::Initializing;
        // Client never owns the socket file
        self.owns_socket_file = false;

        // Connect to server
        let stream = UnixStream::connect(&config.socket_path).await?;
        self.stream = Some(stream);
        self.state = TransportState::Connected;

        debug!("Unix Domain Socket client connected");
        Ok(())
    }

    async fn send(&mut self, message: &Message) -> Result<()> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment for server
        if self.stream.is_none() && self.listener.is_some() {
            debug!("Server accepting connection on first send");
            let (stream, _) = self.listener.as_ref().unwrap().accept().await?;
            self.stream = Some(stream);
        }

        if let Some(ref mut stream) = self.stream {
            match Self::write_message(stream, message).await {
                Ok(()) => {
                    debug!("Sent message {} via Unix Domain Socket", message.id);
                    Ok(())
                }
                Err(e) => {
                    // Reset stream so next call will accept a fresh connection
                    self.stream = None;
                    Err(e)
                }
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
            let (stream, _) = self.listener.as_ref().unwrap().accept().await?;
            self.stream = Some(stream);
        }

        if let Some(ref mut stream) = self.stream {
            match Self::read_message(stream).await {
                Ok(message) => {
                    debug!("Received message {} via Unix Domain Socket", message.id);
                    Ok(message)
                }
                Err(e) => {
                    // Reset stream so next call will accept a fresh connection
                    self.stream = None;
                    Err(e)
                }
            }
        } else {
            Err(anyhow!("No active stream available"))
        }
    }

    async fn close(&mut self) -> Result<()> {
        debug!("Closing Unix Domain Socket transport");

        // Close all connections
        {
            let mut conns = self.connections.lock().await;
            conns.clear();
        }

        self.stream = None;
        self.listener = None;
        self.message_receiver = None;
        self.state = TransportState::Disconnected;

        // Clean up socket file on close
        if let Err(e) = self.cleanup_socket() {
            error!("Error cleaning up socket in close: {}", e);
        }

        debug!("Unix Domain Socket transport closed");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Unix Domain Socket"
    }

    fn supports_bidirectional(&self) -> bool {
        true
    }

    fn max_message_size(&self) -> usize {
        16 * 1024 * 1024 // 16MB for Unix Domain Sockets
    }

    // NEW MULTI-CLIENT INTERFACE

    fn supports_multiple_connections(&self) -> bool {
        true // Unix Domain Sockets support multiple connections
    }

    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        debug!(
            "Starting Unix Domain Socket multi-server on: {}",
            config.socket_path
        );

        self.socket_path = config.socket_path.clone();
        self.state = TransportState::Initializing;

        // Multi-server also owns the socket file. Best-effort remove if stale exists.
        self.owns_socket_file = true;
        let _ = std::fs::remove_file(&config.socket_path);

        // Create listener
        let listener = UnixListener::bind(&config.socket_path)?;
        // Relax permissions for multi-server as well
        #[cfg(unix)]
        {
            use std::fs;
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o666));
        }
        debug!(
            "Unix Domain Socket multi-server listening on: {}",
            config.socket_path
        );

        // Create message channel
        let (message_sender, message_receiver) = mpsc::channel(1000);

        // Clone shared state for the accept loop
        let connections = self.connections.clone();
        let next_connection_id = self.next_connection_id.clone();

        // Start accept loop
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let connection_id = next_connection_id.fetch_add(1, Ordering::Relaxed);
                        debug!("Accepted Unix Domain Socket connection {}", connection_id);

                        // Spawn handler for this connection
                        let handler_sender = message_sender.clone();
                        let handler_connections = connections.clone();

                        tokio::spawn(Self::handle_connection(
                            connection_id,
                            stream,
                            handler_sender,
                            handler_connections,
                        ));
                    }
                    Err(e) => {
                        error!("Failed to accept Unix Domain Socket connection: {}", e);
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
                "Sent message {} to Unix Domain Socket connection {}",
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
            debug!("Closed Unix Domain Socket connection {}", connection_id);
            Ok(())
        } else {
            Err(anyhow!("Connection {} not found", connection_id))
        }
    }
}

impl Drop for UnixDomainSocketTransport {
    fn drop(&mut self) {
        // Clean up socket file on drop
        if let Err(e) = self.cleanup_socket() {
            error!("Error cleaning up socket in drop: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::MessageType;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_unix_domain_socket_communication() {
        let socket_path = "/tmp/test_uds.sock";
        let config = TransportConfig {
            socket_path: socket_path.to_string(),
            ..Default::default()
        };

        // Clean up any existing socket
        let _ = std::fs::remove_file(socket_path);

        let mut server = UnixDomainSocketTransport::new();
        let mut client = UnixDomainSocketTransport::new();

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

        // Give server time to start
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
    async fn test_unix_domain_socket_multi_client() {
        let socket_path = "/tmp/test_uds_multi.sock";
        let config = TransportConfig {
            socket_path: socket_path.to_string(),
            ..Default::default()
        };

        // Clean up any existing socket
        let _ = std::fs::remove_file(socket_path);

        let mut server = UnixDomainSocketTransport::new();

        // Start multi-server
        let mut receiver = server.start_multi_server(&config).await.unwrap();

        // Give server time to start
        sleep(Duration::from_millis(100)).await;

        // Start multiple clients
        let mut clients = Vec::new();
        for i in 0..3 {
            let mut client = UnixDomainSocketTransport::new();
            client.start_client(&config).await.unwrap();

            let message = Message::new(i as u64, vec![i as u8; 10], MessageType::OneWay);
            client.send(&message).await.unwrap();

            clients.push(client);
        }

        // Receive messages from multiple clients
        let mut received_count = 0;
        while received_count < 3 {
            if let Ok(Some((connection_id, msg))) =
                tokio::time::timeout(Duration::from_millis(1000), receiver.recv()).await
            {
                debug!(
                    "Received message {} from connection {}",
                    msg.id, connection_id
                );
                received_count += 1;
            } else {
                break;
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
