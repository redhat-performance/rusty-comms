use super::{ConnectionId, IpcTransport, Message, TransportConfig, TransportState};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use nix::errno::Errno;
use nix::mqueue::{mq_close, mq_open, mq_receive, mq_send, mq_unlink, MQ_OFlag, MqAttr, MqdT};
use nix::sys::stat::Mode;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// POSIX Message Queue transport implementation
pub struct PosixMessageQueueTransport {
    state: TransportState,
    queue_name: String,
    mq_fd: Option<MqdT>,
    max_msg_size: usize,
    max_msg_count: usize,
    is_creator: bool, // Track if this instance created the queue (server) vs opened it (client)
}

impl PosixMessageQueueTransport {
    /// Create a new POSIX Message Queue transport
    pub fn new() -> Self {
        Self {
            state: TransportState::Uninitialized,
            queue_name: String::new(),
            mq_fd: None,
            max_msg_size: 8192,
            max_msg_count: 10,
            is_creator: false,
        }
    }

    /// Open a message queue
    fn open_queue(&self, queue_name: &str, create: bool) -> Result<MqdT> {
        let flags = if create {
            MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR
        } else {
            MQ_OFlag::O_RDWR
        };
        
        let attr = if create {
            Some(MqAttr::new(0, self.max_msg_count as i64, self.max_msg_size as i64, 0))
        } else {
            None
        };
        
        let mq_fd = mq_open(
            queue_name,
            flags,
            Mode::S_IRUSR | Mode::S_IWUSR,
            attr.as_ref(),
        ).map_err(|e| anyhow!("Failed to open queue '{}': {}", queue_name, e))?;

        debug!("Opened message queue '{}' with fd: {:?}", queue_name, mq_fd);
        Ok(mq_fd)
    }

    /// Clean up message queue
    fn cleanup_queue(&mut self) {
        debug!("Cleaning up POSIX message queue");

        if let Some(fd) = self.mq_fd.take() {
            debug!("Closing message queue with fd: {:?}", fd);
            if let Err(e) = mq_close(fd) {
                warn!("Failed to close message queue: {}", e);
            } else {
                debug!("Closed message queue");
            }
        }

        // Only unlink the queue if this instance created it (server)
        if self.is_creator && !self.queue_name.is_empty() {
            if let Err(e) = mq_unlink(self.queue_name.as_str()) {
                warn!("Failed to unlink message queue '{}': {}", self.queue_name, e);
            } else {
                debug!("Unlinked message queue: {}", self.queue_name);
            }
        }
    }
}

impl Drop for PosixMessageQueueTransport {
    fn drop(&mut self) {
        debug!("Dropping POSIX Message Queue transport");
        self.cleanup_queue();
        debug!("POSIX Message Queue transport dropped");
    }
}

#[async_trait]
impl IpcTransport for PosixMessageQueueTransport {
    async fn start_server(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue server");

        self.queue_name = format!("/{}", config.message_queue_name);
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = true; // Mark this instance as the creator

        // Open/create the message queue in a blocking task
        let queue_name = self.queue_name.clone();
        let max_msg_count = self.max_msg_count;
        let max_msg_size = self.max_msg_size;
        
        let mq_fd = tokio::task::spawn_blocking(move || {
            debug!("Server creating message queue '{}'...", queue_name);
            let attr = MqAttr::new(0, max_msg_count as i64, max_msg_size as i64, 0);
            let result = mq_open(
                queue_name.as_str(),
                MQ_OFlag::O_CREAT | MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK, // Add O_NONBLOCK
                Mode::S_IRUSR | Mode::S_IWUSR,
                Some(&attr),
            ).map_err(|e| anyhow!("Failed to create server queue: {}", e));
            
            match &result {
                Ok(fd) => debug!("Server successfully created queue '{}' with fd: {:?}", queue_name, fd),
                Err(e) => debug!("Server failed to create queue '{}': {}", queue_name, e),
            }
            
            result
        }).await??;

        self.mq_fd = Some(mq_fd);
        self.state = TransportState::Connected;
        debug!("POSIX Message Queue server started with queue: {}", self.queue_name);
        Ok(())
    }

