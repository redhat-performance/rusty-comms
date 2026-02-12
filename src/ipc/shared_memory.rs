use super::{
    ConnectionId, ConnectionRole, IpcError, IpcTransport, Message, TransportConfig, TransportState,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use shared_memory::{Shmem, ShmemConf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio::time::sleep;
use tracing::{debug, warn};

/// Header for dual ring buffer shared memory layout.
/// Contains coordination flags shared by both directions.
#[repr(C)]
struct DualRingBufferHeader {
    /// Capacity of each individual ring buffer (not total)
    buffer_capacity: AtomicUsize,
    /// Server is ready to communicate
    server_ready: AtomicBool,
    /// Client is ready to communicate
    client_ready: AtomicBool,
    /// Shutdown signal
    shutdown: AtomicBool,
}

impl DualRingBufferHeader {
    const SIZE: usize = std::mem::size_of::<Self>();

    fn new(buffer_capacity: usize) -> Self {
        Self {
            buffer_capacity: AtomicUsize::new(buffer_capacity),
            server_ready: AtomicBool::new(false),
            client_ready: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }
}

/// Single direction ring buffer header (no coordination flags - those are in DualRingBufferHeader)
#[repr(C)]
struct SingleRingBuffer {
    read_pos: AtomicUsize,
    write_pos: AtomicUsize,
    message_count: AtomicUsize,
    // Data follows after this header
}

impl SingleRingBuffer {
    const HEADER_SIZE: usize = std::mem::size_of::<Self>();

    fn new() -> Self {
        Self {
            read_pos: AtomicUsize::new(0),
            write_pos: AtomicUsize::new(0),
            message_count: AtomicUsize::new(0),
        }
    }

    fn data_ptr(&self) -> *mut u8 {
        unsafe { (self as *const Self as *mut u8).add(Self::HEADER_SIZE) }
    }

    fn available_write_space(&self, capacity: usize) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            capacity - (write_pos - read_pos) - 1
        } else {
            read_pos - write_pos - 1
        }
    }

    fn available_read_data(&self, capacity: usize) -> usize {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        let write_pos = self.write_pos.load(Ordering::Acquire);

        if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            capacity - (read_pos - write_pos)
        }
    }

    fn write_data(&self, data: &[u8], capacity: usize) -> Result<()> {
        let data_len = data.len();
        let required_space = data_len + 4; // 4 bytes for length prefix

        if self.available_write_space(capacity) < required_space {
            return Err(anyhow!("Not enough space in ring buffer"));
        }

        let write_pos = self.write_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Write length prefix (always fits in 4 bytes, handle wrap)
        let len_bytes = (data_len as u32).to_le_bytes();
        unsafe {
            for (i, &byte) in len_bytes.iter().enumerate() {
                *data_ptr.add((write_pos + i) % capacity) = byte;
            }
        }

        // Write data using bulk copy when possible
        let data_start = (write_pos + 4) % capacity;
        unsafe {
            if data_start + data_len <= capacity {
                // Data fits contiguously - use fast memcpy
                std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), data_len);
            } else {
                // Data wraps around - copy in two parts
                let first_part = capacity - data_start;
                std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(data_start), first_part);
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(first_part),
                    data_ptr,
                    data_len - first_part,
                );
            }
        }

        self.write_pos
            .store((write_pos + required_space) % capacity, Ordering::Release);
        self.message_count.fetch_add(1, Ordering::Release);

        Ok(())
    }

    fn read_data(&self, capacity: usize) -> Result<Vec<u8>> {
        if self.available_read_data(capacity) < 4 {
            return Err(anyhow!("No data available"));
        }

        let read_pos = self.read_pos.load(Ordering::Acquire);
        let data_ptr = self.data_ptr();

        // Read length prefix (handle potential wrap)
        let mut len_bytes = [0u8; 4];
        unsafe {
            for (i, byte) in len_bytes.iter_mut().enumerate() {
                *byte = *data_ptr.add((read_pos + i) % capacity);
            }
        }
        let data_len = u32::from_le_bytes(len_bytes) as usize;

        // Basic validation to prevent reading garbage
        if data_len == 0 || data_len > capacity {
            return Err(anyhow!("Invalid message length: {}", data_len));
        }

        if self.available_read_data(capacity) < data_len + 4 {
            return Err(anyhow!("Incomplete message"));
        }

        // Read data using bulk copy when possible
        let mut data = vec![0u8; data_len];
        let data_start = (read_pos + 4) % capacity;
        unsafe {
            if data_start + data_len <= capacity {
                // Data is contiguous - use fast memcpy
                std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), data_len);
            } else {
                // Data wraps around - copy in two parts
                let first_part = capacity - data_start;
                std::ptr::copy_nonoverlapping(data_ptr.add(data_start), data.as_mut_ptr(), first_part);
                std::ptr::copy_nonoverlapping(
                    data_ptr,
                    data.as_mut_ptr().add(first_part),
                    data_len - first_part,
                );
            }
        }

        self.read_pos
            .store((read_pos + data_len + 4) % capacity, Ordering::Release);

        Ok(data)
    }
}


