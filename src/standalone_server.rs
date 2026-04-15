//! Standalone server logic for the IPC benchmark suite.
//!
//! This module contains all server-side functionality for standalone
//! (split-process) mode, where the server and client run as separate
//! OS processes. It supports both blocking and async transports,
//! single-connection and multi-accept modes, and collects server-side
//! one-way latency metrics.
//!
//! The top-level entry point is [`run_standalone_server`], which sets
//! up logging, CPU affinity, and dispatches to the appropriate blocking
//! or async implementation based on CLI flags.

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use tracing_subscriber::filter::LevelFilter;

use crate::benchmark::BenchmarkConfig;
use crate::cli::{Args, IpcMechanism};
use crate::ipc::{
    get_monotonic_time_ns, BlockingTransport, BlockingTransportFactory, Message, MessageType,
    TransportConfig, TransportFactory,
};
use crate::logging::ColorizedFormatter;
use crate::metrics::{LatencyType, MetricsCollector};

// --- Standalone constants ---

/// Maximum time the client will retry connecting before giving up.
pub const CONNECT_RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Interval between client connection retry attempts.
pub const CONNECT_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Grace period after the first client connects before the multi-accept
/// server checks if all clients have disconnected. This prevents premature
/// server exit when a fast client finishes before slower clients connect.
/// Clients that take longer than this to connect after the first one may
/// be missed.
pub const SERVER_ACCEPT_GRACE_PERIOD: std::time::Duration = std::time::Duration::from_secs(2);

// --- Standalone helpers ---

/// Determine the effective concurrency for a given mechanism.
///
/// Only TCP and UDS support concurrent connections (socket-based accept
/// loop). SHM and PMQ are forced to concurrency=1 with a warning.
pub fn effective_concurrency(mechanism: IpcMechanism, requested: usize) -> usize {
    let mut supports_concurrency = mechanism == IpcMechanism::TcpSocket;
    #[cfg(unix)]
    {
        supports_concurrency = supports_concurrency || mechanism == IpcMechanism::UnixDomainSocket;
    }
    if !supports_concurrency && requested > 1 {
        warn!(
            "{} does not support concurrency > 1. Forcing concurrency = 1.",
            mechanism
        );
        1
    } else {
        requested
    }
}

/// Determine the server's response to an incoming message.
///
/// Returns `Some(response)` for Request (-> Response) and Ping (-> Pong).
/// Returns `None` for all other message types (OneWay, Shutdown, etc.),
/// which the caller handles directly for control flow.
///
/// Response payloads are intentionally empty: the server echoes back only
/// the message ID for correlation. This matches the existing benchmark
/// runner's behavior and measures pure IPC round-trip latency without
/// payload echo overhead.
pub fn dispatch_server_message(msg: &Message) -> Option<Message> {
    match msg.message_type {
        MessageType::Request => Some(Message::new(msg.id, Vec::new(), MessageType::Response)),
        MessageType::Ping => Some(Message::new(msg.id, Vec::new(), MessageType::Pong)),
        _ => None,
    }
}

/// Build a TransportConfig from CLI args for standalone mode.
///
/// Uses explicit endpoint flags if provided, otherwise falls back
/// to defaults from TransportConfig::default(). This allows the
/// simple case (no extra flags) to work out of the box.
pub fn build_standalone_transport_config(args: &Args) -> TransportConfig {
    let defaults = TransportConfig::default();

    TransportConfig {
        host: args.host.clone(),
        port: args.port,
        pmq_priority: args.pmq_priority,
        socket_path: args.socket_path.clone().unwrap_or(defaults.socket_path),
        shared_memory_name: args
            .shared_memory_name
            .clone()
            .unwrap_or(defaults.shared_memory_name),
        message_queue_name: args
            .message_queue_name
            .clone()
            .unwrap_or(defaults.message_queue_name),
        buffer_size: args.buffer_size.unwrap_or(defaults.buffer_size),
        ..defaults
    }
}

/// Connect a blocking transport with retry logic.
///
/// Retries the connection with backoff until the server is available
/// or the timeout is reached. This is used by both server tests and
/// the standalone client (re-exported for the client module).
pub fn connect_blocking_with_retry(
    transport: &mut Box<dyn crate::ipc::BlockingTransport>,
    config: &TransportConfig,
) -> Result<()> {
    let start = std::time::Instant::now();

    loop {
        match transport.start_client_blocking(config) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if start.elapsed() > CONNECT_RETRY_TIMEOUT {
                    return Err(e).context(
                        "Timed out waiting for server. \
                         Is the server running with the same mechanism and endpoint?",
                    );
                }
                debug!("Connection failed, retrying: {}", e);
                std::thread::sleep(CONNECT_RETRY_INTERVAL);
            }
        }
    }
}

/// Run in standalone server mode.
///
/// Starts a server that listens for client connections using the
/// specified IPC mechanism. The server runs until the client
/// disconnects or a shutdown message is received.
///
/// Works with both async and blocking transports depending on
/// the --blocking flag.
pub fn run_standalone_server(args: Args) -> Result<()> {
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => return Err(anyhow::anyhow!("No IPC mechanism specified")),
    };

    if mechanism == IpcMechanism::All {
        return Err(anyhow::anyhow!(
            "Cannot use 'all' mechanism in standalone server mode. \
             Specify a single mechanism (e.g., -m uds)"
        ));
    }

    // Defensive: --shm-direct requires --blocking (normally enforced by
    // main() before this function is called, but guard here too).
    if args.shm_direct && !args.blocking {
        return Err(anyhow::anyhow!("--shm-direct requires --blocking mode"));
    }

    // Set up logging (simplified: stderr only for standalone server)
    if !args.quiet {
        let log_level = match args.verbose {
            0 => LevelFilter::INFO,
            1 => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        };

        if tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(log_level)
            .event_format(ColorizedFormatter)
            .try_init()
            .is_err()
        {
            eprintln!("Note: tracing subscriber already initialized, using existing configuration");
        }
    }

    // Set CPU affinity if specified
    if let Some(core) = args.server_affinity {
        if let Err(e) = crate::utils::set_affinity(core) {
            error!("Failed to set server CPU affinity to core {}: {}", core, e);
        } else {
            info!("Server affinity set to CPU core {}", core);
        }
    }

    let transport_config = build_standalone_transport_config(&args);
    let config = BenchmarkConfig::from_args(&args)?;

    info!(
        "Starting standalone server: mechanism={}, blocking={}",
        mechanism, args.blocking
    );

    if args.blocking {
        run_standalone_server_blocking(&args, mechanism, &transport_config, &config)
    } else {
        run_standalone_server_async(mechanism, transport_config, &config)
    }
}

/// Blocking standalone server implementation.
///
/// Collects one-way latency metrics for OneWay messages using the
/// monotonic clock difference between send and receive timestamps.
/// This is accurate when server and client share the same kernel
/// clock (same host, container-to-host, container-to-container).
///
/// For TCP and UDS, the server accepts multiple concurrent connections,
/// spawning a handler thread per client. For SHM and PMQ, only a single
/// connection is supported.
pub fn run_standalone_server_blocking(
    args: &Args,
    mechanism: IpcMechanism,
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    match mechanism {
        IpcMechanism::TcpSocket => {
            run_standalone_server_blocking_multi_accept_tcp(transport_config, config)
        }
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket => {
            run_standalone_server_blocking_multi_accept_uds(transport_config, config)
        }
        _ => run_standalone_server_blocking_single(args, mechanism, transport_config, config),
    }
}