    async fn start_client(&mut self, config: &TransportConfig) -> Result<()> {
        debug!("Starting POSIX Message Queue client");

        self.queue_name = format!("/{}", config.message_queue_name);
        self.max_msg_count = config.message_queue_depth;
        self.max_msg_size = config.buffer_size.max(1024);
        self.is_creator = false; // Mark this instance as a client

        // Open existing message queue with retry logic
        let queue_name = self.queue_name.clone();
        
        let mq_fd = tokio::task::spawn_blocking(move || {
            // Retry opening the queue with exponential backoff
            let mut attempts = 0;
            let max_attempts = 10;
            let mut delay_ms = 10;
            
            loop {
                match mq_open(
                    queue_name.as_str(),
                    MQ_OFlag::O_RDWR | MQ_OFlag::O_NONBLOCK, // Add O_NONBLOCK
                    Mode::empty(),
                    None,
                ) {
                    Ok(fd) => {
                        debug!("Client successfully opened queue '{}' with fd: {:?} after {} attempts", queue_name, fd, attempts + 1);
                        return Ok(fd);
                    }
                    Err(Errno::ENOENT) if attempts < max_attempts => {
                        debug!("Queue '{}' not ready yet, retrying in {}ms (attempt {}/{})", queue_name, delay_ms, attempts + 1, max_attempts);
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        attempts += 1;
                        delay_ms = (delay_ms * 2).min(1000); // Cap at 1 second
                        continue;
                    }
                    Err(e) => {
                        return Err(anyhow!("Failed to open client queue after {} attempts: {}", attempts + 1, e));
                    }
                }
            }
        }).await??;

        self.mq_fd = Some(mq_fd);
        self.state = TransportState::Connected;
        debug!("POSIX Message Queue client connected to queue: {}", self.queue_name);
        Ok(())
    }

    async fn send(&mut self, message: &Message) -> Result<()> {
        let data = message.to_bytes()?;
        let fd_ref = self.mq_fd.as_ref().ok_or_else(|| anyhow!("No message queue available"))?;
        
        // Since MqdT is just a wrapper around an fd, we can extract its raw fd
        let raw_fd = fd_ref.as_raw_fd();
        
        // Use non-blocking send with exponential backoff for queue-full conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100; // More retries since each one is much faster
        
        for attempt in 0..max_retries {
            let result = tokio::task::spawn_blocking({
                let data = data.clone();
                let raw_fd = raw_fd;
                move || {
                    // Reconstruct MqdT from raw fd for the blocking operation
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    let result = mq_send(&fd, &data, 0);
                    std::mem::forget(fd); // Don't close the fd when this MqdT drops
                    result
                }
            }).await?;
            
            match result {
                Ok(()) => {
                    debug!("Sent message {} bytes via POSIX message queue", data.len());
                    return Ok(());
                }
                Err(Errno::EAGAIN) => {
                    // Queue is full, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!("Send failed after {} attempts - queue consistently full", max_retries));
                    }
                    tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                    retry_delay_ms = (retry_delay_ms * 2).min(10); // Cap at 10ms for faster throughput
                }
                Err(e) => {
                    return Err(anyhow!("Failed to send message: {}", e));
                }
            }
        }
        
