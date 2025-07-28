//! # IPC Transport Abstraction and Implementation Module
//!
//! This module provides a unified abstraction layer for Inter-Process Communication
//! mechanisms, enabling consistent benchmarking across different transport types.
//! It defines the core traits, data structures, and factory patterns used throughout
//! the IPC benchmark suite.
//!
//! ## Key Design Principles
//!
//! - **Unified Interface**: All IPC mechanisms implement the same `IpcTransport` trait
//! - **Message Abstraction**: Common message format across all transport types
//! - **Multi-Client Support**: Optional support for concurrent connections where applicable
//! - **Async-First**: Built on Tokio for non-blocking I/O and scalable concurrency
//! - **Type Safety**: Strong typing prevents runtime errors and improves reliability
//!
//! ## Transport Architecture
//!
//! ```
//! ┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
//! │   Application   │───▶│  IpcTransport    │───▶│   Specific      │
//! │   Benchmark     │    │     Trait        │    │ Implementation  │
//! │     Logic       │    │   (abstraction)  │    │ (UDS/TCP/SHM)   │
//! └─────────────────┘    └──────────────────┘    └─────────────────┘
//! ```
//!
//! ## Supported Transport Mechanisms
//!
//! - **Unix Domain Sockets**: High-performance local sockets with full duplex
//! - **Shared Memory**: Direct memory access with custom ring buffer protocol
//! - **TCP Sockets**: Network-capable sockets with low-latency optimizations
//! - **POSIX Message Queues**: System-integrated queues with priority support
//!
//! ## Message Protocol
//!
//! All transports use a common message format that includes:
//! - Unique message ID for correlation and ordering
//! - High-precision timestamps for latency measurement
//! - Variable-length payload with arbitrary data
//! - Message type classification for different benchmark patterns
//!
//! ## Multi-Client Architecture
//!
//! Advanced transports support multiple concurrent clients through:
//! - Connection pooling and management
//! - Per-connection message routing
//! - Concurrent worker support for scalability testing
//! - Resource isolation between connections

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::mpsc;

// Public module exports for specific transport implementations
pub mod posix_message_queue;
pub mod shared_memory;
pub mod tcp_socket;
pub mod unix_domain_socket;

// Re-export transport implementations for convenient access
pub use posix_message_queue::PosixMessageQueueTransport;
pub use shared_memory::SharedMemoryTransport;
pub use tcp_socket::TcpSocketTransport;
pub use unix_domain_socket::UnixDomainSocketTransport;

/// Connection identifier for tracking multiple client connections
/// 
/// This type alias provides a clear identifier for individual connections
/// in multi-client scenarios. Each connection gets a unique ID that
/// allows the transport to route messages appropriately.
///
/// ## Usage Context
///
/// - Identifying specific clients in multi-client transports
/// - Routing responses to the correct client connection
/// - Managing per-connection state and resources
/// - Debugging and logging connection-specific events
pub type ConnectionId = u64;

/// Message structure for IPC communication
///
/// This structure provides a unified message format used across all IPC
/// transport mechanisms. It includes metadata needed for benchmarking
/// while remaining transport-agnostic.
///
/// ## Design Considerations
///
/// - **Serializable**: Uses Serde for consistent encoding across transports
/// - **Timestamped**: High-precision timing for latency measurement
/// - **Typed**: Message types enable different benchmark patterns
/// - **Flexible**: Variable payload size supports different test scenarios
/// - **Identifiable**: Unique IDs enable message correlation and ordering
///
/// ## Message Lifecycle
///
/// 1. **Creation**: Message created with payload and type
/// 2. **Serialization**: Converted to bytes for transport
/// 3. **Transmission**: Sent via specific transport mechanism
/// 4. **Deserialization**: Reconstructed from bytes on receiver
/// 5. **Processing**: Analyzed for latency and throughput metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique identifier for message correlation and ordering
    ///
    /// Used to match requests with responses in round-trip tests
    /// and to detect message loss or reordering in transport.
    pub id: u64,
    
    /// Timestamp when message was created (nanoseconds since epoch)
    ///
    /// High-precision timestamp enables accurate latency measurement
    /// from message creation to receipt, accounting for serialization
    /// and transport overhead.
    pub timestamp: u64,
    
    /// Message payload data
    ///
    /// Variable-length byte array containing the actual message content.
    /// Payload size is configurable to test different scenarios from
    /// small control messages to large data transfers.
    pub payload: Vec<u8>,
    
    /// Classification of message type for benchmark patterns
    ///
    /// Enables different test patterns like one-way messaging,
    /// request-response cycles, and ping-pong latency measurement.
    pub message_type: MessageType,
}

