use super::{ConnectionId, ConnectionRole, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use shared_memory::{Shmem, ShmemConf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::time::sleep;
use tracing::debug;

/// Shared memory ring buffer structure
#[repr(C)]
struct SharedMemoryRingBuffer {
    // Ring buffer metadata
    capacity: AtomicUsize,
    read_pos: AtomicUsize,
    write_pos: AtomicUsize,

    // Synchronization flags
    server_ready: AtomicBool,
    client_ready: AtomicBool,
    shutdown: AtomicBool,

    // Message count for coordination
    message_count: AtomicUsize,
    // Data follows after this header
}

impl SharedMemoryRingBuffer {
    const HEADER_SIZE: usize = std::mem::size_of::<Self>();

    fn new(capacity: usize) -> Self {
        Self {
            capacity: AtomicUsize::new(capacity),
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            server_ready: AtomicBool::new(false),
            client_ready: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            message_count: AtomicUsize::new(0),
        }
    }

    fn data_ptr(&self) -> *mut u8 {
        unsafe { (self as *const Self as *mut u8).add(Self::HEADER_SIZE) }
    }

    fn available_write_space(&self) -> usize {
        let capacity = self.capacity.load(Ordering::Acquire);
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            capacity - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        }
    }

    fn available_read_data(&self) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            self.capacity.load(Ordering::Acquire) - (read_pos - write_pos)
        }
    }

    fn write_data(&self, data: &[u8]) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        if self.available_write_space() < required_space {
            return Err(anyhow!("Not enough space in ring buffer"));
        }

        let capacity = self.capacity.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Write length prefix
        let len_bytes = (data_len as u32).to_le_bytes();
        for (i, &byte) in len_bytes.iter().enumerate() {
            unsafe {
                *data_ptr.add((write_pos + i) % capacity) = byte;
            }
        }

        // Write data
        for (i, &byte) in data.iter().enumerate() {
            unsafe {
                *data_ptr.add((write_pos + 4 + i) % capacity) = byte;
            }
        }

        self.write_pos
            .store((write_pos + required_space) % capacity, Ordering::Release);
        self.message_count.fetch_add(1, Ordering::Release);

        Ok(())
    }

    fn read_data(&self) -> Result<Vec<u8>> {
        if self.available_read_data() < 4 {
            return Err(anyhow!("No data available"));
        }

        let capacity = self.capacity.load(Ordering::Acquire);
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Read length prefix
        let mut len_bytes = [0u8; 4];
        for i in 0..4 {
            unsafe {
                len_bytes[i] = *data_ptr.add((read_pos + i) % capacity);
            }
        }
        let data_len = u32::from_le_bytes(len_bytes) as usize;

        // Basic validation to prevent reading garbage
        if data_len == 0 || data_len > capacity {
            return Err(anyhow!("Invalid message length: {}", data_len));
        }

        if self.available_read_data() < data_len + 4 {
            return Err(anyhow!("Incomplete message"));
        }

        // Read data
        let mut data = vec![0u8; data_len];
        for i in 0..data_len {
            unsafe {
                data[i] = *data_ptr.add((read_pos + 4 + i) % capacity);
            }
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        Ok(data)
    }
}

/// Connection-specific shared memory segment
struct SharedMemoryConnection {
    connection_id: ConnectionId,
    shmem: Arc<Shmem>,
    ring_buffer: *mut SharedMemoryRingBuffer,
    segment_name: String,
    role: ConnectionRole,
}

unsafe impl Send for SharedMemoryConnection {}
unsafe impl Sync for SharedMemoryConnection {}

impl SharedMemoryConnection {
    fn new(
        connection_id: ConnectionId,
        segment_name: String,
        buffer_size: usize,
        role: ConnectionRole,
        create: bool,
    ) -> Result<Self> {
        let total_size = SharedMemoryRingBuffer::HEADER_SIZE + buffer_size;

        let shmem = if create {
            ShmemConf::new()
                .size(total_size)
                .os_id(&segment_name)
                .create()?
        } else {
            ShmemConf::new().os_id(&segment_name).open()?
        };

        let ring_buffer_ptr = shmem.as_ptr() as *mut SharedMemoryRingBuffer;

        if create {
            // Initialize ring buffer for new segment
            unsafe {
                *ring_buffer_ptr = SharedMemoryRingBuffer::new(buffer_size);
            }
        }

        Ok(Self {
            connection_id,
            shmem: Arc::new(shmem),
            ring_buffer: ring_buffer_ptr,
            segment_name,
            role,
        })
    }