/// Dual ring buffer connection for bidirectional communication.
/// 
/// Memory layout:
/// [DualRingBufferHeader] - coordination flags
/// [SingleRingBuffer for client→server] - header + data
/// [SingleRingBuffer for server→client] - header + data
#[derive(Clone)]
struct SharedMemoryConnection {
    connection_id: ConnectionId,
    /// Pointer to the dual ring buffer header (coordination flags)
    header: *mut DualRingBufferHeader,
    /// Pointer to client→server ring buffer (client writes, server reads)
    client_to_server: *mut SingleRingBuffer,
    /// Pointer to server→client ring buffer (server writes, client reads)
    server_to_client: *mut SingleRingBuffer,
    /// Capacity of each ring buffer's data area
    buffer_capacity: usize,
    role: ConnectionRole,
    _shmem: Arc<Mutex<Shmem>>,
    // Async notification primitives for efficient waiting (replaces sleep loops)
    notify_data_ready: Arc<Notify>, // Signals when data is available to read
    notify_space_ready: Arc<Notify>, // Signals when space is available to write
}

unsafe impl Send for SharedMemoryConnection {}
unsafe impl Sync for SharedMemoryConnection {}

#[allow(clippy::arc_with_non_send_sync)]
impl SharedMemoryConnection {
    /// Calculate total shared memory size needed for dual ring buffers
    fn total_size(buffer_capacity: usize) -> usize {
        DualRingBufferHeader::SIZE 
            + SingleRingBuffer::HEADER_SIZE + buffer_capacity  // client→server
            + SingleRingBuffer::HEADER_SIZE + buffer_capacity  // server→client
    }

    fn new(
        connection_id: ConnectionId,
        segment_name: String,
        buffer_size: usize,
        role: ConnectionRole,
        create: bool,
    ) -> Result<Self> {
        let total_size = Self::total_size(buffer_size);

        let shmem = if create {
            ShmemConf::new()
                .size(total_size)
                .os_id(&segment_name)
                .create()?
        } else {
            ShmemConf::new().os_id(&segment_name).open()?
        };

        let base_ptr = shmem.as_ptr();
        
        // Calculate pointers to each section
        let header_ptr = base_ptr as *mut DualRingBufferHeader;
        let c2s_ptr = unsafe { 
            base_ptr.add(DualRingBufferHeader::SIZE) as *mut SingleRingBuffer 
        };
        let s2c_ptr = unsafe { 
            base_ptr.add(
                DualRingBufferHeader::SIZE 
                + SingleRingBuffer::HEADER_SIZE 
                + buffer_size
            ) as *mut SingleRingBuffer 
        };

        if create {
            // Initialize all structures for new segment
            unsafe {
                *header_ptr = DualRingBufferHeader::new(buffer_size);
                *c2s_ptr = SingleRingBuffer::new();
                *s2c_ptr = SingleRingBuffer::new();
            }
            debug!("Created dual ring buffer shared memory: {} bytes total, {} per buffer", 
                   total_size, buffer_size);
        }

        Ok(Self {
            connection_id,
            header: header_ptr,
            client_to_server: c2s_ptr,
            server_to_client: s2c_ptr,
            buffer_capacity: buffer_size,
            role,
            _shmem: Arc::new(Mutex::new(shmem)),
            notify_data_ready: Arc::new(Notify::new()),
            notify_space_ready: Arc::new(Notify::new()),
        })
    }

    fn get_header(&self) -> &DualRingBufferHeader {
        unsafe { &*self.header }
    }

    /// Get the ring buffer to use for sending (based on role)
    fn send_buffer(&self) -> &SingleRingBuffer {
        unsafe {
            match self.role {
                ConnectionRole::Client => &*self.client_to_server,
                ConnectionRole::Server => &*self.server_to_client,
            }
        }
    }