/// Message types for different benchmark patterns
///
/// This enumeration defines the different types of messages used in
/// various benchmark scenarios, enabling the implementation of complex
/// communication patterns for comprehensive performance testing.
///
/// ## Test Pattern Support
///
/// - **One-way**: Simple message transmission without response
/// - **Request-Response**: Client-server interaction patterns  
/// - **Ping-Pong**: Round-trip latency measurement
/// - **Custom**: Extensible for future test patterns
///
/// ## Protocol Considerations
///
/// Message types help transports understand the expected communication
/// flow and can be used to optimize behavior for specific patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// One-way message (no response expected)
    ///
    /// Used for throughput testing and fire-and-forget scenarios.
    /// The sender does not wait for acknowledgment or response.
    OneWay,
    
    /// Request message (expecting response)
    ///
    /// Used in request-response patterns where the sender expects
    /// a corresponding response message. Essential for round-trip
    /// latency measurement and client-server testing.
    Request,
    
    /// Response message (reply to request)
    ///
    /// Sent in reply to a Request message, completing the
    /// request-response cycle. Used for measuring full
    /// round-trip communication latency.
    Response,
    
    /// Ping message for round-trip measurement
    ///
    /// Specialized message type for ping-pong latency testing.
    /// Similar to Request but optimized for minimal processing
    /// overhead on the receiver side.
    Ping,
    
    /// Pong message (reply to ping)
    ///
    /// Response to a Ping message, completing the ping-pong cycle.
    /// Used for pure round-trip latency measurement with minimal
    /// processing overhead.
    Pong,
}

impl Message {
    /// Create a new message with the given payload
    ///
    /// Factory method for creating messages with automatic timestamp
    /// generation and proper initialization of all fields.
    ///
    /// ## Parameters
    /// - `id`: Unique identifier for the message
    /// - `payload`: Message content as byte vector
    /// - `message_type`: Type classification for the message
    ///
    /// ## Returns
    /// Fully initialized Message instance ready for transmission
    ///
    /// ## Timestamp Generation
    ///
    /// The timestamp is captured at creation time using high-precision
    /// system timing, providing the baseline for latency calculations.
    pub fn new(id: u64, payload: Vec<u8>, message_type: MessageType) -> Self {
        Self {
            id,
            timestamp: Instant::now().elapsed().as_nanos() as u64,
            payload,
            message_type,
        }
    }

    /// Get the message size in bytes
    ///
    /// Calculates the approximate serialized size of the message,
    /// useful for bandwidth calculations and buffer sizing.
    ///
    /// ## Returns
    /// Estimated message size in bytes including all fields
    ///
    /// ## Size Calculation
    ///
    /// The calculation includes:
    /// - 8 bytes for message ID (u64)
    /// - 8 bytes for timestamp (u64)  
    /// - Variable payload length
    /// - 1 byte for message type enum discriminant
    ///
    /// Note: This is an approximation; actual serialized size may
    /// vary slightly due to encoding overhead.
    pub fn size(&self) -> usize {
        // Approximate size calculation
        8 + // id
        8 + // timestamp
        self.payload.len() + // payload
        1 // message_type (enum discriminant)
    }