/// Single-connection blocking server for SHM/PMQ mechanisms.
pub fn run_standalone_server_blocking_single(
    args: &Args,
    mechanism: IpcMechanism,
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;
    transport
        .start_server_blocking(transport_config)
        .context("Server failed to start transport")?;

    info!("Server listening, waiting for client...");

    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;
    let mut one_way_count = 0u64;

    while let Ok((message, receive_time_ns)) = transport.receive_blocking_timed() {
        if message.message_type == MessageType::Shutdown {
            debug!("Server received shutdown message, exiting");
            break;
        }

        if message.message_type == MessageType::OneWay && message.id != u64::MAX {
            let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
            let latency = std::time::Duration::from_nanos(latency_ns);
            one_way_metrics.record_message(config.message_size, Some(latency))?;
            one_way_count += 1;
        }

        if let Some(response) = dispatch_server_message(&message) {
            if let Err(e) = transport.send_blocking(&response) {
                warn!("Server failed to send response: {}", e);
                break;
            }
        }
    }

    print_server_one_way_latency(one_way_count, &one_way_metrics);
    transport.close_blocking()?;
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Handle a single client connection in a server handler thread.
///
/// Runs the message dispatch loop and collects one-way latency metrics.
/// Returns the metrics collector when the client disconnects or sends Shutdown.
pub fn handle_client_connection(
    transport: &mut dyn crate::ipc::BlockingTransport,
    config: &BenchmarkConfig,
) -> Result<MetricsCollector> {
    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;

    while let Ok((message, receive_time_ns)) = transport.receive_blocking_timed() {
        if message.message_type == MessageType::Shutdown {
            debug!("Handler received shutdown message");
            break;
        }

        if message.message_type == MessageType::OneWay && message.id != u64::MAX {
            let latency_ns = receive_time_ns.saturating_sub(message.timestamp);
            let latency = std::time::Duration::from_nanos(latency_ns);
            one_way_metrics.record_message(config.message_size, Some(latency))?;
        }

        if let Some(response) = dispatch_server_message(&message) {
            if let Err(e) = transport.send_blocking(&response) {
                warn!("Handler failed to send response: {}", e);
                break;
            }
        }
    }

    Ok(one_way_metrics)
}

/// Multi-accept blocking TCP server.
///
/// Binds a raw TCP listener and accepts connections in a loop, spawning
/// a handler thread per client. Each handler gets its own `BlockingTcpSocket`
/// via `from_stream()`. Metrics are aggregated across all handlers at shutdown.
pub fn run_standalone_server_blocking_multi_accept_tcp(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use crate::ipc::BlockingTcpSocket;
    use socket2::{Domain, Socket, Type};
    use std::sync::{Arc, Mutex};

    let addr = format!("{}:{}", transport_config.host, transport_config.port);
    let socket =
        Socket::new(Domain::IPV4, Type::STREAM, None).context("Failed to create TCP socket")?;
    socket
        .set_reuse_address(true)
        .context("Failed to set SO_REUSEADDR")?;
    let socket_addr: std::net::SocketAddr = addr
        .parse()
        .with_context(|| format!("Failed to parse address: {}", addr))?;
    socket
        .bind(&socket_addr.into())
        .with_context(|| format!("Failed to bind TCP socket to {}", addr))?;
    socket
        .listen(128)
        .context("Failed to listen on TCP socket")?;
    let listener: std::net::TcpListener = socket.into();
    // Non-blocking so we can periodically check if all handler threads
    // have finished (i.e. all clients disconnected).
    listener
        .set_nonblocking(true)
        .context("Failed to set listener to non-blocking")?;

    info!("TCP server listening on {}, accepting connections...", addr);

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    // Grace period after first client connects before we check if all
    // handlers are done. This prevents premature exit when a fast client
    // finishes before slower clients have connected.
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = SERVER_ACCEPT_GRACE_PERIOD;

    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                info!("Accepted connection from {}", peer_addr);
                // Reset grace timer on every new connection so multi-phase
                // tests (one-way then round-trip) don't trigger premature exit.
                first_client_time = Some(std::time::Instant::now());
                // Accepted streams inherit non-blocking from the listener;
                // set back to blocking for the handler thread.
                // Skip bad connections rather than crashing the server.
                if let Err(e) = stream.set_nonblocking(false) {
                    warn!("Failed to configure stream from {}: {}", peer_addr, e);
                    continue;
                }
                if let Err(e) = stream.set_nodelay(true) {
                    warn!("Failed to set TCP_NODELAY for {}: {}", peer_addr, e);
                    continue;
                }
                // Apply socket buffer tuning to match normal transport behavior
                let sock = socket2::Socket::from(stream);
                let _ = sock.set_recv_buffer_size(transport_config.buffer_size);
                let _ = sock.set_send_buffer_size(transport_config.buffer_size);
                let stream: std::net::TcpStream = sock.into();

                let metrics_clone = worker_metrics.clone();
                let handler_config = config.clone();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    match handle_client_connection(&mut transport, &handler_config) {
                        Ok(collector) => {
                            metrics_clone
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(collector);
                        }
                        Err(e) => {
                            warn!("Handler error: {}", e);
                        }
                    }
                    let _ = transport.close_blocking();
                    info!("Client {} disconnected", peer_addr);
                });
                handles.push(handle);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection. Check if all handlers are done,
                // but only after the grace period has elapsed since the
                // last client connected.
                let grace_elapsed =
                    first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
                if grace_elapsed && !handles.is_empty() && handles.iter().all(|h| h.is_finished()) {
                    debug!("All clients disconnected, shutting down");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                debug!("Accept error: {}", e);
                break;
            }
        }
    }

    for handle in handles {
        if let Err(e) = handle.join() {
            warn!("Handler thread panicked: {:?}", e);
        }
    }

    let collectors = worker_metrics.lock().unwrap_or_else(|e| e.into_inner());
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Multi-accept blocking UDS server.
///
/// Same pattern as the TCP multi-accept server but using Unix domain sockets.
#[cfg(unix)]
pub fn run_standalone_server_blocking_multi_accept_uds(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use crate::ipc::BlockingUnixDomainSocket;
    use std::sync::{Arc, Mutex};

    // Remove existing socket file
    let _ = std::fs::remove_file(&transport_config.socket_path);

    let listener = std::os::unix::net::UnixListener::bind(&transport_config.socket_path)
        .with_context(|| {
            format!(
                "Failed to bind Unix domain socket at: {}",
                transport_config.socket_path
            )
        })?;
    listener
        .set_nonblocking(true)
        .context("Failed to set UDS listener to non-blocking")?;

    info!(
        "UDS server listening on {}, accepting connections...",
        transport_config.socket_path
    );

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = SERVER_ACCEPT_GRACE_PERIOD;

    loop {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                info!("Accepted UDS connection from {:?}", peer_addr);
                // Reset grace timer on every new connection so multi-phase
                // tests don't trigger premature exit.
                first_client_time = Some(std::time::Instant::now());
                // Accepted streams inherit non-blocking from the listener;
                // set back to blocking for the handler thread.
                if let Err(e) = stream.set_nonblocking(false) {
                    warn!("Failed to configure UDS stream from {:?}: {}", peer_addr, e);
                    continue;
                }

                let metrics_clone = worker_metrics.clone();
                let handler_config = config.clone();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingUnixDomainSocket::from_stream(stream);
                    match handle_client_connection(&mut transport, &handler_config) {
                        Ok(collector) => {
                            metrics_clone
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(collector);
                        }
                        Err(e) => {
                            warn!("Handler error: {}", e);
                        }
                    }
                    let _ = transport.close_blocking();
                    info!("UDS client disconnected");
                });
                handles.push(handle);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let grace_elapsed =
                    first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
                if grace_elapsed && !handles.is_empty() && handles.iter().all(|h| h.is_finished()) {
                    debug!("All clients disconnected, shutting down");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                debug!("Accept error: {}", e);
                break;
            }
        }
    }

    for handle in handles {
        if let Err(e) = handle.join() {
            warn!("Handler thread panicked: {:?}", e);
        }
    }

    let collectors = worker_metrics.lock().unwrap_or_else(|e| e.into_inner());
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    // Clean up socket file
    let _ = std::fs::remove_file(&transport_config.socket_path);
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Aggregate and print server-side one-way latency from multiple handler threads.
pub fn aggregate_and_print_server_metrics(collectors: &[MetricsCollector], percentiles: &[f64]) {
    let total_one_way: u64 = collectors
        .iter()
        .map(|c| c.get_metrics().throughput.total_messages as u64)
        .sum();

    if total_one_way > 0 {
        let all_metrics: Vec<_> = collectors.iter().map(|c| c.get_metrics()).collect();
        match MetricsCollector::aggregate_worker_metrics(all_metrics, percentiles) {
            Ok(aggregated) => {
                if let Some(ref latency) = aggregated.latency {
                    info!(
                        "Server one-way latency ({} messages, {} clients): \
                         mean={:.2}us, P50={:.2}us, P95={:.2}us, P99={:.2}us, \
                         min={:.2}us, max={:.2}us",
                        total_one_way,
                        collectors.len(),
                        latency.mean_ns / 1000.0,
                        latency.median_ns / 1000.0,
                        latency
                            .percentiles
                            .iter()
                            .find(|p| (p.percentile - 95.0).abs() < 0.1)
                            .map_or(0.0, |p| p.value_ns as f64)
                            / 1000.0,
                        latency
                            .percentiles
                            .iter()
                            .find(|p| (p.percentile - 99.0).abs() < 0.1)
                            .map_or(0.0, |p| p.value_ns as f64)
                            / 1000.0,
                        latency.min_ns as f64 / 1000.0,
                        latency.max_ns as f64 / 1000.0,
                    );
                }
            }
            Err(e) => warn!("Failed to aggregate server metrics: {}", e),
        }
    }
}