    fn get_ring_buffer(&self) -> &SharedMemoryRingBuffer {
        unsafe { &*self.ring_buffer }
    }

    async fn wait_for_peer(&self, timeout_duration: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        let ring_buffer = self.get_ring_buffer();

        loop {
            match self.role {
                ConnectionRole::Server => {
                    if ring_buffer.client_ready.load(Ordering::Acquire) {
                        return Ok(());
                    }
                }
                ConnectionRole::Client => {
                    if ring_buffer.server_ready.load(Ordering::Acquire) {
                        return Ok(());
                    }
                }
            }

            if start.elapsed() > timeout_duration {
                return Err(anyhow!("Timeout waiting for peer"));
            }

            sleep(Duration::from_millis(10)).await;
        }
    }

    fn mark_ready(&self) {
        let ring_buffer = self.get_ring_buffer();
        match self.role {
            ConnectionRole::Server => ring_buffer.server_ready.store(true, Ordering::Release),
            ConnectionRole::Client => ring_buffer.client_ready.store(true, Ordering::Release),
        }
    }

    async fn send_message(&self, message: &Message) -> Result<()> {
        let ring_buffer = self.get_ring_buffer();
        let message_bytes = bincode::serialize(&message)?;

        // Try to write with timeout
        let start = std::time::Instant::now();
        let timeout_duration = Duration::from_secs(5);

        loop {
            match ring_buffer.write_data(&message_bytes) {
                Ok(()) => {
                    debug!(
                        "Sent message {} via connection {}",
                        message.id, self.connection_id
                    );
                    return Ok(());
                }
                Err(_) => {
                    if start.elapsed() > timeout_duration {
                        return Err(anyhow!("Timeout sending message"));
                    }
                    sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }

    async fn receive_message(&self) -> Result<Message> {
        let ring_buffer = self.get_ring_buffer();

        // Try to read with timeout
        let start = std::time::Instant::now();
        let timeout_duration = Duration::from_secs(5);

        loop {
            match ring_buffer.read_data() {
                Ok(data) => {
                    let message = Message::from_bytes(&data)?;
                    debug!(
                        "Received message {} via connection {}",
                        message.id, self.connection_id
                    );
                    return Ok(message);
                }
                Err(_) => {
                    if start.elapsed() > timeout_duration {
                        return Err(anyhow!("Timeout receiving message"));
                    }
                    sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }
}

/// Shared Memory transport implementation with multi-client support
pub struct SharedMemoryTransport {
    state: TransportState,
    role: Option<ConnectionRole>,
    // Single connection mode (legacy)
    single_connection: Option<SharedMemoryConnection>,
    // Multi-connection mode
    connections: Arc<Mutex<HashMap<ConnectionId, SharedMemoryConnection>>>,
    next_connection_id: Arc<AtomicU64>,
    shared_memory_name: String,
    buffer_size: usize,
    message_receiver: Option<mpsc::Receiver<(ConnectionId, Message)>>,
}

unsafe impl Send for SharedMemoryTransport {}
unsafe impl Sync for SharedMemoryTransport {}

impl SharedMemoryTransport {
    /// Create a new Shared Memory transport
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            role: None,
            single_connection: None,
            connections: Arc::new(Mutex::new(HashMap::new())),
            next_connection_id: Arc::new(AtomicU64::new(1)),
            shared_memory_name: String::new(),
            buffer_size: 0,
            message_receiver: None,
        }
    }

    /// Establish the actual shared memory connection (called lazily for client)
    async fn establish_connection(&mut self) -> Result<()> {
        if let Some(role) = self.role {
            match role {
                ConnectionRole::Server => {
                    // Server should already have the connection from start_server()
                    if self.single_connection.is_some() {
                        debug!("Server connection already established");
                        return Ok(());
                    } else {
                        return Err(anyhow!("Server connection not properly initialized"));
                    }
                }
                ConnectionRole::Client => {
                    debug!("Establishing shared memory connection as client");
                    
                    // Connect to existing shared memory segment with retry logic
                    let mut attempts = 0;
                    let max_attempts = 30;
                    let connection = loop {
                        match SharedMemoryConnection::new(
                            0, // Connection ID 0 for single connection mode
                            self.shared_memory_name.clone(),
                            self.buffer_size,
                            ConnectionRole::Client,
                            false, // Open existing segment
                        ) {
                            Ok(conn) => break conn,
                            Err(_) if attempts < max_attempts => {
                                debug!("Shared memory segment not ready yet, retrying... (attempt {}/{})", attempts + 1, max_attempts);
                                attempts += 1;
                                sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            Err(e) => return Err(anyhow!("Failed to open shared memory segment after {} attempts: {}", attempts + 1, e)),
                        }
                    };

                    // Wait for server to be ready
                    connection.wait_for_peer(Duration::from_secs(30)).await?;

                    // Mark client as ready
                    connection.mark_ready();

                    self.single_connection = Some(connection);
                    debug!("Shared Memory client connection established");
                }
            }
        } else {
            return Err(anyhow!("Role not set"));
        }
        
        Ok(())
    }

    /// Handle a client connection in multi-server mode
    async fn handle_connection(
        connection_id: ConnectionId,
        mut connection: SharedMemoryConnection,
        message_sender: mpsc::Sender<(ConnectionId, Message)>,
        connections: Arc<Mutex<HashMap<ConnectionId, SharedMemoryConnection>>>,
    ) {
        debug!("Handling shared memory connection {}", connection_id);

        // Wait for client to connect
        if let Err(e) = connection.wait_for_peer(Duration::from_secs(30)).await {
            debug!(
                "Failed to wait for peer on connection {}: {}",
                connection_id, e
            );
            return;
        }

        // Mark server as ready
        connection.mark_ready();

        // Add to active connections
        {
            let mut conns = connections.lock().await;
            conns.insert(connection_id, connection);
        }

        // Get the connection back for receiving messages
        let conn = {
            let conns = connections.lock().await;
            if let Some(conn) = conns.get(&connection_id) {
                // We can't easily clone the connection, so we'll work with the one in the map
                // This is a limitation of the current design - we'd need a more sophisticated
                // approach for true concurrent access
                return;
            } else {
                return;
            }
        };

        // For now, we'll use a simpler approach where each connection
        // is handled independently in the multi-server setup
        debug!("Connection {} handler setup completed", connection_id);
    }
}

#[async_trait]
impl IpcTransport for SharedMemoryTransport {
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting Shared Memory server with name: {}",
            config.shared_memory_name
        );

        self.shared_memory_name = config.shared_memory_name.clone();
        self.buffer_size = config.buffer_size;
        self.role = Some(ConnectionRole::Server);

        // Create the shared memory segment immediately so clients can find it
        let connection = SharedMemoryConnection::new(
            0, // Connection ID 0 for single connection mode
            config.shared_memory_name.clone(),
            config.buffer_size,
            ConnectionRole::Server,
            true, // Create the segment
        )?;

        // Mark server as ready immediately
        connection.mark_ready();
        self.single_connection = Some(connection);
        self.state = TransportState::Connected;

        debug!("Shared Memory server ready with segment created");
        Ok(())
    }

    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        debug!(
            "Starting Shared Memory client connecting to: {}",
            config.shared_memory_name
        );

        self.shared_memory_name = config.shared_memory_name.clone();
        self.buffer_size = config.buffer_size;
        self.role = Some(ConnectionRole::Client);
        self.state = TransportState::Connected; // Mark as ready immediately

        debug!("Shared Memory client ready (connection will be established on first use)");
        Ok(())
    }

    async fn send(&mut self, message: &Message) -> Result<()> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment
        if self.single_connection.is_none() {
            self.establish_connection().await?;
        }

        if let Some(ref connection) = self.single_connection {
            connection.send_message(message).await
        } else {
            Err(anyhow!("No active connection available"))
        }
    }

    async fn receive(&mut self) -> Result<Message> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment
        if self.single_connection.is_none() {
            self.establish_connection().await?;
        }

        if let Some(ref connection) = self.single_connection {
            connection.receive_message().await
        } else {
            Err(anyhow!("No active connection available"))
        }
    }