    /// Serialize the message to bytes
    ///
    /// Converts the message to a byte representation for transmission
    /// over the transport layer. Uses bincode for efficient binary
    /// serialization.
    ///
    /// ## Returns
    /// - `Ok(Vec<u8>)`: Serialized message bytes
    /// - `Err(anyhow::Error)`: Serialization failure
    ///
    /// ## Serialization Format
    ///
    /// Uses bincode for compact binary serialization, which provides:
    /// - Efficient encoding with minimal overhead
    /// - Fast serialization/deserialization
    /// - Cross-platform compatibility
    /// - Strong type safety
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(bincode::serialize(self)?)
    }

    /// Deserialize bytes to a message
    ///
    /// Reconstructs a Message instance from its byte representation,
    /// performing the inverse of `to_bytes()`.
    ///
    /// ## Parameters
    /// - `bytes`: Serialized message data
    ///
    /// ## Returns
    /// - `Ok(Message)`: Reconstructed message instance
    /// - `Err(anyhow::Error)`: Deserialization failure
    ///
    /// ## Error Conditions
    ///
    /// Deserialization can fail if:
    /// - Data is corrupted or truncated
    /// - Incompatible message format/version
    /// - Invalid enum variants
    /// - Memory allocation failure
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(bincode::deserialize(bytes)?)
    }
}

/// Transport configuration for IPC mechanisms
///
/// This structure contains all configuration parameters needed to
/// initialize and configure different IPC transport mechanisms.
/// It provides a unified configuration interface while allowing
/// transport-specific parameters.
///
/// ## Configuration Categories
///
/// - **Performance**: Buffer sizes and connection limits
/// - **Network**: Host addresses and port numbers
/// - **Local**: Socket paths and shared memory names
/// - **Queue**: Message queue parameters and limits
///
/// ## Transport Compatibility
///
/// Not all parameters apply to every transport:
/// - Network parameters (host/port) only apply to TCP
/// - Local parameters (socket_path) only apply to UDS
/// - Queue parameters only apply to message queues
/// - Buffer parameters apply to most transports but with different meanings
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// Buffer size for internal data structures
    ///
    /// Controls the size of various internal buffers used by transports:
    /// - Shared memory ring buffer size
    /// - Socket send/receive buffer sizes  
    /// - Internal message queuing buffers
    pub buffer_size: usize,
    
    /// Host address for network-based transports
    ///
    /// Specifies the network interface for TCP socket communication.
    /// Common values:
    /// - "127.0.0.1": Localhost testing
    /// - "0.0.0.0": Accept connections from any interface
    /// - Specific IP: Bind to particular network interface
    pub host: String,
    
    /// Port number for network-based transports
    ///
    /// TCP socket port number. The benchmark may modify this value
    /// to ensure uniqueness across concurrent tests.
    pub port: u16,
    
    /// Unix domain socket file path
    ///
    /// Filesystem path for the Unix domain socket. Should be in
    /// a writable directory and will be cleaned up after testing.
    pub socket_path: String,
    
    /// Shared memory segment name
    ///
    /// System-wide identifier for the shared memory segment.
    /// Must be unique to avoid conflicts with other processes.
    pub shared_memory_name: String,
    
    /// Maximum number of concurrent connections
    ///
    /// Limits the number of simultaneous client connections for
    /// transports that support multiple clients. Helps prevent
    /// resource exhaustion during concurrent testing.
    pub max_connections: usize,
    
    /// Maximum number of messages in message queue
    ///
    /// Controls the depth of POSIX message queues. Limited by
    /// system configuration and affects memory usage and
    /// throughput characteristics.
    pub message_queue_depth: usize,
    
    /// Base name for POSIX message queues
    ///
    /// System identifier for message queue resources. The actual
    /// queue name may be derived from this base to ensure uniqueness.
    pub message_queue_name: String,
}