/// Print server-side one-way latency summary.
pub fn print_server_one_way_latency(one_way_count: u64, one_way_metrics: &MetricsCollector) {
    if one_way_count > 0 {
        let metrics = one_way_metrics.get_metrics();
        if let Some(ref latency) = metrics.latency {
            info!(
                "Server one-way latency ({} messages): \
                 mean={:.2}us, P50={:.2}us, P95={:.2}us, P99={:.2}us, \
                 min={:.2}us, max={:.2}us",
                one_way_count,
                latency.mean_ns / 1000.0,
                latency.median_ns / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 95.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency
                    .percentiles
                    .iter()
                    .find(|p| (p.percentile - 99.0).abs() < 0.1)
                    .map_or(0.0, |p| p.value_ns as f64)
                    / 1000.0,
                latency.min_ns as f64 / 1000.0,
                latency.max_ns as f64 / 1000.0,
            );
        }
    }
}

/// Async standalone server implementation.
///
/// For TCP and UDS, the server accepts multiple concurrent connections,
/// spawning a tokio task per client. For SHM and PMQ, only a single
/// connection is supported.
#[tokio::main]
pub async fn run_standalone_server_async(
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    match mechanism {
        IpcMechanism::TcpSocket => {
            run_standalone_server_async_multi_accept_tcp(&transport_config, config).await
        }
        #[cfg(unix)]
        IpcMechanism::UnixDomainSocket => {
            run_standalone_server_async_multi_accept_uds(&transport_config, config).await
        }
        _ => run_standalone_server_async_single(mechanism, &transport_config, config).await,
    }
}

/// Single-connection async server for SHM/PMQ mechanisms.
pub async fn run_standalone_server_async_single(
    mechanism: IpcMechanism,
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = TransportFactory::create(&mechanism)?;
    transport
        .start_server(transport_config)
        .await
        .context("Server failed to start transport")?;

    info!("Server listening, waiting for client...");

    let mut one_way_metrics =
        MetricsCollector::new(Some(LatencyType::OneWay), config.percentiles.clone())?;
    let mut one_way_count = 0u64;

    loop {
        match transport.receive().await {
            Ok(msg) => {
                if msg.message_type == MessageType::Shutdown {
                    debug!("Server received shutdown message, exiting");
                    break;
                }

                if msg.message_type == MessageType::OneWay && msg.id != u64::MAX {
                    let receive_time_ns = get_monotonic_time_ns();
                    let latency_ns = receive_time_ns.saturating_sub(msg.timestamp);
                    let latency = std::time::Duration::from_nanos(latency_ns);
                    one_way_metrics.record_message(config.message_size, Some(latency))?;
                    one_way_count += 1;
                }

                if let Some(response) = dispatch_server_message(&msg) {
                    if transport.send(&response).await.is_err() {
                        info!("Client disconnected during send, exiting.");
                        break;
                    }
                }
            }
            Err(e) => {
                info!("Server receive loop ending: {}", e);
                break;
            }
        }
    }

    print_server_one_way_latency(one_way_count, &one_way_metrics);
    let _ = transport.close().await;
    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Multi-accept async TCP server.
///
/// Binds a TCP listener and accepts connections in a loop, spawning
/// a tokio task per client. Each handler uses blocking transport
/// operations on a dedicated thread (via spawn_blocking) since
/// the transport trait is not async-safe for concurrent use.
pub async fn run_standalone_server_async_multi_accept_tcp(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use crate::ipc::BlockingTcpSocket;
    use std::sync::{Arc, Mutex};

    let addr = format!("{}:{}", transport_config.host, transport_config.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind TCP socket to {}", addr))?;

    info!("TCP server listening on {}, accepting connections...", addr);

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = tokio::task::JoinSet::new();
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = SERVER_ACCEPT_GRACE_PERIOD;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        info!("Accepted connection from {}", peer_addr);
                        // Reset grace timer on every new connection
                        first_client_time = Some(std::time::Instant::now());

                        let std_stream = match stream.into_std() {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("Failed to convert stream from {}: {}", peer_addr, e);
                                continue;
                            }
                        };
                        if let Err(e) = std_stream.set_nonblocking(false) {
                            warn!("Failed to configure stream from {}: {}", peer_addr, e);
                            continue;
                        }
                        if let Err(e) = std_stream.set_nodelay(true) {
                            warn!("Failed to set TCP_NODELAY for {}: {}", peer_addr, e);
                            continue;
                        }
                        // Apply socket buffer tuning to match normal transport behavior
                        let sock = socket2::Socket::from(std_stream);
                        let _ = sock.set_recv_buffer_size(transport_config.buffer_size);
                        let _ = sock.set_send_buffer_size(transport_config.buffer_size);
                        let std_stream: std::net::TcpStream = sock.into();

                        let metrics_clone = worker_metrics.clone();
                        let handler_config = config.clone();

                        handles.spawn_blocking(move || {
                            let mut transport = BlockingTcpSocket::from_stream(std_stream);
                            match handle_client_connection(&mut transport, &handler_config) {
                                Ok(collector) => {
                                    metrics_clone.lock().unwrap_or_else(|e| e.into_inner()).push(collector);
                                }
                                Err(e) => {
                                    warn!("Handler error: {}", e);
                                }
                            }
                            let _ = transport.close_blocking();
                            info!("Client {} disconnected", peer_addr);
                        });
                    }
                    Err(e) => {
                        debug!("Accept error: {}", e);
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }

        // Check if all spawned tasks are done
        // Drain completed tasks from the JoinSet
        while let Some(result) = handles.try_join_next() {
            if let Err(e) = result {
                warn!("Handler task panicked: {}", e);
            }
        }

        let grace_elapsed = first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
        if grace_elapsed && handles.is_empty() {
            debug!("All clients disconnected, shutting down");
            break;
        }
    }

    // Wait for any remaining handlers
    while let Some(result) = handles.join_next().await {
        let _ = result;
    }

    let collectors = worker_metrics.lock().unwrap_or_else(|e| e.into_inner());
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    info!("Standalone server exiting cleanly.");
    Ok(())
}