    /// Get the ring buffer to use for receiving (based on role)
    fn receive_buffer(&self) -> &SingleRingBuffer {
        unsafe {
            match self.role {
                ConnectionRole::Client => &*self.server_to_client,
                ConnectionRole::Server => &*self.client_to_server,
            }
        }
    }

    async fn wait_for_peer(&self, timeout_duration: Duration) -> Result<()> {
        let start = std::time::Instant::now();
        let header = self.get_header();

        loop {
            match self.role {
                ConnectionRole::Server => {
                    if header.client_ready.load(Ordering::Acquire) {
                        return Ok(());
                    }
                }
                ConnectionRole::Client => {
                    if header.server_ready.load(Ordering::Acquire) {
                        return Ok(());
                    }
                }
            }

            if start.elapsed() > timeout_duration {
                return Err(anyhow!("Timeout waiting for peer"));
            }

            // Justification: Short poll delay to wait for the peer to become ready without busy-waiting.
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn mark_ready(&self) {
        let header = self.get_header();
        match self.role {
            ConnectionRole::Server => header.server_ready.store(true, Ordering::Release),
            ConnectionRole::Client => header.client_ready.store(true, Ordering::Release),
        }
    }

    /// Sends a message, returning true if the buffer was full and caused a delay.
    /// Sends a message with accurate timestamp capture for latency measurement.
    ///
    /// The timestamp is updated immediately before the write to shared memory
    /// to ensure accurate one-way latency measurement.
    ///
    /// Uses adaptive spinning: tight spin for immediate write, then short pause to
    /// reduce CPU contention in sustained high-throughput scenarios.
    ///
    /// With dual ring buffers:
    /// - Client sends on client_to_server buffer
    /// - Server sends on server_to_client buffer
    async fn send_message(&self, message: &Message) -> Result<bool, IpcError> {
        let ring_buffer = self.send_buffer();
        let capacity = self.buffer_capacity;

        // Pre-serialize with current timestamp (will be updated before write)
        let mut message_bytes =
            bincode::serialize(&message).map_err(|e| IpcError::Generic(e.into()))?;
        let mut backpressure_detected = false;

        // Pre-compute timestamp offset for efficient in-place updates
        let ts_offset = Message::timestamp_offset();

        // Try to write with timeout and backpressure detection
        let start = std::time::Instant::now();
        let timeout_duration = Duration::from_secs(5);
        let mut spin_count: u32 = 0;
        const SPIN_LIMIT: u32 = 1000; // Spin for quick space availability

        // CRITICAL: Capture timestamp ONCE before the retry loop
        // This ensures accurate latency measurement - the timestamp reflects when
        // the send was initiated, not when it finally succeeded after backpressure
        let ts_now = crate::ipc::get_monotonic_time_ns();
        let ts_bytes = ts_now.to_le_bytes();
        if message_bytes.len() >= ts_offset.end {
            message_bytes[ts_offset].copy_from_slice(&ts_bytes);
        }

        loop {
            match ring_buffer.write_data(&message_bytes, capacity) {
                Ok(()) => {
                    debug!(
                        "Sent message {} via connection {} ({:?})",
                        message.id, self.connection_id, self.role
                    );
                    // Signal reader that data is available (for in-process use)
                    self.notify_data_ready.notify_one();
                    return Ok(backpressure_detected);
                }
                Err(_) => {
                    // Buffer is full - wait for space to become available
                    if !backpressure_detected {
                        backpressure_detected = true;
                    }
                    if start.elapsed() > timeout_duration {
                        return Err(IpcError::BackpressureTimeout);
                    }

                    // Adaptive spin: fast spin for immediate space, then brief pause
                    // to reduce CPU contention and allow tokio runtime progress
                    spin_count += 1;
                    if spin_count < SPIN_LIMIT {
                        std::hint::spin_loop();
                    } else {
                        // Yield to tokio runtime for both in-process and cross-process scenarios
                        tokio::task::yield_now().await;
                        spin_count = 0;
                    }
                }
            }
        }
    }

    /// Receives a message with accurate timestamp capture for latency measurement.
    ///
    /// The receive timestamp is captured immediately after reading from shared memory
    /// and stored in the message's one_way_latency_ns field.
    ///
    /// Uses adaptive spinning: tight spin for immediate data, then short pause to
    /// reduce CPU contention in sustained high-throughput scenarios.
    ///
    /// With dual ring buffers:
    /// - Client receives from server_to_client buffer
    /// - Server receives from client_to_server buffer
    async fn receive_message(&self) -> Result<Message> {
        let ring_buffer = self.receive_buffer();
        let capacity = self.buffer_capacity;

        // Try to read with timeout
        let start = std::time::Instant::now();
        let timeout_duration = Duration::from_secs(5);
        let mut spin_count: u32 = 0;
        const SPIN_LIMIT: u32 = 1000; // Spin for quick data availability

        loop {
            match ring_buffer.read_data(capacity) {
                Ok(data) => {
                    // CRITICAL: Capture receive timestamp immediately after read
                    // This ensures accurate latency measurement
                    let receive_time_ns = crate::ipc::get_monotonic_time_ns();

                    let mut message = Message::from_bytes(&data)?;
                    debug!(
                        "Received message {} via connection {} ({:?})",
                        message.id, self.connection_id, self.role
                    );

                    // Calculate and store one-way latency using accurate receive timestamp
                    // Only for Request/OneWay messages - Response messages already have
                    // one_way_latency_ns set by the server, so don't overwrite it
                    if message.message_type != crate::ipc::MessageType::Response {
                        message.one_way_latency_ns = receive_time_ns.saturating_sub(message.timestamp);
                    }

                    // Signal writer that space is available (for in-process use)
                    self.notify_space_ready.notify_one();
                    return Ok(message);
                }
                Err(_) => {
                    if start.elapsed() > timeout_duration {
                        return Err(anyhow!("Timeout receiving message"));
                    }

                    // Adaptive spin: fast spin for immediate data, then brief pause
                    // to reduce CPU contention and allow tokio runtime progress
                    spin_count += 1;
                    if spin_count < SPIN_LIMIT {
                        std::hint::spin_loop();
                    } else {
                        // Yield to tokio runtime for both in-process and cross-process scenarios
                        tokio::task::yield_now().await;
                        spin_count = 0;
                    }
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
    has_warned_buffer_full: bool,
}

unsafe impl Send for SharedMemoryTransport {}
unsafe impl Sync for SharedMemoryTransport {}

impl Default for SharedMemoryTransport {
    fn default() -> Self {
        Self::new()
    }
}

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
            has_warned_buffer_full: false,
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
                                // Justification: Short poll delay to wait for the server to create the shared memory segment.
                                sleep(Duration::from_millis(100)).await;
                                continue;
                            }
                            Err(e) => {
                                return Err(anyhow!(
                                    "Failed to open shared memory segment after {} attempts: {}",
                                    attempts + 1,
                                    e
                                ))
                            }
                        }
                    };

                    // Wait for server to be ready
                    connection.wait_for_peer(Duration::from_secs(10)).await?;

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
        connection: SharedMemoryConnection,
        _message_sender: mpsc::Sender<(ConnectionId, Message)>,
        connections: Arc<Mutex<HashMap<ConnectionId, SharedMemoryConnection>>>,
    ) {
        debug!("Handling shared memory connection {}", connection_id);

        // Wait for client to connect
        if let Err(e) = connection.wait_for_peer(Duration::from_secs(10)).await {
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
            let mut conns = connections.lock();
            conns.insert(connection_id, connection);
        }

        // Get the connection back for receiving messages
        {
            let conns = connections.lock();
            if let Some(_conn) = conns.get(&connection_id) {
                // We can't easily clone the connection, so we'll work with the one in the map
                // This is a limitation of the current design - we'd need a more sophisticated
                // approach for true concurrent access
            }
        };
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

    /// This implementation detects backpressure when the ring buffer is full,
    /// causing the send operation to retry until space becomes available or a
    /// timeout is reached. A warning is logged on the first detection.
    async fn send(&mut self, message: &Message) -> Result<bool> {
        if self.state != TransportState::Connected {
            return Err(anyhow!("Transport not connected"));
        }

        // Lazy connection establishment
        if self.single_connection.is_none() {
            self.establish_connection().await?;
        }

        if let Some(ref connection) = self.single_connection {
            let backpressure_detected = connection.send_message(message).await;
            match backpressure_detected {
                Ok(detected) => {
                    if detected && !self.has_warned_buffer_full {
                        warn!(
                            "Shared memory buffer is full; backpressure is occurring. 
                            This may impact latency and throughput measurements. 
                            Consider increasing the buffer size if this is not the desired scenario."
                        );
                        self.has_warned_buffer_full = true;
                    }
                    Ok(detected)
                }
                Err(IpcError::BackpressureTimeout) => {
                    // The warning is implicitly handled by the error, but we can log it too.
                    if !self.has_warned_buffer_full {
                        warn!(
                            "Shared memory buffer is full; send timed out due to backpressure. 
                            This will significantly impact latency and throughput measurements."
                        );
                        self.has_warned_buffer_full = true;
                    }
                    Err(anyhow!("Timeout sending message due to backpressure"))
                }
                Err(IpcError::Generic(e)) => Err(e),
            }
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
            let mut conns = self.connections.lock();
            conns.clear();
        }

        // Unlink shared memory segment if we're the server (creator)
        // This releases the system resource so it can be reclaimed
        if self.role == Some(ConnectionRole::Server) && !self.shared_memory_name.is_empty() {
            #[cfg(unix)]
            {
                use std::ffi::CString;
                // POSIX requires shm names to start with '/'.
                // Normalize the name before calling shm_unlink to
                // ensure the correct segment is removed.
                let posix_name =
                    if self.shared_memory_name.starts_with('/') {
                        self.shared_memory_name.clone()
                    } else {
                        format!("/{}", self.shared_memory_name)
                    };
                if let Ok(name) = CString::new(posix_name.as_str()) {
                    unsafe {
                        let result = libc::shm_unlink(name.as_ptr());
                        if result == 0 {
                            debug!(
                                "Unlinked shared memory segment: {}",
                                posix_name
                            );
                        } else {
                            // Not a critical error - segment may
                            // already be unlinked
                            debug!(
                                "shm_unlink failed for {}: {} \
                                 (may already be unlinked)",
                                posix_name,
                                std::io::Error::last_os_error()
                            );
                        }
                    }
                }
            }
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
        let _next_connection_id = self.next_connection_id.clone();
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

                // Justification: Polling interval to check for new client connections in multi-server mode.
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
        let connection = {
            let conns = self.connections.lock();
            conns.get(&connection_id).cloned() // Clone the connection to release the lock
        };

        if let Some(connection) = connection {
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
        if let Some(conns) = self.connections.try_lock() {
            conns.keys().copied().collect()
        } else {
            vec![] // Return empty if locked
        }
    }

    async fn close_connection(&mut self, connection_id: ConnectionId) -> Result<()> {
        let mut conns = self.connections.lock();

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
        // Using a shorter name for shared memory to support macOS and Windows
        let shared_memory_name = format!("test-{}", &Uuid::new_v4().as_simple().to_string()[..18]);
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

        // Justification: Give the server task time to start up before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(500)).await;

        // Start client and communicate
        client.start_client(&config).await.unwrap();

        let message = Message::new(1, vec![1, 2, 3, 4, 5], MessageType::Request);
        client.send(&message).await.unwrap();
        // Justification: Allow a brief moment for the message to be processed by the server.
        // In a real application, a more robust synchronization mechanism would be used.
        sleep(Duration::from_millis(100)).await;

        let response = client.receive().await.unwrap();
        assert_eq!(response.id, 2);
        assert_eq!(response.payload, vec![6, 7, 8]);

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_shared_memory_backpressure() {
        let shared_memory_name = format!(
            "test-backpressure-{}",
            &Uuid::new_v4().as_simple().to_string()[..10]
        );
        let config = TransportConfig {
            shared_memory_name: shared_memory_name.clone(),
            buffer_size: 128, // Small buffer to trigger backpressure easily
            ..Default::default()
        };

        let mut server = SharedMemoryTransport::new();
        let mut client = SharedMemoryTransport::new();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        // Start server in background, but it won't receive anything, causing the buffer to fill up.
        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server.start_server(&server_config).await.unwrap();
            // The server does nothing, so the client's send buffer will fill up.
            // Wait for the client to signal it's done.
            rx.await.unwrap();
            server.close().await.unwrap();
        });

        // Justification: Give the server task time to start up before the client connects.
        // This is a pragmatic approach for testing to avoid race conditions on startup.
        sleep(Duration::from_millis(500)).await;

        // Start client
        client.start_client(&config).await.unwrap();

        let mut backpressure_timeout_detected = false;

        // Send messages until a backpressure timeout occurs.
        for i in 0..20 {
            let message = Message::new(i, vec![0; 64], MessageType::Request); // 64-byte payload
            match client.send(&message).await {
                Ok(backpressure_detected) => {
                    // The first few sends might succeed without backpressure.
                    // Some might even succeed with backpressure if the timing is just right.
                    if backpressure_detected {
                        println!("Regular backpressure detected, continuing to force a timeout.");
                    }
                }
                Err(e) => {
                    // We expect a specific error related to backpressure timeout.
                    if e.to_string()
                        .contains("Timeout sending message due to backpressure")
                    {
                        backpressure_timeout_detected = true;
                        break;
                    }
                    // Other errors should cause a panic.
                    panic!("An unexpected error occurred: {}", e);
                }
            }
        }

        assert!(
            backpressure_timeout_detected,
            "A backpressure timeout was not detected when the buffer was full"
        );

        // Signal the server to shut down.
        tx.send(()).unwrap();
        // Wait for the server to finish.
        server_handle.await.unwrap();
        client.close().await.unwrap();
    }

    /// Test SHM round-trip: client sends multiple Requests, server
    /// echoes back Responses.
    #[tokio::test]
    async fn test_shared_memory_round_trip() {
        let shared_memory_name = format!(
            "test-rt-{}",
            &Uuid::new_v4().as_simple().to_string()[..18]
        );
        let config = TransportConfig {
            shared_memory_name: shared_memory_name.clone(),
            buffer_size: 8192,
            ..Default::default()
        };

        let mut server = SharedMemoryTransport::new();
        let mut client = SharedMemoryTransport::new();

        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server
                .start_server(&server_config)
                .await
                .unwrap();

            for expected_id in 0u64..5 {
                let msg = server.receive().await.unwrap();
                assert_eq!(msg.id, expected_id);
                assert_eq!(
                    msg.message_type,
                    MessageType::Request
                );

                let resp = Message::new(
                    msg.id,
                    msg.payload.clone(),
                    MessageType::Response,
                );
                server.send(&resp).await.unwrap();
            }

            server.close().await.unwrap();
        });

        // Justification: Give the server task time to create the
        // shared memory segment and start listening.
        sleep(Duration::from_millis(500)).await;

        client.start_client(&config).await.unwrap();

        for id in 0u64..5 {
            let payload = vec![id as u8; 128];
            let msg = Message::new(
                id,
                payload.clone(),
                MessageType::Request,
            );
            client.send(&msg).await.unwrap();

            // Justification: Allow server to process the message
            // and write its response to the ring buffer.
            sleep(Duration::from_millis(50)).await;

            let resp = client.receive().await.unwrap();
            assert_eq!(resp.id, id);
            assert_eq!(resp.payload, payload);
            assert_eq!(
                resp.message_type,
                MessageType::Response
            );
        }

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }

    /// Test SHM with various message sizes to exercise the ring
    /// buffer with different payload lengths.
    #[tokio::test]
    async fn test_shared_memory_various_sizes() {
        let sizes: Vec<usize> =
            vec![1, 32, 128, 512, 2048];
        let sizes_clone = sizes.clone();

        let shared_memory_name = format!(
            "test-sz-{}",
            &Uuid::new_v4().as_simple().to_string()[..18]
        );
        let config = TransportConfig {
            shared_memory_name: shared_memory_name.clone(),
            buffer_size: 65536,
            ..Default::default()
        };

        let mut server = SharedMemoryTransport::new();
        let mut client = SharedMemoryTransport::new();

        let server_config = config.clone();
        let server_handle = tokio::spawn(async move {
            server
                .start_server(&server_config)
                .await
                .unwrap();

            for (i, &expected_size) in
                sizes_clone.iter().enumerate()
            {
                let msg = server.receive().await.unwrap();
                assert_eq!(msg.id, i as u64);
                assert_eq!(
                    msg.payload.len(),
                    expected_size,
                    "Size mismatch for message {}",
                    i
                );
            }

            server.close().await.unwrap();
        });

        // Justification: Give the server task time to create the
        // shared memory segment.
        sleep(Duration::from_millis(500)).await;

        client.start_client(&config).await.unwrap();

        for (i, &size) in sizes.iter().enumerate() {
            let payload = vec![0xAB_u8; size];
            let msg = Message::new(
                i as u64,
                payload,
                MessageType::OneWay,
            );
            client.send(&msg).await.unwrap();
        }

        // Justification: Allow time for all messages to be read
        // by the server before closing.
        sleep(Duration::from_millis(200)).await;

        client.close().await.unwrap();
        server_handle.await.unwrap();
    }
}