impl Default for TransportConfig {
    /// Create default transport configuration
    ///
    /// Provides sensible defaults for all transport parameters,
    /// suitable for basic testing scenarios.
    ///
    /// ## Default Values
    ///
    /// - Buffer size: 8KB (good balance of memory usage and performance)
    /// - Host: localhost (127.0.0.1) for local testing
    /// - Port: 8080 (commonly available port above privileged range)
    /// - Socket path: /tmp/ipc_benchmark.sock (writable temporary location)
    /// - Shared memory: ipc_benchmark_shm (descriptive unique name)
    /// - Max connections: 16 (reasonable concurrency for most systems)
    /// - Queue depth: 10 (typical system default for message queues)
    /// - Queue name: ipc_benchmark_pmq (descriptive unique name)
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
///
/// This trait defines the unified interface that all IPC transport
/// implementations must provide. It supports both simple single-client
/// scenarios and advanced multi-client concurrent testing.
///
/// ## Interface Design
///
/// The trait provides two levels of functionality:
/// 1. **Legacy Interface**: Simple single-connection methods
/// 2. **Multi-Client Interface**: Advanced concurrent connection support
///
/// ## Async Design
///
/// All methods are async to support non-blocking I/O operations,
/// enabling efficient handling of multiple concurrent connections
/// and high-throughput scenarios.
///
/// ## Error Handling
///
/// All methods return `Result` types with descriptive error information
/// to help diagnose transport-specific issues during benchmarking.
#[async_trait]
pub trait IpcTransport: Send + Sync {
    /// Initialize the transport as a server
    ///
    /// Prepares the transport to accept incoming connections or messages.
    /// This includes binding to network addresses, creating shared resources,
    /// or initializing system objects as needed.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration parameters
    ///
    /// ## Returns
    /// - `Ok(())`: Server initialized successfully
    /// - `Err(anyhow::Error)`: Initialization failed
    ///
    /// ## Resource Management
    ///
    /// Server initialization may create:
    /// - Network sockets and bindings
    /// - Shared memory segments
    /// - Message queues
    /// - Temporary files or system objects
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()>;

    /// Initialize the transport as a client
    ///
    /// Prepares the transport to connect to an existing server.
    /// This includes establishing connections, opening shared resources,
    /// or connecting to system objects.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration parameters
    ///
    /// ## Returns
    /// - `Ok(())`: Client connected successfully
    /// - `Err(anyhow::Error)`: Connection failed
    ///
    /// ## Connection Process
    ///
    /// Client initialization typically involves:
    /// - Connecting to server endpoints
    /// - Opening shared resources
    /// - Handshaking or authentication
    /// - Establishing communication channels
    async fn start_client(&mut self, config: &TransportConfig) -> Result<()>;

    /// Send a message (legacy single-connection interface)
    ///
    /// Transmits a message using the transport mechanism. This is the
    /// primary method for one-way communication and the first half
    /// of request-response patterns.
    ///
    /// ## Parameters
    /// - `message`: Message to transmit
    ///
    /// ## Returns
    /// - `Ok(())`: Message sent successfully
    /// - `Err(anyhow::Error)`: Transmission failed
    ///
    /// ## Performance Considerations
    ///
    /// This method should be optimized for low latency and high throughput
    /// as it's called frequently during benchmarking. Implementations
    /// should minimize copies and allocations where possible.
    async fn send(&mut self, message: &Message) -> Result<()>;

    /// Receive a message (legacy single-connection interface)
    ///
    /// Waits for and receives a message from the transport. This method
    /// may block until a message is available or timeout based on
    /// transport-specific behavior.
    ///
    /// ## Returns
    /// - `Ok(Message)`: Received message
    /// - `Err(anyhow::Error)`: Reception failed or timeout
    ///
    /// ## Blocking Behavior
    ///
    /// The method behavior depends on transport implementation:
    /// - Some transports may block indefinitely
    /// - Others may timeout after a reasonable period
    /// - Async implementation allows cancellation
    async fn receive(&mut self) -> Result<Message>;

    /// Close the transport
    ///
    /// Cleanly shuts down the transport, releasing all resources
    /// and performing necessary cleanup operations.
    ///
    /// ## Returns
    /// - `Ok(())`: Transport closed successfully
    /// - `Err(anyhow::Error)`: Cleanup failed (non-fatal)
    ///
    /// ## Cleanup Operations
    ///
    /// Transport closure typically involves:
    /// - Closing network connections
    /// - Releasing shared memory
    /// - Cleaning up temporary files
    /// - Destroying system objects
    async fn close(&mut self) -> Result<()>;

    /// Get transport name for identification
    ///
    /// Returns a human-readable name for the transport implementation,
    /// used in logging, results, and error messages.
    ///
    /// ## Returns
    /// Static string identifying the transport type
    fn name(&self) -> &'static str;

    /// Check if transport supports bidirectional communication
    ///
    /// Indicates whether the transport can handle both sending and
    /// receiving messages on the same connection.
    ///
    /// ## Returns
    /// - `true`: Supports bidirectional communication (default)
    /// - `false`: Unidirectional transport
    ///
    /// ## Default Implementation
    ///
    /// Most IPC mechanisms support bidirectional communication,
    /// so the default implementation returns true.
    fn supports_bidirectional(&self) -> bool {
        true
    }