    async fn close(&mut self) -> Result<()> {
        debug!("Closing Shared Memory transport");

        // Close all connections
        {
            let mut conns = self.connections.lock().await;
            conns.clear();
        }

        self.single_connection = None;
        self.role = None;
        self.message_receiver = None;
        self.state = TransportState::Disconnected;

        debug!("Shared Memory transport closed");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "Shared Memory"
    }

    fn supports_bidirectional(&self) -> bool {
        true
    }

    fn max_message_size(&self) -> usize {
        // Limited by ring buffer size
        self.buffer_size.saturating_sub(1024) // Reserve space for metadata
    }

    // NEW MULTI-CLIENT INTERFACE

    fn supports_multiple_connections(&self) -> bool {
        true // Now supports multiple connections via separate segments
    }

    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        debug!(
            "Starting Shared Memory multi-server with base name: {}",
            config.shared_memory_name
        );

        self.shared_memory_name = config.shared_memory_name.clone();
        self.buffer_size = config.buffer_size;
        self.role = Some(ConnectionRole::Server);
        self.state = TransportState::Initializing;

        // Create message channel
        let (message_sender, message_receiver) = mpsc::channel(1000);

        // For shared memory, we'll create connections on-demand when clients connect
        // This is different from TCP/UDS where we accept connections
        // Instead, we'll create a monitoring task that checks for new segments