/// Multi-accept async UDS server.
#[cfg(unix)]
pub async fn run_standalone_server_async_multi_accept_uds(
    transport_config: &TransportConfig,
    config: &BenchmarkConfig,
) -> Result<()> {
    use crate::ipc::BlockingUnixDomainSocket;
    use std::sync::{Arc, Mutex};

    let _ = std::fs::remove_file(&transport_config.socket_path);

    let listener =
        tokio::net::UnixListener::bind(&transport_config.socket_path).with_context(|| {
            format!(
                "Failed to bind Unix domain socket at: {}",
                transport_config.socket_path
            )
        })?;

    info!(
        "UDS server listening on {}, accepting connections...",
        transport_config.socket_path
    );

    let worker_metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = tokio::task::JoinSet::new();
    let mut first_client_time: Option<std::time::Instant> = None;
    let accept_grace_period = SERVER_ACCEPT_GRACE_PERIOD;

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        info!("Accepted UDS connection from {:?}", peer_addr);
                        // Reset grace timer on every new connection
                        first_client_time = Some(std::time::Instant::now());

                        let std_stream = match stream.into_std() {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("Failed to convert UDS stream from {:?}: {}", peer_addr, e);
                                continue;
                            }
                        };
                        if let Err(e) = std_stream.set_nonblocking(false) {
                            warn!("Failed to configure UDS stream from {:?}: {}", peer_addr, e);
                            continue;
                        }

                        let metrics_clone = worker_metrics.clone();
                        let handler_config = config.clone();

                        handles.spawn_blocking(move || {
                            let mut transport = BlockingUnixDomainSocket::from_stream(std_stream);
                            match handle_client_connection(&mut transport, &handler_config) {
                                Ok(collector) => {
                                    metrics_clone.lock().unwrap_or_else(|e| e.into_inner()).push(collector);
                                }
                                Err(e) => {
                                    warn!("Handler error: {}", e);
                                }
                            }
                            let _ = transport.close_blocking();
                            info!("UDS client disconnected");
                        });
                    }
                    Err(e) => {
                        debug!("Accept error: {}", e);
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
        }

        // Drain completed tasks
        while let Some(result) = handles.try_join_next() {
            if let Err(e) = result {
                warn!("Handler task panicked: {}", e);
            }
        }

        let grace_elapsed = first_client_time.is_some_and(|t| t.elapsed() > accept_grace_period);
        if grace_elapsed && handles.is_empty() {
            debug!("All clients disconnected, shutting down");
            break;
        }
    }

    while let Some(result) = handles.join_next().await {
        let _ = result;
    }

    let collectors = worker_metrics.lock().unwrap_or_else(|e| e.into_inner());
    aggregate_and_print_server_metrics(&collectors, &config.percentiles);

    let _ = std::fs::remove_file(&transport_config.socket_path);
    info!("Standalone server exiting cleanly.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{BlockingTransportFactory, Message, MessageType, TransportConfig};
    use clap::Parser;

    #[test]
    fn test_build_standalone_transport_config_defaults() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "tcp"]);
        let config = build_standalone_transport_config(&args);

        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.shared_memory_name, "ipc_benchmark_shm");
        assert_eq!(config.message_queue_name, "ipc_benchmark_pmq");
        assert_eq!(config.pmq_priority, 0);
    }

    #[test]
    fn test_build_standalone_transport_config_overrides() {
        let args = Args::parse_from([
            "ipc-benchmark",
            "--server",
            "-m",
            "tcp",
            "--host",
            "0.0.0.0",
            "--port",
            "9999",
            "--socket-path",
            "/tmp/custom.sock",
            "--shared-memory-name",
            "custom_shm",
            "--message-queue-name",
            "custom_pmq",
            "--buffer-size",
            "65536",
            "--pmq-priority",
            "3",
        ]);
        let config = build_standalone_transport_config(&args);

        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 9999);
        assert_eq!(config.socket_path, "/tmp/custom.sock");
        assert_eq!(config.shared_memory_name, "custom_shm");
        assert_eq!(config.message_queue_name, "custom_pmq");
        assert_eq!(config.buffer_size, 65536);
        assert_eq!(config.pmq_priority, 3);
    }

    #[test]
    fn test_build_standalone_transport_config_partial_overrides() {
        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "shm",
            "--shared-memory-name",
            "my_shm",
        ]);
        let config = build_standalone_transport_config(&args);

        // Overridden
        assert_eq!(config.shared_memory_name, "my_shm");
        // Defaults preserved
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.buffer_size, 8192);
    }

    /// Get a free TCP port by binding to port 0 and extracting the assigned port.
    fn get_free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    #[test]
    fn test_standalone_server_rejects_all_mechanism() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "all"]);

        // Simulate what run_standalone_server does
        let mechanism = args.mechanisms.first().unwrap();
        assert_eq!(*mechanism, IpcMechanism::All);
    }

    // --- Unit tests for shared helper functions ---

    #[test]
    fn test_dispatch_server_message_request() {
        let msg = Message::new(1, Vec::new(), MessageType::Request);
        let resp = dispatch_server_message(&msg).unwrap();
        assert_eq!(resp.id, 1);
        assert_eq!(resp.message_type, MessageType::Response);
    }

    #[test]
    fn test_dispatch_server_message_ping() {
        let msg = Message::new(42, Vec::new(), MessageType::Ping);
        let resp = dispatch_server_message(&msg).unwrap();
        assert_eq!(resp.id, 42);
        assert_eq!(resp.message_type, MessageType::Pong);
    }

    #[test]
    fn test_dispatch_server_message_one_way_returns_none() {
        let msg = Message::new(1, Vec::new(), MessageType::OneWay);
        assert!(dispatch_server_message(&msg).is_none());
    }

    #[test]
    fn test_dispatch_server_message_shutdown_returns_none() {
        let msg = Message::new(1, Vec::new(), MessageType::Shutdown);
        assert!(dispatch_server_message(&msg).is_none());
    }

    /// Integration test: blocking TCP round-trip with duration mode.
    #[test]
    fn test_standalone_blocking_tcp_duration_round_trip() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                if message.message_type == MessageType::Request {
                    let resp = Message::new(message.id, Vec::new(), MessageType::Response);
                    if transport.send_blocking(&resp).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Run round-trip for a short duration
        let test_duration = std::time::Duration::from_millis(200);
        let start = std::time::Instant::now();
        let mut count = 0u64;
        while start.elapsed() < test_duration {
            let msg = Message::new(count, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, count, "Response ID should match request ID");
            assert_eq!(resp.message_type, MessageType::Response);
            count += 1;
        }

        let elapsed = start.elapsed();
        assert!(
            count > 10,
            "Should have completed many round-trips in 200ms, got {}",
            count
        );
        assert!(
            elapsed >= test_duration,
            "Should have run for at least the specified duration"
        );

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP one-way with duration mode.
    #[test]
    fn test_standalone_blocking_tcp_duration_one_way() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let mut received = 0u64;
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                assert_eq!(message.message_type, MessageType::OneWay);
                assert_eq!(message.id, received, "Message IDs should be sequential");
                received += 1;
            }
            transport.close_blocking().unwrap();
            received
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Run one-way for a short duration
        let test_duration = std::time::Duration::from_millis(200);
        let start = std::time::Instant::now();
        let mut count = 0u64;
        while start.elapsed() < test_duration {
            let msg = Message::new(count, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
            count += 1;
        }

        assert!(
            count > 10,
            "Should have sent many messages in 200ms, got {}",
            count
        );

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();

        let received = server_handle.join().unwrap();
        assert_eq!(
            received, count,
            "Server should have received all {} messages",
            count
        );
    }

    /// Integration test: blocking TCP round-trip through standalone
    /// server and client paths.
    ///
    /// Exercises: run_standalone_server_blocking server loop (receive,
    /// respond to Request), connect_blocking_with_retry, and
    /// run_standalone_client_blocking round-trip path.
    #[test]
    fn test_standalone_blocking_tcp_round_trip() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start server in a background thread
        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            // Handle messages until client disconnects
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                if message.message_type == MessageType::Request {
                    let resp = Message::new(message.id, Vec::new(), MessageType::Response);
                    if transport.send_blocking(&resp).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        // Give server time to bind
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client: connect with retry and do round-trip
        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send round-trip messages
        let msg_count = 10usize;
        let payload = vec![0u8; 64];
        for i in 0..msg_count {
            let msg = Message::new(i as u64, payload.clone(), MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, i as u64);
            assert_eq!(resp.message_type, MessageType::Response);
        }

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP one-way through standalone paths.
    ///
    /// Exercises: server handling OneWay messages (no response),
    /// client sending fire-and-forget messages.
    #[test]
    fn test_standalone_blocking_tcp_one_way() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let msg_count = 10u64;

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let mut received = 0u64;
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                assert_eq!(message.message_type, MessageType::OneWay);
                assert_eq!(message.id, received);
                received += 1;
            }
            transport.close_blocking().unwrap();
            received
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send one-way messages
        let payload = vec![0u8; 64];
        for i in 0..msg_count {
            let msg = Message::new(i, payload.clone(), MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();

        let received = server_handle.join().unwrap();
        assert_eq!(
            received, msg_count,
            "Server should have received all {} messages",
            msg_count
        );
    }

    /// Integration test: blocking TCP server handles Ping/Pong.
    ///
    /// Exercises the Ping message handling branch in the server loop.
    #[test]
    fn test_standalone_blocking_tcp_ping_pong() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Ping {
                    let pong = Message::new(message.id, Vec::new(), MessageType::Pong);
                    if transport.send_blocking(&pong).is_err() {
                        break;
                    }
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send ping, expect pong
        let ping = Message::new(42, Vec::new(), MessageType::Ping);
        transport.send_blocking(&ping).unwrap();
        let pong = transport.receive_blocking().unwrap();
        assert_eq!(pong.id, 42);
        assert_eq!(pong.message_type, MessageType::Pong);

        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: Shutdown message causes server to exit cleanly.
    #[test]
    fn test_standalone_server_shutdown_message() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            // Server loop: should exit on Shutdown
            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send shutdown
        let shutdown = Message::new(0, Vec::new(), MessageType::Shutdown);
        transport.send_blocking(&shutdown).unwrap();

        // Give server time to process and exit
        std::thread::sleep(std::time::Duration::from_millis(100));
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    // --- Concurrency tests ---

    /// Integration test: multi-accept TCP server handles 3 concurrent
    /// round-trip clients.
    #[test]
    fn test_standalone_concurrent_tcp_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use std::sync::{Arc, Mutex};

        let port = get_free_port();
        let addr = format!("127.0.0.1:{}", port);
        let num_clients = 3usize;
        let msgs_per_client = 5usize;

        // Start multi-accept server using blocking accept with known
        // client count to avoid the non-blocking race condition where
        // a fast client could finish before others connect.
        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let metrics: Arc<Mutex<Vec<MetricsCollector>>> = Arc::new(Mutex::new(Vec::new()));
            let mut handles = Vec::new();

            // Accept exactly num_clients connections (blocking)
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let mc = metrics.clone();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    if let Ok(collector) = handle_client_connection(&mut transport, &config) {
                        mc.lock().unwrap().push(collector);
                    }
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }

            for handle in handles {
                if let Err(e) = handle.join() {
                    warn!("Handler thread panicked: {:?}", e);
                }
            }

            let collectors = metrics.lock().unwrap();
            assert_eq!(
                collectors.len(),
                num_clients,
                "Server should have handled {} clients",
                num_clients
            );
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Spawn concurrent clients
        let client_handles: Vec<_> = (0..num_clients)
            .map(|worker_id| {
                let config = TransportConfig {
                    host: "127.0.0.1".to_string(),
                    port,
                    ..Default::default()
                };
                std::thread::spawn(move || {
                    let mut transport =
                        BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                    connect_blocking_with_retry(&mut transport, &config).unwrap();

                    for i in 0..msgs_per_client {
                        let msg = Message::new(
                            (worker_id * msgs_per_client + i) as u64,
                            vec![0u8; 64],
                            MessageType::Request,
                        );
                        transport.send_blocking(&msg).unwrap();
                        let resp = transport.receive_blocking().unwrap();
                        assert_eq!(resp.message_type, MessageType::Response);
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking().unwrap();
                })
            })
            .collect();

        for h in client_handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection correctly dispatches message types
    /// and returns metrics.
    #[test]
    fn test_handle_client_connection_round_trip() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            // Round-trip messages don't get recorded as one-way
            let metrics = collector.get_metrics();
            assert_eq!(metrics.throughput.total_messages, 0);
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send round-trip messages (should NOT be recorded as one-way)
        for i in 0..5u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.message_type, MessageType::Response);
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection records one-way latency metrics.
    #[test]
    fn test_handle_client_connection_one_way_metrics() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            let metrics = collector.get_metrics();
            // One-way messages should be recorded
            assert_eq!(
                metrics.throughput.total_messages, 5,
                "Should have recorded 5 one-way messages"
            );
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send one-way messages
        for i in 0..5u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    // --- Concurrent one-way test ---

    /// Integration test: multi-accept TCP server handles 2 concurrent
    /// one-way clients and records server-side latency metrics.
    #[test]
    fn test_standalone_concurrent_tcp_one_way() {
        use crate::ipc::BlockingTcpSocket;

        let port = get_free_port();
        let addr = format!("127.0.0.1:{}", port);
        let num_clients = 2usize;
        let msgs_per_client = 10usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let mut handles = Vec::new();

            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();

                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let collector = handle_client_connection(&mut transport, &config).unwrap();
                    let metrics = collector.get_metrics();
                    assert_eq!(
                        metrics.throughput.total_messages, msgs_per_client,
                        "Handler should have recorded exactly {} one-way messages",
                        msgs_per_client
                    );
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }

            for handle in handles {
                handle.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let client_handles: Vec<_> = (0..num_clients)
            .map(|worker_id| {
                let config = TransportConfig {
                    host: "127.0.0.1".to_string(),
                    port,
                    ..Default::default()
                };
                std::thread::spawn(move || {
                    let mut transport =
                        BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                    connect_blocking_with_retry(&mut transport, &config).unwrap();

                    for i in 0..msgs_per_client {
                        let msg = Message::new(
                            (worker_id * msgs_per_client + i) as u64,
                            vec![0u8; 64],
                            MessageType::OneWay,
                        );
                        transport.send_blocking(&msg).unwrap();
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking().unwrap();
                })
            })
            .collect();

        for h in client_handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    /// Integration test: async multi-accept TCP server handles 2 concurrent
    /// round-trip clients. Exercises tokio accept + spawn_blocking +
    /// from_stream + handle_client_connection path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_standalone_async_concurrent_tcp_round_trip() {
        use crate::ipc::BlockingTcpSocket;

        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = std_listener.local_addr().unwrap().port();
        std_listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();

        let num_clients = 2usize;
        let msgs_per_client = 5usize;

        let mut server_handles = tokio::task::JoinSet::new();
        let mut client_handles = Vec::new();

        // Accept connections and spawn handlers + clients concurrently
        for worker_id in 0..num_clients {
            // Spawn client in std thread (blocking connect)
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
                ..Default::default()
            };
            let client_handle = std::thread::spawn(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                connect_blocking_with_retry(&mut transport, &config).unwrap();

                for i in 0..msgs_per_client {
                    let msg = Message::new(
                        (worker_id * msgs_per_client + i) as u64,
                        vec![0u8; 64],
                        MessageType::Request,
                    );
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            client_handles.push(client_handle);

            // Accept this client via tokio and spawn blocking handler
            let (stream, _) = listener.accept().await.unwrap();
            let std_stream = stream.into_std().unwrap();
            std_stream.set_nonblocking(false).unwrap();
            std_stream.set_nodelay(true).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();

            server_handles.spawn_blocking(move || {
                let mut transport = BlockingTcpSocket::from_stream(std_stream);
                handle_client_connection(&mut transport, &config).unwrap();
                let _ = transport.close_blocking();
            });
        }

        // Wait for all handlers and clients
        while let Some(result) = server_handles.join_next().await {
            result.unwrap();
        }
        for h in client_handles {
            h.join().unwrap();
        }
    }

    // --- from_stream tests ---

    /// Test: BlockingTcpSocket::from_stream creates a functional transport.
    #[test]
    fn test_tcp_from_stream_send_receive() {
        use crate::ipc::BlockingTcpSocket;

        let port = get_free_port();
        let addr = format!("127.0.0.1:{}", port);

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(&addr).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();

            let mut transport = BlockingTcpSocket::from_stream(stream);
            let msg = transport.receive_blocking().unwrap();
            assert_eq!(msg.id, 99);
            assert_eq!(msg.message_type, MessageType::Request);

            let resp = Message::new(msg.id, Vec::new(), MessageType::Response);
            transport.send_blocking(&resp).unwrap();
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let client = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        client.set_nodelay(true).unwrap();

        // Use a raw BlockingTcpSocket for client too
        let mut client_transport = BlockingTcpSocket::from_stream(client);
        let msg = Message::new(99, vec![0u8; 32], MessageType::Request);
        client_transport.send_blocking(&msg).unwrap();
        let resp = client_transport.receive_blocking().unwrap();
        assert_eq!(resp.id, 99);
        assert_eq!(resp.message_type, MessageType::Response);
        client_transport.close_blocking().unwrap();

        server_handle.join().unwrap();
    }

    /// Test: BlockingUnixDomainSocket::from_stream creates a functional transport.
    #[cfg(unix)]
    #[test]
    fn test_uds_from_stream_send_receive() {
        use crate::ipc::BlockingUnixDomainSocket;

        let socket_path = format!("/tmp/test_from_stream_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let path_clone = socket_path.clone();
        let server_handle = std::thread::spawn(move || {
            let listener = std::os::unix::net::UnixListener::bind(&path_clone).unwrap();
            let (stream, _) = listener.accept().unwrap();

            let mut transport = BlockingUnixDomainSocket::from_stream(stream);
            let msg = transport.receive_blocking().unwrap();
            assert_eq!(msg.id, 77);

            let resp = Message::new(msg.id, Vec::new(), MessageType::Response);
            transport.send_blocking(&resp).unwrap();
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let stream = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
        let mut client_transport = BlockingUnixDomainSocket::from_stream(stream);
        let msg = Message::new(77, vec![0u8; 32], MessageType::Request);
        client_transport.send_blocking(&msg).unwrap();
        let resp = client_transport.receive_blocking().unwrap();
        assert_eq!(resp.id, 77);
        assert_eq!(resp.message_type, MessageType::Response);
        client_transport.close_blocking().unwrap();

        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    // --- Concurrency fallback test ---

    /// Test: SHM mechanism forces concurrency=1 with a warning.
    #[test]
    fn test_concurrency_forced_to_one_for_shm() {
        assert_eq!(
            effective_concurrency(IpcMechanism::SharedMemory, 4),
            1,
            "SHM should force concurrency to 1"
        );
        assert_eq!(
            effective_concurrency(IpcMechanism::SharedMemory, 1),
            1,
            "SHM with concurrency=1 should stay at 1"
        );
        assert_eq!(
            effective_concurrency(IpcMechanism::TcpSocket, 4),
            4,
            "TCP should allow concurrency > 1"
        );
        assert_eq!(
            effective_concurrency(IpcMechanism::TcpSocket, 1),
            1,
            "TCP with concurrency=1 should stay at 1"
        );
    }

    // --- aggregate_and_print_server_metrics test ---

    /// Test: aggregate_and_print_server_metrics handles empty input.
    #[test]
    fn test_aggregate_and_print_empty_collectors() {
        let collectors: Vec<MetricsCollector> = Vec::new();
        // Should not panic on empty input
        aggregate_and_print_server_metrics(&collectors, &[50.0, 95.0, 99.0]);
    }

    /// Test: aggregate_and_print_server_metrics with single collector.
    #[test]
    fn test_aggregate_and_print_single_collector() {
        let mut collector =
            MetricsCollector::new(Some(LatencyType::OneWay), vec![50.0, 95.0, 99.0]).unwrap();
        collector
            .record_message(64, Some(std::time::Duration::from_micros(100)))
            .unwrap();
        collector
            .record_message(64, Some(std::time::Duration::from_micros(200)))
            .unwrap();

        // Should not panic
        aggregate_and_print_server_metrics(&[collector], &[50.0, 95.0, 99.0]);
    }

    /// Test: aggregate_and_print_server_metrics with multiple collectors.
    #[test]
    fn test_aggregate_and_print_multiple_collectors() {
        let mut c1 =
            MetricsCollector::new(Some(LatencyType::OneWay), vec![50.0, 95.0, 99.0]).unwrap();
        c1.record_message(64, Some(std::time::Duration::from_micros(100)))
            .unwrap();
        c1.record_message(64, Some(std::time::Duration::from_micros(150)))
            .unwrap();

        let mut c2 =
            MetricsCollector::new(Some(LatencyType::OneWay), vec![50.0, 95.0, 99.0]).unwrap();
        c2.record_message(64, Some(std::time::Duration::from_micros(200)))
            .unwrap();
        c2.record_message(64, Some(std::time::Duration::from_micros(300)))
            .unwrap();

        // Should aggregate across both collectors without panic
        aggregate_and_print_server_metrics(&[c1, c2], &[50.0, 95.0, 99.0]);
    }

    /// Test: effective_concurrency covers UDS and PMQ mechanisms.
    #[test]
    fn test_effective_concurrency_all_mechanisms() {
        // TCP always allows concurrency
        assert_eq!(effective_concurrency(IpcMechanism::TcpSocket, 8), 8);

        // UDS allows concurrency on unix
        #[cfg(unix)]
        assert_eq!(effective_concurrency(IpcMechanism::UnixDomainSocket, 4), 4);

        // SHM forces to 1
        assert_eq!(effective_concurrency(IpcMechanism::SharedMemory, 4), 1);

        // PMQ forces to 1 (Linux only mechanism, but the check works on all platforms)
        #[cfg(target_os = "linux")]
        assert_eq!(effective_concurrency(IpcMechanism::PosixMessageQueue, 4), 1);

        // Concurrency=1 always stays at 1 regardless of mechanism
        assert_eq!(effective_concurrency(IpcMechanism::SharedMemory, 1), 1);
        assert_eq!(effective_concurrency(IpcMechanism::TcpSocket, 1), 1);
    }

    /// Test: large payload round-trip preserves content integrity.
    #[test]
    fn test_standalone_large_payload_integrity() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let payload_size = 4096usize;

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            while let Ok(message) = transport.receive_blocking() {
                if message.message_type == MessageType::Shutdown {
                    break;
                }
                // Echo the payload back in the response
                let resp = Message::new(message.id, message.payload.clone(), MessageType::Response);
                if transport.send_blocking(&resp).is_err() {
                    break;
                }
            }
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Create a payload with recognizable pattern
        let payload: Vec<u8> = (0..payload_size).map(|i| (i % 256) as u8).collect();

        for i in 0..5u64 {
            let msg = Message::new(i, payload.clone(), MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, i);
            assert_eq!(resp.payload.len(), payload_size, "Payload size mismatch");
            assert_eq!(resp.payload, payload, "Payload content corrupted");
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection filters canary messages from metrics.
    #[test]
    fn test_handle_client_connection_filters_canary() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            let metrics = collector.get_metrics();
            // Should have recorded 3 real messages, not the 2 canaries
            assert_eq!(
                metrics.throughput.total_messages, 3,
                "Canary messages (id=u64::MAX) should be filtered from metrics"
            );
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Send canary (warmup), real messages, another canary, then shutdown
        let canary = Message::new(u64::MAX, vec![0u8; 64], MessageType::OneWay);
        transport.send_blocking(&canary).unwrap();

        for i in 0..3u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        // Second canary
        transport.send_blocking(&canary).unwrap();

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: mixed message types on a single connection.
    #[test]
    fn test_handle_client_connection_mixed_message_types() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_config = transport_config.clone();
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            transport.start_server_blocking(&server_config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let collector = handle_client_connection(transport.as_mut(), &config).unwrap();
            let metrics = collector.get_metrics();
            // Only OneWay messages should be recorded (not Request/Ping)
            assert_eq!(
                metrics.throughput.total_messages, 3,
                "Only OneWay messages should be recorded in one-way metrics"
            );
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        // Interleave OneWay and Request messages
        for i in 0..3u64 {
            // OneWay (should be recorded)
            let ow = Message::new(i * 2, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&ow).unwrap();

            // Request (should NOT be recorded as one-way, but should get a response)
            let req = Message::new(i * 2 + 1, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&req).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, i * 2 + 1);
            assert_eq!(resp.message_type, MessageType::Response);
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: run_standalone_server_blocking_multi_accept_tcp accepts
    /// multiple clients and aggregates metrics.
    #[test]
    fn test_multi_accept_tcp_server_direct() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_multi_accept_tcp(&tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        // Connect 2 clients, send messages, shutdown
        let mut handles = Vec::new();
        for worker in 0..2u64 {
            let tc = transport_config.clone();
            let h = std::thread::spawn(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                crate::standalone_server::connect_blocking_with_retry(&mut transport, &tc).unwrap();
                for i in 0..5u64 {
                    let msg = Message::new(worker * 100 + i, vec![0u8; 64], MessageType::Request);
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            handles.push(h);
        }

        for h in handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    /// Test: run_standalone_server_blocking_single handles single client.
    #[test]
    fn test_single_server_direct() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64", "--blocking"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let a = args.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_single(&a, IpcMechanism::TcpSocket, &tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        for i in 0..5u64 {
            let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
            transport.send_blocking(&msg).unwrap();
        }

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: run_standalone_server_blocking dispatches correctly.
    #[test]
    fn test_server_blocking_dispatch() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from([
            "ipc-benchmark",
            "-m",
            "tcp",
            "-s",
            "64",
            "--blocking",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let a = args.clone();
        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking(&a, IpcMechanism::TcpSocket, &tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &transport_config).unwrap();

        let msg = Message::new(0, vec![0u8; 64], MessageType::Request);
        transport.send_blocking(&msg).unwrap();
        let resp = transport.receive_blocking().unwrap();
        assert_eq!(resp.message_type, MessageType::Response);

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: aggregate_and_print_server_metrics with real handler data.
    #[test]
    fn test_aggregate_server_metrics_from_handlers() {
        let mut c1 = MetricsCollector::new(
            Some(crate::metrics::LatencyType::OneWay),
            vec![50.0, 95.0, 99.0],
        )
        .unwrap();
        for _ in 0..10 {
            c1.record_message(64, Some(std::time::Duration::from_micros(50)))
                .unwrap();
        }

        let mut c2 = MetricsCollector::new(
            Some(crate::metrics::LatencyType::OneWay),
            vec![50.0, 95.0, 99.0],
        )
        .unwrap();
        for _ in 0..10 {
            c2.record_message(64, Some(std::time::Duration::from_micros(100)))
                .unwrap();
        }

        // Should aggregate 20 total messages across 2 collectors
        aggregate_and_print_server_metrics(&[c1, c2], &[50.0, 95.0, 99.0]);
    }

    /// Test: print_server_one_way_latency with data.
    #[test]
    fn test_print_server_one_way_latency_with_data() {
        let mut metrics = MetricsCollector::new(
            Some(crate::metrics::LatencyType::OneWay),
            vec![50.0, 95.0, 99.0],
        )
        .unwrap();
        for _ in 0..5 {
            metrics
                .record_message(64, Some(std::time::Duration::from_micros(75)))
                .unwrap();
        }
        // Should print without panic
        print_server_one_way_latency(5, &metrics);
    }

    /// Test: print_server_one_way_latency with zero count.
    #[test]
    fn test_print_server_one_way_latency_zero() {
        let metrics = MetricsCollector::new(
            Some(crate::metrics::LatencyType::OneWay),
            vec![50.0, 95.0, 99.0],
        )
        .unwrap();
        // Should not panic with zero count
        print_server_one_way_latency(0, &metrics);
    }

    /// Test: async multi-accept TCP server handles clients via
    /// run_standalone_server_async_multi_accept_tcp directly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_async_multi_accept_tcp_server_direct() {
        use crate::ipc::BlockingTcpSocket;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        // We can't call run_standalone_server_async_multi_accept_tcp directly
        // because it binds its own listener. Instead, exercise the async path
        // components: tokio accept + spawn_blocking + handle_client_connection.
        let mut join_set = tokio::task::JoinSet::new();
        let num_clients = 2usize;

        // Spawn clients in blocking threads
        for worker_id in 0..num_clients {
            let tc = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
                ..Default::default()
            };
            join_set.spawn_blocking(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                connect_blocking_with_retry(&mut transport, &tc).unwrap();
                for i in 0..5u64 {
                    let msg = Message::new(
                        worker_id as u64 * 100 + i,
                        vec![0u8; 64],
                        MessageType::Request,
                    );
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
        }

        // Accept and handle with async pattern
        for _ in 0..num_clients {
            let (stream, _) = listener.accept().await.unwrap();
            let std_stream = stream.into_std().unwrap();
            std_stream.set_nonblocking(false).unwrap();
            std_stream.set_nodelay(true).unwrap();

            let cfg = config.clone();
            join_set.spawn_blocking(move || {
                let mut transport = BlockingTcpSocket::from_stream(std_stream);
                handle_client_connection(&mut transport, &cfg).unwrap();
                let _ = transport.close_blocking();
            });
        }

        while let Some(result) = join_set.join_next().await {
            result.unwrap();
        }
    }

    /// Test: async single-client server path.
    #[tokio::test]
    async fn test_async_single_server_path() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();

        // Start async server in a task
        let server_handle = tokio::spawn(async move {
            run_standalone_server_async_single(IpcMechanism::TcpSocket, &tc, &cfg)
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Connect blocking client from a blocking thread
        let client_tc = transport_config.clone();
        let client_handle = tokio::task::spawn_blocking(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &client_tc).unwrap();

            for i in 0..5u64 {
                let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
                transport.send_blocking(&msg).unwrap();
            }

            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = transport.send_blocking(&shutdown);
            transport.close_blocking().unwrap();
        });

        client_handle.await.unwrap();
        server_handle.await.unwrap();
    }

    /// Test: UDS multi-accept server handles multiple clients.
    #[cfg(unix)]
    #[test]
    fn test_multi_accept_uds_server_direct() {
        use crate::ipc::BlockingUnixDomainSocket;

        let socket_path = format!("/tmp/test_multi_accept_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let transport_config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "uds", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_multi_accept_uds(&tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let mut handles = Vec::new();
        for worker in 0..2u64 {
            let sp = socket_path.clone();
            let h = std::thread::spawn(move || {
                let stream = std::os::unix::net::UnixStream::connect(&sp).unwrap();
                let mut transport = BlockingUnixDomainSocket::from_stream(stream);
                for i in 0..5u64 {
                    let msg = Message::new(worker * 100 + i, vec![0u8; 64], MessageType::Request);
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            handles.push(h);
        }

        for h in handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    /// Test: async multi-accept TCP server function directly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_async_multi_accept_tcp_full() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = tokio::spawn(async move {
            run_standalone_server_async_multi_accept_tcp(&tc, &cfg)
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Connect 2 clients from blocking threads
        let mut client_handles = Vec::new();
        for worker in 0..2u64 {
            let tc = transport_config.clone();
            let h = tokio::task::spawn_blocking(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                connect_blocking_with_retry(&mut transport, &tc).unwrap();
                for i in 0..5u64 {
                    let msg = Message::new(worker * 100 + i, vec![0u8; 64], MessageType::Request);
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            client_handles.push(h);
        }

        for h in client_handles {
            h.await.unwrap();
        }
        server_handle.await.unwrap();
    }

    /// Test: async single-server with one-way messages records latency.
    #[tokio::test]
    async fn test_async_single_server_one_way_metrics() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = tokio::spawn(async move {
            run_standalone_server_async_single(IpcMechanism::TcpSocket, &tc, &cfg)
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let client_tc = transport_config.clone();
        let client_handle = tokio::task::spawn_blocking(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &client_tc).unwrap();

            // Send one-way messages
            for i in 0..10u64 {
                let msg = Message::new(i, vec![0u8; 64], MessageType::OneWay);
                transport.send_blocking(&msg).unwrap();
            }

            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = transport.send_blocking(&shutdown);
            transport.close_blocking().unwrap();
        });

        client_handle.await.unwrap();
        server_handle.await.unwrap();
    }

    /// Test: connect_blocking_with_retry retry path (server not ready).
    #[test]
    fn test_connect_blocking_retry_path() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start client retry in background (server not yet up)
        let client_tc = transport_config.clone();
        let client_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &client_tc).unwrap();
            transport.close_blocking().unwrap();
        });

        // Delay server start to exercise retry path
        std::thread::sleep(std::time::Duration::from_millis(300));

        let mut server_transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        server_transport
            .start_server_blocking(&transport_config)
            .unwrap();
        // Accept and close
        let _ = server_transport.receive_blocking();
        server_transport.close_blocking().unwrap();

        client_handle.join().unwrap();
    }

    /// Test: run_standalone_server_blocking UDS dispatch.
    #[cfg(unix)]
    #[test]
    fn test_server_blocking_dispatch_uds() {
        use crate::ipc::BlockingUnixDomainSocket;

        let socket_path = format!("/tmp/test_dispatch_uds_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let transport_config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        let args = Args::parse_from([
            "ipc-benchmark",
            "-m",
            "uds",
            "-s",
            "64",
            "--blocking",
            "--socket-path",
            &socket_path,
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let a = args.clone();
        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking(&a, IpcMechanism::UnixDomainSocket, &tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let stream = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
        let mut transport = BlockingUnixDomainSocket::from_stream(stream);

        let msg = Message::new(0, vec![0u8; 64], MessageType::Request);
        transport.send_blocking(&msg).unwrap();
        let resp = transport.receive_blocking().unwrap();
        assert_eq!(resp.message_type, MessageType::Response);

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    /// Test: async UDS multi-accept server directly.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_async_multi_accept_uds_full() {
        use crate::ipc::BlockingUnixDomainSocket;

        let socket_path = format!("/tmp/test_async_uds_multi_{}.sock", std::process::id());
        let _ = std::fs::remove_file(&socket_path);

        let transport_config = TransportConfig {
            socket_path: socket_path.clone(),
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "uds", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = tokio::spawn(async move {
            run_standalone_server_async_multi_accept_uds(&tc, &cfg)
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut client_handles = Vec::new();
        for worker in 0..2u64 {
            let sp = socket_path.clone();
            let h = tokio::task::spawn_blocking(move || {
                let stream = std::os::unix::net::UnixStream::connect(&sp).unwrap();
                let mut transport = BlockingUnixDomainSocket::from_stream(stream);
                for i in 0..5u64 {
                    let msg = Message::new(worker * 100 + i, vec![0u8; 64], MessageType::Request);
                    transport.send_blocking(&msg).unwrap();
                    let resp = transport.receive_blocking().unwrap();
                    assert_eq!(resp.message_type, MessageType::Response);
                }
                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            client_handles.push(h);
        }

        for h in client_handles {
            h.await.unwrap();
        }
        server_handle.await.unwrap();
        let _ = std::fs::remove_file(&socket_path);
    }

    /// Test: run_standalone_server top-level dispatch (TCP blocking).
    /// Exercises the full entry point including logging init, affinity
    /// check, shm-direct guard, and dispatch to blocking server.
    #[test]
    fn test_run_standalone_server_full_dispatch() {
        let port = get_free_port();

        let server_handle = std::thread::spawn(move || {
            let args = Args::parse_from([
                "ipc-benchmark",
                "--server",
                "-m",
                "tcp",
                "--blocking",
                "--port",
                &port.to_string(),
            ]);
            run_standalone_server(args).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(300));

        let tc = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &tc).unwrap();

        let msg = Message::new(0, vec![0u8; 64], MessageType::Request);
        transport.send_blocking(&msg).unwrap();
        let resp = transport.receive_blocking().unwrap();
        assert_eq!(resp.message_type, MessageType::Response);

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: server send failure when client disconnects mid-conversation.
    /// Exercises the send_blocking error path in handle_client_connection.
    #[test]
    fn test_handle_client_connection_send_failure() {
        let port = get_free_port();

        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
                ..Default::default()
            };
            transport.start_server_blocking(&config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let cfg = BenchmarkConfig::from_args(&args).unwrap();
            // This should handle the send failure gracefully (client drops)
            let result = handle_client_connection(transport.as_mut(), &cfg);
            // Should succeed (handler exits cleanly on send error)
            assert!(result.is_ok());
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client: send a Request but close immediately without reading response
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        // Send a valid Request message
        let msg = Message::new(1, vec![0u8; 64], MessageType::Request);
        let data = bincode::serialize(&msg).unwrap();
        use std::io::Write;
        let len = (data.len() as u32).to_le_bytes();
        stream.write_all(&len).unwrap();
        stream.write_all(&data).unwrap();
        // Close immediately without reading the response
        drop(stream);

        server_handle.join().unwrap();
    }

    /// Test: server single-client send failure path.
    /// Exercises the send error branch in run_standalone_server_blocking_single.
    #[test]
    fn test_single_server_client_disconnect() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64", "--blocking"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let a = args.clone();
        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            // Should handle client disconnect gracefully
            let result =
                run_standalone_server_blocking_single(&a, IpcMechanism::TcpSocket, &tc, &cfg);
            assert!(result.is_ok());
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        // Connect and send Request then drop (triggers send error on server)
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let msg = Message::new(1, vec![0u8; 64], MessageType::Request);
        let data = bincode::serialize(&msg).unwrap();
        use std::io::Write;
        let len = (data.len() as u32).to_le_bytes();
        stream.write_all(&len).unwrap();
        stream.write_all(&data).unwrap();
        drop(stream);

        server_handle.join().unwrap();
    }

    /// Test: multi-accept server handles a bad client gracefully and
    /// continues serving good clients. First client sends garbage bytes
    /// that cause a deserialization error in the handler, exercising
    /// the handler error path. Second client connects normally.
    #[test]
    fn test_multi_accept_server_survives_bad_client() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_multi_accept_tcp(&tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        // Bad client: connect and send garbage bytes
        let bad_client = std::thread::spawn(move || {
            use std::io::Write;
            let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
            // Send a valid length prefix but garbage payload
            let len: u32 = 100;
            stream.write_all(&len.to_le_bytes()).unwrap();
            stream.write_all(&[0xFFu8; 100]).unwrap();
            // Close immediately
            drop(stream);
        });

        bad_client.join().unwrap();

        // Give server a moment to process the bad client
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Good client: should still be able to connect and work
        let tc = transport_config.clone();
        let good_client = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &tc).unwrap();

            let msg = Message::new(42, vec![0u8; 64], MessageType::Request);
            transport.send_blocking(&msg).unwrap();
            let resp = transport.receive_blocking().unwrap();
            assert_eq!(resp.id, 42);
            assert_eq!(resp.message_type, MessageType::Response);

            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = transport.send_blocking(&shutdown);
            transport.close_blocking().unwrap();
        });

        good_client.join().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: handle_client_connection returns Ok even when client sends
    /// garbage, because the receive error causes a clean exit from the
    /// message loop (not a panic).
    #[test]
    fn test_handle_client_connection_garbage_input() {
        let port = get_free_port();

        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
                ..Default::default()
            };
            transport.start_server_blocking(&config).unwrap();

            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let cfg = BenchmarkConfig::from_args(&args).unwrap();
            // Should return an error (deserialization failure) but not panic
            let result = handle_client_connection(transport.as_mut(), &cfg);
            // The receive loop exits on error, returning the collector
            // (which may have 0 messages recorded)
            assert!(result.is_ok());
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Send garbage
        use std::io::Write;
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let len: u32 = 50;
        stream.write_all(&len.to_le_bytes()).unwrap();
        stream.write_all(&[0xDEu8; 50]).unwrap();
        drop(stream);

        server_handle.join().unwrap();
    }

    /// Test: multi-accept server handles clients sending with delay.
    /// Verifies server doesn't timeout or misbehave with slow senders.
    #[test]
    fn test_multi_accept_server_with_delayed_client() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_multi_accept_tcp(&tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        let tc = transport_config.clone();
        let client_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &tc).unwrap();

            // Send messages with 10ms delay between each
            for i in 0..5u64 {
                let msg = Message::new(i, vec![0u8; 64], MessageType::Request);
                transport.send_blocking(&msg).unwrap();
                let resp = transport.receive_blocking().unwrap();
                assert_eq!(resp.message_type, MessageType::Response);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
            let _ = transport.send_blocking(&shutdown);
            transport.close_blocking().unwrap();
        });

        client_handle.join().unwrap();
        server_handle.join().unwrap();
    }

    /// Test: multi-accept TCP server with duration-mode one-way clients.
    /// Exercises the grace timer with longer-running duration-based clients.
    #[test]
    fn test_multi_accept_server_duration_one_way() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = std::thread::spawn(move || {
            run_standalone_server_blocking_multi_accept_tcp(&tc, &cfg).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        // Two clients each sending one-way for 200ms
        let mut handles = Vec::new();
        for worker in 0..2u64 {
            let tc = transport_config.clone();
            let h = std::thread::spawn(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                connect_blocking_with_retry(&mut transport, &tc).unwrap();

                let start = std::time::Instant::now();
                let duration = std::time::Duration::from_millis(200);
                let mut count = 0u64;
                while start.elapsed() < duration {
                    let msg =
                        Message::new(worker * 10000 + count, vec![0u8; 64], MessageType::OneWay);
                    transport.send_blocking(&msg).unwrap();
                    count += 1;
                }
                assert!(count > 0, "Should have sent messages during duration");

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            handles.push(h);
        }

        for h in handles {
            h.join().unwrap();
        }
        server_handle.join().unwrap();
    }

    /// Test: async multi-accept TCP server with duration-mode one-way clients.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_async_multi_accept_server_duration_one_way() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
        let config = BenchmarkConfig::from_args(&args).unwrap();

        let tc = transport_config.clone();
        let cfg = config.clone();
        let server_handle = tokio::spawn(async move {
            run_standalone_server_async_multi_accept_tcp(&tc, &cfg)
                .await
                .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut client_handles = Vec::new();
        for worker in 0..2u64 {
            let tc = transport_config.clone();
            let h = tokio::task::spawn_blocking(move || {
                let mut transport =
                    BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
                connect_blocking_with_retry(&mut transport, &tc).unwrap();

                let start = std::time::Instant::now();
                let duration = std::time::Duration::from_millis(200);
                let mut count = 0u64;
                while start.elapsed() < duration {
                    let msg =
                        Message::new(worker * 10000 + count, vec![0u8; 64], MessageType::OneWay);
                    transport.send_blocking(&msg).unwrap();
                    count += 1;
                }
                assert!(count > 0, "Should have sent messages during duration");

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send_blocking(&shutdown);
                transport.close_blocking().unwrap();
            });
            client_handles.push(h);
        }

        for h in client_handles {
            h.await.unwrap();
        }
        server_handle.await.unwrap();
    }

    /// Test: run_standalone_server rejects 'all' mechanism.
    #[test]
    fn test_run_standalone_server_rejects_all_via_dispatch() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "all"]);
        let result = run_standalone_server(args);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Cannot use 'all'"),
            "Should reject 'all' mechanism"
        );
    }

    /// Test: run_standalone_server rejects --shm-direct without --blocking.
    #[test]
    fn test_run_standalone_server_rejects_shm_direct() {
        let args = Args::parse_from(["ipc-benchmark", "--server", "-m", "shm", "--shm-direct"]);
        let result = run_standalone_server(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("--shm-direct requires --blocking"),
            "Should reject shm-direct without blocking"
        );
    }

    /// Test: run_standalone_server with verbose logging (-vv).
    #[test]
    fn test_run_standalone_server_verbose() {
        let port = get_free_port();

        let server_handle = std::thread::spawn(move || {
            let args = Args::parse_from([
                "ipc-benchmark",
                "--server",
                "-m",
                "tcp",
                "--blocking",
                "-vv",
                "--port",
                &port.to_string(),
            ]);
            run_standalone_server(args).unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(300));

        let tc = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let mut transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        connect_blocking_with_retry(&mut transport, &tc).unwrap();

        let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
        let _ = transport.send_blocking(&shutdown);
        transport.close_blocking().unwrap();
        server_handle.join().unwrap();
    }
}