        Err(anyhow!("Send failed after {} attempts", max_retries))
    }

    async fn receive(&mut self) -> Result<Message> {
        let fd_ref = self.mq_fd.as_ref().ok_or_else(|| anyhow!("No message queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        let max_msg_size = self.max_msg_size;
        
        // Use non-blocking receive with exponential backoff for empty queue conditions
        let mut retry_delay_ms = 1;
        let max_retries = 100;
        
        for attempt in 0..max_retries {
            let result = tokio::task::spawn_blocking({
                let raw_fd = raw_fd;
                let max_msg_size = max_msg_size;
                move || {
                    let fd = unsafe { MqdT::from_raw_fd(raw_fd) };
                    let mut buffer = vec![0u8; max_msg_size];
                    let mut priority = 0u32;
                    let result = mq_receive(&fd, &mut buffer, &mut priority);
                    std::mem::forget(fd); // Don't close the fd when this MqdT drops
                    result.map(|bytes_read| {
                        buffer.truncate(bytes_read);
                        buffer
                    })
                }
            }).await?;
            
            match result {
                Ok(buffer) => {
                    debug!("Received message {} bytes via POSIX message queue", buffer.len());
                    return Message::from_bytes(&buffer);
                }
                Err(Errno::EAGAIN) => {
                    // Queue is empty, wait and retry
                    if attempt == max_retries - 1 {
                        return Err(anyhow!("Receive failed after {} attempts - queue consistently empty", max_retries));
                    }
                    tokio::time::sleep(Duration::from_millis(retry_delay_ms)).await;
                    retry_delay_ms = (retry_delay_ms * 2).min(10); // Cap at 10ms
                }
                Err(e) => {
                    return Err(anyhow!("Failed to receive message: {}", e));
                }
            }
        }
        
        Err(anyhow!("Receive failed after {} attempts", max_retries))
    }

    async fn close(&mut self) -> Result<()> {
        debug!("Closing POSIX Message Queue transport");
        let queue_name = self.queue_name.clone();
        let mq_fd = self.mq_fd.take();
        let is_creator = self.is_creator;
        
        tokio::task::spawn_blocking(move || {
            if let Some(fd) = mq_fd {
                if let Err(e) = mq_close(fd) {
                    warn!("Failed to close message queue: {}", e);
                }
            }
            
            // Only unlink if this instance created the queue
            if is_creator && !queue_name.is_empty() {
                if let Err(e) = mq_unlink(queue_name.as_str()) {
                    warn!("Failed to unlink message queue '{}': {}", queue_name, e);
                }
            }
        }).await.ok();
        
        self.state = TransportState::Disconnected;
        debug!("POSIX Message Queue transport closed");
        Ok(())
    }

    fn name(&self) -> &'static str {
        "POSIX Message Queue"
    }

    fn supports_bidirectional(&self) -> bool {
        true
    }

    fn max_message_size(&self) -> usize {
        self.max_msg_size
    }

    fn supports_multiple_connections(&self) -> bool {
        false
    }

    async fn start_multi_server(
        &mut self,
        config: &TransportConfig,
    ) -> Result<mpsc::Receiver<(ConnectionId, Message)>> {
        debug!("POSIX Message Queue multi-server mode - using single connection");
        
        self.start_server(config).await?;
        
        let (tx, rx) = mpsc::channel(1000);
        let fd_ref = self.mq_fd.as_ref().ok_or_else(|| anyhow!("No message queue available"))?;
        let raw_fd = fd_ref.as_raw_fd();
        let max_msg_size = self.max_msg_size;
        
        tokio::spawn(async move {
            let connection_id = 1;
            
            loop {
                let result = tokio::task::spawn_blocking({
                    let raw_fd_copy = raw_fd;
                    move || {
                        let fd = unsafe { MqdT::from_raw_fd(raw_fd_copy) };
                        let mut buffer = vec![0u8; max_msg_size];
                        let mut priority = 0u32;
                        let result = mq_receive(&fd, &mut buffer, &mut priority)
                            .map(|bytes_read| {
                                buffer.truncate(bytes_read);
                                buffer
                            });
                        std::mem::forget(fd); // Don't close the fd when this MqdT drops
                        result
                    }
                }).await;
                
                match result {
                    Ok(Ok(buffer)) => {
                        debug!("Received message {} bytes via POSIX message queue", buffer.len());
                        if let Ok(message) = Message::from_bytes(&buffer) {
                            if tx.send((connection_id, message)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Failed to receive message: {}", e);
                        break;
                    }
                    Err(e) => {
                        error!("Task join error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn send_to_connection(
        &mut self,
        _connection_id: ConnectionId,
        message: &Message,
    ) -> Result<()> {
        self.send(message).await
    }

    fn get_active_connections(&self) -> Vec<ConnectionId> {
        vec![1]
    }

    async fn close_connection(&mut self, _connection_id: ConnectionId) -> Result<()> {
        self.close().await
    }
} 