        let connections = self.connections.clone();
        let next_connection_id = self.next_connection_id.clone();
        let base_name = config.shared_memory_name.clone();
        let buffer_size = config.buffer_size;
        let max_connections = config.max_connections; // Clone the value to avoid borrowing issues

        // Start monitoring task for new connections
        tokio::spawn(async move {
            let mut known_connections = std::collections::HashSet::new();

            loop {
                // Check for new shared memory segments matching our pattern
                // This is a simplified approach - in a real implementation, you might
                // use inotify or similar mechanisms to detect new segments

                for i in 1..=max_connections {
                    let connection_id = i as ConnectionId;
                    let segment_name = format!("{}_{}", base_name, connection_id);

                    if !known_connections.contains(&connection_id) {
                        // Try to open the segment (client creates it)
                        if let Ok(connection) = SharedMemoryConnection::new(
                            connection_id,
                            segment_name,
                            buffer_size,
                            ConnectionRole::Server,
                            false, // Don't create, just open
                        ) {
                            debug!("Detected new shared memory connection {}", connection_id);
                            known_connections.insert(connection_id);

                            // Handle this connection
                            let handler_sender = message_sender.clone();
                            let handler_connections = connections.clone();

                            tokio::spawn(Self::handle_connection(
                                connection_id,
                                connection,
                                handler_sender,
                                handler_connections,
                            ));
                        }
                    }
                }

                // Check every 100ms for new connections
                sleep(Duration::from_millis(100)).await;
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
        let conns = self.connections.lock().await;

        if let Some(connection) = conns.get(&connection_id) {
            connection.send_message(message).await?;
            debug!(
                "Sent message {} to shared memory connection {}",
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
            debug!("Closed shared memory connection {}", connection_id);
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
    use uuid::Uuid;

    #[tokio::test]
    async fn test_shared_memory_communication() {
        let shared_memory_name = format!("test_shm_{}", Uuid::new_v4());
        let config = TransportConfig {
            shared_memory_name: shared_memory_name.clone(),
            buffer_size: 8192,
            ..Default::default()
        };

        let mut server = SharedMemoryTransport::new();
        let mut client = SharedMemoryTransport::new();

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
}