    /// Get maximum message size supported
    ///
    /// Returns the maximum size of messages that can be transmitted
    /// through this transport mechanism.
    ///
    /// ## Returns
    /// Maximum message size in bytes
    ///
    /// ## Default Implementation
    ///
    /// Provides a reasonable 1MB default that works for most transports.
    /// Specific implementations should override with transport-specific limits.
    fn max_message_size(&self) -> usize {
        1024 * 1024 // 1MB default
    }

    // NEW MULTI-CLIENT INTERFACE

    /// Check if transport supports multiple concurrent connections
    ///
    /// Indicates whether the transport implementation can handle
    /// multiple simultaneous client connections for concurrent testing.
    ///
    /// ## Returns
    /// - `true`: Supports multiple concurrent connections
    /// - `false`: Limited to single connection (default)
    ///
    /// ## Default Implementation
    ///
    /// Returns false for backward compatibility. Transports that support
    /// multiple connections should override this method.
    fn supports_multiple_connections(&self) -> bool {
        false // Default to false for backward compatibility
    }

    /// Start server that can handle multiple concurrent connections
    ///
    /// Initializes a server that can accept and manage multiple simultaneous
    /// client connections. Returns a channel for receiving messages from
    /// all connected clients.
    ///
    /// ## Parameters
    /// - `config`: Transport configuration parameters
    ///
    /// ## Returns
    /// - `Ok(Receiver)`: Channel for receiving (connection_id, message) pairs
    /// - `Err(anyhow::Error)`: Multi-server initialization failed
    ///
    /// ## Message Routing
    ///
    /// Messages are delivered with their originating connection ID,
    /// enabling the application to route responses back to the correct client.
    ///
    /// ## Default Implementation
    ///
    /// Provides a fallback that uses the single-connection interface
    /// and assigns connection ID 0. Advanced transports should override
    /// this with true multi-client support.
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
    ///
    /// Transmits a message to a particular client connection identified
    /// by its connection ID. Used for responding to specific clients
    /// in multi-client scenarios.
    ///
    /// ## Parameters
    /// - `connection_id`: Target connection identifier
    /// - `message`: Message to transmit
    ///
    /// ## Returns
    /// - `Ok(())`: Message sent successfully
    /// - `Err(anyhow::Error)`: Transmission failed or connection not found
    ///
    /// ## Default Implementation
    ///
    /// Ignores the connection ID and uses the legacy send method,
    /// suitable for single-connection transports.
    async fn send_to_connection(
        &mut self,
        connection_id: ConnectionId,
        message: &Message,
    ) -> Result<()> {
        // Default implementation ignores connection_id and uses legacy send
        self.send(message).await
    }

    /// Get list of active connection IDs
    ///
    /// Returns a list of all currently active connection identifiers,
    /// useful for monitoring and debugging multi-client scenarios.
    ///
    /// ## Returns
    /// Vector of active connection IDs
    ///
    /// ## Default Implementation
    ///
    /// Returns a single connection ID (0) for single-connection transports.
    fn get_active_connections(&self) -> Vec<ConnectionId> {
        // Default implementation returns single connection
        vec![0]
    }

    /// Close a specific connection
    ///
    /// Terminates a particular client connection while leaving other
    /// connections active. Used for testing connection failure scenarios
    /// and resource management.
    ///
    /// ## Parameters
    /// - `connection_id`: Connection to close
    ///
    /// ## Returns
    /// - `Ok(())`: Connection closed successfully
    /// - `Err(anyhow::Error)`: Close failed or connection not found
    ///
    /// ## Default Implementation
    ///
    /// Closes all connections since single-connection transports
    /// can only have one active connection.
    async fn close_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        // Default implementation closes all connections
        self.close().await
    }
}

/// Connection role for tracking server/client status
///
/// This enumeration distinguishes between server and client roles
/// in transport connections, enabling role-specific behavior and
/// resource management.
///
/// ## Role Significance
///
/// - **Server**: Accepts connections, creates resources, manages lifecycle
/// - **Client**: Connects to server, uses existing resources, follows server
///
/// ## Usage Context
///
/// Role information is used for:
/// - Resource creation vs. connection logic
/// - Cleanup responsibility assignment
/// - Protocol behavior differences
/// - Debugging and logging context
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRole {
    /// Server role - accepts connections and manages resources
    Server,
    
    /// Client role - connects to server and uses shared resources
    Client,
}

