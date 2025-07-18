use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

pub mod posix_message_queue;
pub mod shared_memory;
pub mod tcp_socket;
pub mod unix_domain_socket;

pub use posix_message_queue::PosixMessageQueueTransport;
pub use shared_memory::SharedMemoryTransport;
pub use tcp_socket::TcpSocketTransport;
pub use unix_domain_socket::UnixDomainSocketTransport;

/// Connection identifier for tracking multiple client connections
pub type ConnectionId = u64;

/// Message structure for IPC communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub timestamp: u64, // nanoseconds since epoch
    pub payload: Vec<u8>,
    pub message_type: MessageType,
}

/// Message types for different benchmark patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// One-way message (no response expected)
    OneWay,
    /// Request message (expecting response)
    Request,
    /// Response message (reply to request)
    Response,
    /// Ping message for round-trip measurement
    Ping,
    /// Pong message (reply to ping)
    Pong,
}

impl Message {
    /// Create a new message with the given payload
    pub fn new(id: u64, payload: Vec<u8>, message_type: MessageType) -> Self {
        Self {
            id,
            timestamp: Instant::now().elapsed().as_nanos() as u64,
            payload,
            message_type,
        }
    }

    /// Get the message size in bytes
    pub fn size(&self) -> usize {
        // Approximate size calculation
        8 + // id
        8 + // timestamp
        self.payload.len() + // payload
        1 // message_type (enum discriminant)
    }

    /// Serialize the message to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(bincode::serialize(self)?)
    }

    /// Deserialize bytes to a message
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(bincode::deserialize(bytes)?)
    }
}

/// Transport configuration for IPC mechanisms
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub buffer_size: usize,
    pub host: String,
    pub port: u16,
    pub socket_path: String,
    pub shared_memory_name: String,
    pub max_connections: usize, // New: maximum concurrent connections
    pub message_queue_depth: usize, // POSIX Message Queue: maximum number of messages in queue
    pub message_queue_name: String, // POSIX Message Queue: base queue name
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            buffer_size: 8192,
            host: "127.0.0.1".to_string(),
            port: 8080,
            socket_path: "/tmp/ipc_benchmark.sock".to_string(),
            shared_memory_name: "ipc_benchmark_shm".to_string(),
            max_connections: 16, // Default to support up to 16 concurrent connections
            message_queue_depth: 10, // Default POSIX Message Queue depth
            message_queue_name: "ipc_benchmark_pmq".to_string(), // Default PMQ name
        }
    }
}

/// Generic IPC transport interface with multi-client support
#[async_trait]
pub trait IpcTransport: Send + Sync {
    /// Initialize the transport as a server
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()>;

    /// Initialize the transport as a client
    async fn start_client(&mut self, config: &TransportConfig) -> Result<()>;

    /// Send a message (legacy single-connection interface)
    async fn send(&mut self, message: &Message) -> Result<()>;

    /// Receive a message (legacy single-connection interface)
    async fn receive(&mut self) -> Result<Message>;

    /// Close the transport
    async fn close(&mut self) -> Result<()>;

    /// Get transport name for identification
    fn name(&self) -> &'static str;

    /// Check if transport supports bidirectional communication
    fn supports_bidirectional(&self) -> bool {
        true
    }

    /// Get maximum message size supported
    fn max_message_size(&self) -> usize {
        1024 * 1024 // 1MB default
    }

    // NEW MULTI-CLIENT INTERFACE

    /// Check if transport supports multiple concurrent connections
    fn supports_multiple_connections(&self) -> bool {
        false // Default to false for backward compatibility
    }

    /// Start server that can handle multiple concurrent connections
    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        // Default implementation falls back to single connection
        self.start_server(config).await?;

        // Create a channel for forwarding messages
        let (tx, rx) = mpsc::channel(1000);

        // For single-connection transports, we'll assign connection ID 0
        tokio::spawn(async move {
            // This is a placeholder - individual transports should override this
            // The actual implementation should properly handle multiple connections
        });

        Ok(rx)
    }

    /// Send a message to a specific connection
    async fn send_to_connection(
        &mut self,
        connection_id: ConnectionId,
        message: &Message,
    ) -> Result<()> {
        // Default implementation ignores connection_id and uses legacy send
        self.send(message).await
    }

    /// Get list of active connection IDs
    fn get_active_connections(&self) -> Vec<ConnectionId> {
        // Default implementation returns single connection
        vec![0]
    }

    /// Close a specific connection
    async fn close_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        // Default implementation closes all connections
        self.close().await
    }
}

/// Connection role for tracking server/client status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRole {
    Server,
    Client,
}

/// Transport state for tracking connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    Uninitialized,
    Initializing,
    Connected,
    Disconnected,
    Error,
}

/// Transport factory for creating IPC transport instances
pub struct TransportFactory;

impl TransportFactory {
    /// Create a new transport instance based on the mechanism
    pub fn create(mechanism: &crate::cli::IpcMechanism) -> Result<Box<dyn IpcTransport>> {
        use crate::cli::IpcMechanism;

        match mechanism {
            IpcMechanism::UnixDomainSocket => Ok(Box::new(UnixDomainSocketTransport::new())),
            IpcMechanism::SharedMemory => Ok(Box::new(SharedMemoryTransport::new())),
            IpcMechanism::TcpSocket => Ok(Box::new(TcpSocketTransport::new())),
            IpcMechanism::PosixMessageQueue => Ok(Box::new(PosixMessageQueueTransport::new())),
            IpcMechanism::All => {
                Err(anyhow::anyhow!("'All' mechanism should be expanded before transport creation"))
            }
        }
    }

    /// Create multiple transport instances for concurrent testing
    pub fn create_multiple(
        mechanism: &crate::cli::IpcMechanism,
        count: usize,
    ) -> Result<Vec<Box<dyn IpcTransport>>> {
        (0..count).map(|_| Self::create(mechanism)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let payload = vec![1, 2, 3, 4, 5];
        let message = Message::new(1, payload.clone(), MessageType::OneWay);

        assert_eq!(message.id, 1);
        assert_eq!(message.payload, payload);
        assert_eq!(message.message_type, MessageType::OneWay);
        assert!(message.timestamp > 0);
    }

    #[test]
    fn test_message_serialization() {
        let payload = vec![1, 2, 3, 4, 5];
        let message = Message::new(1, payload, MessageType::Request);

        let bytes = message.to_bytes().unwrap();
        let deserialized = Message::from_bytes(&bytes).unwrap();

        assert_eq!(message.id, deserialized.id);
        assert_eq!(message.payload, deserialized.payload);
        assert_eq!(message.message_type, deserialized.message_type);
    }

    #[test]
    fn test_transport_config_default() {
        let config = TransportConfig::default();

        assert_eq!(config.buffer_size, 8192);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.socket_path, "/tmp/ipc_benchmark.sock");
        assert_eq!(config.shared_memory_name, "ipc_benchmark_shm");
        assert_eq!(config.max_connections, 16);
        assert_eq!(config.message_queue_depth, 10);
        assert_eq!(config.message_queue_name, "ipc_benchmark_pmq");
    }
}