/// Transport state for tracking connection status
///
/// This enumeration tracks the current state of a transport connection,
/// enabling proper state management and error handling throughout
/// the connection lifecycle.
///
/// ## State Transitions
///
/// ```
/// Uninitialized → Initializing → Connected
///       ↓              ↓            ↓
///    Error ←────────────┴────────→ Disconnected
/// ```
///
/// ## State Significance
///
/// - **Uninitialized**: Transport created but not configured
/// - **Initializing**: Configuration in progress
/// - **Connected**: Ready for message transmission
/// - **Disconnected**: Cleanly closed
/// - **Error**: Failed state requiring reset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// Transport has been created but not initialized
    Uninitialized,
    
    /// Transport is in the process of initialization
    Initializing,
    
    /// Transport is connected and ready for communication
    Connected,
    
    /// Transport has been cleanly disconnected
    Disconnected,
    
    /// Transport is in an error state
    Error,
}

/// Transport factory for creating IPC transport instances
///
/// This factory provides a centralized way to create transport instances
/// based on the requested mechanism type. It abstracts the construction
/// details and provides a uniform interface for transport creation.
///
/// ## Factory Benefits
///
/// - **Abstraction**: Hides implementation details from consumers
/// - **Consistency**: Ensures proper initialization across transport types
/// - **Extensibility**: Easy to add new transport mechanisms
/// - **Testing**: Enables mock implementations for unit testing
///
/// ## Design Pattern
///
/// Uses the Factory pattern to create transport instances dynamically
/// based on runtime configuration, enabling flexible benchmark scenarios.
pub struct TransportFactory;

impl TransportFactory {
    /// Create a new transport instance based on the mechanism
    ///
    /// Factory method that instantiates the appropriate transport
    /// implementation based on the requested IPC mechanism.
    ///
    /// ## Parameters
    /// - `mechanism`: The IPC mechanism type to create
    ///
    /// ## Returns
    /// - `Ok(Box<dyn IpcTransport>)`: Transport instance ready for configuration
    /// - `Err(anyhow::Error)`: Creation failed or unsupported mechanism
    ///
    /// ## Supported Mechanisms
    ///
    /// - `UnixDomainSocket`: Creates Unix Domain Socket transport
    /// - `SharedMemory`: Creates shared memory ring buffer transport
    /// - `TcpSocket`: Creates TCP socket transport with optimizations
    /// - `PosixMessageQueue`: Creates POSIX message queue transport
    ///
    /// ## Error Conditions
    ///
    /// - `All` mechanism should be expanded before calling this method
    /// - Transport-specific initialization failures
    /// - System resource limitations
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
    ///
    /// Factory method that creates multiple instances of the same transport
    /// type, useful for concurrent testing scenarios where multiple workers
    /// need independent transport instances.
    ///
    /// ## Parameters
    /// - `mechanism`: The IPC mechanism type to create
    /// - `count`: Number of transport instances to create
    ///
    /// ## Returns
    /// - `Ok(Vec<Box<dyn IpcTransport>>)`: Vector of transport instances
    /// - `Err(anyhow::Error)`: Creation failed for any instance
    ///
    /// ## Use Cases
    ///
    /// - Concurrent worker testing
    /// - Load testing with multiple clients
    /// - Isolation testing with separate transport instances
    /// - Parallel benchmark execution
    ///
    /// ## Error Handling
    ///
    /// If any instance creation fails, the entire operation fails
    /// to ensure consistent test conditions across all workers.
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

    /// Test message creation and basic functionality
    #[test]
    fn test_message_creation() {
        let payload = vec![1, 2, 3, 4, 5];
        let message = Message::new(1, payload.clone(), MessageType::OneWay);

        assert_eq!(message.id, 1);
        assert_eq!(message.payload, payload);
        assert_eq!(message.message_type, MessageType::OneWay);
        assert!(message.timestamp > 0);
    }

    /// Test message serialization and deserialization
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

    /// Test transport configuration defaults
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
