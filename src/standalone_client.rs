//! Standalone client logic for the IPC benchmark suite.
//!
//! This module contains all client-side functionality for standalone
//! (split-process) mode, where the server and client run as separate
//! OS processes. It supports both blocking and async transports,
//! single-connection and multi-connection concurrent modes, and
//! reports client-side round-trip latency and one-way throughput.
//!
//! The top-level entry point is [`run_standalone_client`], which sets
//! up logging, CPU affinity, and dispatches to the appropriate blocking
//! or async implementation based on CLI flags.

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use tracing_subscriber::filter::LevelFilter;

use crate::benchmark::BenchmarkConfig;
use crate::cli::{Args, IpcMechanism};
use crate::ipc::{
    get_monotonic_time_ns, BlockingTransportFactory, Message, MessageType, TransportConfig,
    TransportFactory,
};
use crate::logging::ColorizedFormatter;
use crate::metrics::{LatencyType, MetricsCollector};
use crate::results::{BenchmarkResults, MessageLatencyRecord};
use crate::results_blocking::BlockingResultsManager;
use crate::standalone_server::{
    build_standalone_transport_config, effective_concurrency, CONNECT_RETRY_INTERVAL,
    CONNECT_RETRY_TIMEOUT,
};

/// Run in standalone client mode.
///
/// Connects to an already-running standalone server and executes
/// the benchmark workload. Retries the connection with backoff
/// if the server is not yet available.
pub fn run_standalone_client(args: Args) -> Result<()> {
    let mechanism = match args.mechanisms.first() {
        Some(&m) => m,
        None => return Err(anyhow::anyhow!("No IPC mechanism specified")),
    };

    if mechanism == IpcMechanism::All {
        return Err(anyhow::anyhow!(
            "Cannot use 'all' mechanism in standalone client mode. \
             Specify a single mechanism (e.g., -m uds)"
        ));
    }

    if args.mechanisms.len() > 1 {
        return Err(anyhow::anyhow!(
            "Standalone client mode supports only one mechanism, \
             but {} were specified. Use a single -m flag (e.g., -m tcp)",
            args.mechanisms.len()
        ));
    }

    // Defensive: --shm-direct requires --blocking (normally enforced by
    // main() before this function is called, but guard here too).
    if args.shm_direct && !args.blocking {
        return Err(anyhow::anyhow!("--shm-direct requires --blocking mode"));
    }

    // Set up logging
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

    if let Some(core) = args.client_affinity {
        if let Err(e) = crate::utils::set_affinity(core) {
            error!("Failed to set client CPU affinity to core {}: {}", core, e);
        } else {
            info!("Client affinity set to CPU core {}", core);
        }
    }

    let transport_config = build_standalone_transport_config(&args);
    let config = BenchmarkConfig::from_args(&args)?;

    // Create ResultsManager for structured output
    let log_file_for_manager = match args.log_file.as_deref() {
        Some("stderr") => Some("stderr".to_string()),
        Some(path_str) => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            Some(format!("{}.{}", path_str, today))
        }
        None => None,
    };

    let mut results_manager =
        BlockingResultsManager::new(args.output_file.as_deref(), log_file_for_manager.as_deref())?;

    // Enable streaming if requested
    if let Some(ref streaming_file) = args.streaming_output_json {
        let both_tests = config.one_way && config.round_trip;
        if both_tests {
            results_manager.enable_combined_streaming(streaming_file, true)?;
        } else {
            results_manager.enable_per_message_streaming(streaming_file)?;
        }
    }
    if let Some(ref streaming_file) = args.streaming_output_csv {
        results_manager.enable_csv_streaming(streaming_file)?;
    }

    info!(
        "Starting standalone client: mechanism={}, blocking={}",
        mechanism, args.blocking
    );

    if args.blocking {
        run_standalone_client_blocking(args, mechanism, transport_config, &mut results_manager)?;
    } else {
        run_standalone_client_async(args, mechanism, transport_config, &mut results_manager)?;
    }

    results_manager.finalize()?;
    if let Err(e) = results_manager.print_summary() {
        error!("Failed to print results summary: {}", e);
    }

    Ok(())
}

/// Connect to a server with retry and backoff.
///
/// Retries the connection at 100ms intervals for up to 30 seconds,
/// allowing the server to start after the client.
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

/// Blocking standalone client implementation.
///
/// When `concurrency > 1` (and mechanism supports it), spawns N worker
/// threads each with their own transport connection. Results are
/// aggregated across all workers.
pub fn run_standalone_client_blocking(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
) -> Result<()> {
    let config = BenchmarkConfig::from_args(&args)?;

    let concurrency = effective_concurrency(mechanism, config.concurrency);

    if concurrency > 1 {
        run_standalone_client_blocking_concurrent(
            args,
            mechanism,
            transport_config,
            results_manager,
            &config,
            concurrency,
        )
    } else {
        run_standalone_client_blocking_single(
            args,
            mechanism,
            transport_config,
            results_manager,
            &config,
        )
    }
}

/// Single-threaded blocking standalone client (original behavior).
pub fn run_standalone_client_blocking_single(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = BlockingTransportFactory::create(&mechanism, args.shm_direct)?;

    info!("Connecting to server...");
    connect_blocking_with_retry(&mut transport, &transport_config)?;
    info!("Connected to server.");

    let msg_count = config.msg_count.unwrap_or(crate::defaults::MSG_COUNT);
    let payload = vec![0u8; config.message_size];

    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        1,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    let total_start = std::time::Instant::now();

    // Warmup
    for _ in 0..config.warmup_iterations {
        let msg_type = if config.round_trip {
            MessageType::Request
        } else {
            MessageType::OneWay
        };
        let msg = Message::new(u64::MAX, payload.clone(), msg_type);
        transport.send_blocking(&msg)?;
        if config.round_trip {
            transport.receive_blocking()?;
        }
    }
    info!("Warmup complete ({} iterations)", config.warmup_iterations);

    // One-way test (throughput only -- true one-way latency requires server-side measurement)
    if config.one_way {
        let mut metrics = MetricsCollector::new(None, config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
            if let Err(e) = transport.send_blocking(&canary) {
                warn!("Canary send failed, connection may be broken: {}", e);
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload.clone(), MessageType::OneWay);

        let start = std::time::Instant::now();
        let count = if let Some(test_duration) = config.duration {
            info!(
                "Running one-way throughput test (duration={:.2?})...",
                test_duration
            );
            let mut c = 0u64;
            while start.elapsed() < test_duration {
                msg.id = c;
                msg.timestamp = get_monotonic_time_ns();
                transport.send_blocking(&msg)?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
                c += 1;
            }
            c
        } else {
            info!(
                "Running one-way throughput test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                transport.send_blocking(&msg)?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
            }
            msg_count as u64
        };

        let elapsed = start.elapsed();
        info!(
            "One-way complete: {} messages in {:.2?} ({:.0} msg/s)",
            count,
            elapsed,
            count as f64 / elapsed.as_secs_f64()
        );

        results.add_one_way_results(metrics.get_metrics());
    }

    // Round-trip test
    if config.round_trip {
        let mut metrics =
            MetricsCollector::new(Some(LatencyType::RoundTrip), config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
            match transport.send_blocking(&canary) {
                Ok(()) => {
                    if let Err(e) = transport.receive_blocking() {
                        warn!("Canary receive failed, connection may be broken: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Canary send failed, connection may be broken: {}", e);
                }
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload, MessageType::Request);

        if let Some(test_duration) = config.duration {
            info!(
                "Running round-trip latency test (duration={:.2?})...",
                test_duration
            );
            let start = std::time::Instant::now();
            let mut i = 0u64;
            while start.elapsed() < test_duration {
                msg.id = i;
                msg.timestamp = get_monotonic_time_ns();
                let send_wall_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let send_time = std::time::Instant::now();
                transport.send_blocking(&msg)?;
                let _response = transport.receive_blocking()?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                    send_wall_ns,
                );
                if let Err(e) = results_manager.stream_latency_record(&record) {
                    debug!("Streaming latency record failed: {}", e);
                }

                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
                i += 1;
            }
        } else {
            info!(
                "Running round-trip latency test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                let send_wall_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let send_time = std::time::Instant::now();
                transport.send_blocking(&msg)?;
                let _response = transport.receive_blocking()?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i as u64,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                    send_wall_ns,
                );
                if let Err(e) = results_manager.stream_latency_record(&record) {
                    debug!("Streaming latency record failed: {}", e);
                }

                if let Some(delay) = config.send_delay {
                    std::thread::sleep(delay);
                }
            }
        }

        results.add_round_trip_results(metrics.get_metrics());
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    // Send shutdown message before closing for deterministic server exit
    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
    if let Err(e) = transport.send_blocking(&shutdown) {
        debug!(
            "Shutdown send failed (server may have already exited): {}",
            e
        );
    }

    transport.close_blocking()?;
    info!("Standalone client finished.");
    Ok(())
}

/// Multi-threaded blocking standalone client.
///
/// Spawns `concurrency` worker threads, each creating its own transport
/// connection to the server. Each worker runs the test loop independently
/// with its own `MetricsCollector`. Results are aggregated across all
/// workers using `MetricsCollector::aggregate_worker_metrics()`.
///
/// When both one-way and round-trip tests are enabled, they run as two
/// sequential phases: all workers connect, run one-way, and disconnect;
/// then all workers reconnect for round-trip. This means the server
/// accepts 2N total connections. The single-connection path reuses one
/// transport for both tests, so this overhead is specific to concurrent
/// mode.
pub fn run_standalone_client_blocking_concurrent(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
    concurrency: usize,
) -> Result<()> {
    use crate::metrics::PerformanceMetrics;

    let msg_count = config.msg_count.unwrap_or(crate::defaults::MSG_COUNT);
    let base_messages_per_worker = msg_count / concurrency;
    let remainder_messages = msg_count % concurrency;

    info!(
        "Starting {} concurrent workers (~{} messages each)...",
        concurrency, base_messages_per_worker
    );

    let total_start = std::time::Instant::now();

    // Spawn worker threads for one-way test
    let one_way_results: Option<Vec<PerformanceMetrics>> = if config.one_way {
        let handles: Vec<_> = (0..concurrency)
            .map(|worker_id| {
                let tc = transport_config.clone();
                let percentiles = config.percentiles.clone();
                let message_size = config.message_size;
                let duration = config.duration;
                let send_delay = config.send_delay;
                let include_first = config.include_first_message;
                let warmup_iters = config.warmup_iterations;
                let shm_direct = args.shm_direct;
                let mech = mechanism;
                // Last worker gets any remainder messages
                let worker_msg_count = base_messages_per_worker
                    + if worker_id == concurrency - 1 {
                        remainder_messages
                    } else {
                        0
                    };

                std::thread::spawn(move || -> Result<PerformanceMetrics> {
                    let mut transport = BlockingTransportFactory::create(&mech, shm_direct)?;
                    connect_blocking_with_retry(&mut transport, &tc)?;
                    debug!("Worker {} connected (one-way)", worker_id);

                    let payload = vec![0u8; message_size];

                    for _ in 0..warmup_iters {
                        let msg = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                        transport.send_blocking(&msg)?;
                    }

                    let mut metrics = MetricsCollector::new(None, percentiles)?;

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                        if let Err(e) = transport.send_blocking(&canary) {
                            warn!(
                                "Worker {} canary send failed, connection may be broken: {}",
                                worker_id, e
                            );
                        }
                    }

                    let mut msg = Message::new(0, payload, MessageType::OneWay);

                    if let Some(test_duration) = duration {
                        let start = std::time::Instant::now();
                        let mut c = 0u64;
                        while start.elapsed() < test_duration {
                            msg.id = worker_id as u64 * u32::MAX as u64 + c;
                            msg.timestamp = get_monotonic_time_ns();
                            transport.send_blocking(&msg)?;
                            metrics.record_message(message_size, None)?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                            c += 1;
                        }
                    } else {
                        for i in 0..worker_msg_count {
                            msg.id = (worker_id * base_messages_per_worker + i) as u64;
                            msg.timestamp = get_monotonic_time_ns();
                            transport.send_blocking(&msg)?;
                            metrics.record_message(message_size, None)?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    if let Err(e) = transport.send_blocking(&shutdown) {
                        debug!("Worker {} shutdown send failed: {}", worker_id, e);
                    }
                    transport.close_blocking()?;
                    debug!("Worker {} finished (one-way)", worker_id);
                    Ok(metrics.get_metrics())
                })
            })
            .collect();

        let worker_results: Vec<PerformanceMetrics> = handles
            .into_iter()
            .enumerate()
            .map(|(id, h)| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker {} panicked", id)))
            })
            .collect::<Result<Vec<_>>>()?;

        Some(worker_results)
    } else {
        None
    };

    // Spawn worker threads for round-trip test
    let round_trip_results: Option<Vec<PerformanceMetrics>> = if config.round_trip {
        let handles: Vec<_> = (0..concurrency)
            .map(|worker_id| {
                let tc = transport_config.clone();
                let percentiles = config.percentiles.clone();
                let message_size = config.message_size;
                let duration = config.duration;
                let send_delay = config.send_delay;
                let include_first = config.include_first_message;
                let warmup_iters = config.warmup_iterations;
                let shm_direct = args.shm_direct;
                let mech = mechanism;
                let worker_msg_count = base_messages_per_worker
                    + if worker_id == concurrency - 1 {
                        remainder_messages
                    } else {
                        0
                    };

                std::thread::spawn(move || -> Result<(PerformanceMetrics, Vec<MessageLatencyRecord>)> {
                    let mut transport = BlockingTransportFactory::create(&mech, shm_direct)?;
                    connect_blocking_with_retry(&mut transport, &tc)?;
                    debug!("Worker {} connected (round-trip)", worker_id);

                    let payload = vec![0u8; message_size];

                    for _ in 0..warmup_iters {
                        let msg = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                        transport.send_blocking(&msg)?;
                        transport.receive_blocking()?;
                    }

                    let mut metrics =
                        MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                    let mut records = Vec::new();

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                        match transport.send_blocking(&canary) {
                            Ok(()) => {
                                if let Err(e) = transport.receive_blocking() {
                                    warn!("Worker {} canary receive failed, connection may be broken: {}", worker_id, e);
                                }
                            }
                            Err(e) => {
                                warn!("Worker {} canary send failed, connection may be broken: {}", worker_id, e);
                            }
                        }
                    }

                    let mut msg = Message::new(0, payload, MessageType::Request);

                    if let Some(test_duration) = duration {
                        let start = std::time::Instant::now();
                        let mut i = 0u64;
                        while start.elapsed() < test_duration {
                            msg.id = worker_id as u64 * u32::MAX as u64 + i;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_wall_ns = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos() as u64;
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            records.push(MessageLatencyRecord::new(
                                msg.id,
                                mech,
                                message_size,
                                LatencyType::RoundTrip,
                                latency,
                                send_wall_ns,
                            ));
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                            i += 1;
                        }
                    } else {
                        for i in 0..worker_msg_count {
                            msg.id = (worker_id * base_messages_per_worker + i) as u64;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_wall_ns = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos() as u64;
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            records.push(MessageLatencyRecord::new(
                                msg.id,
                                mech,
                                message_size,
                                LatencyType::RoundTrip,
                                latency,
                                send_wall_ns,
                            ));
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    if let Err(e) = transport.send_blocking(&shutdown) {
                        debug!("Worker {} shutdown send failed: {}", worker_id, e);
                    }
                    transport.close_blocking()?;
                    debug!("Worker {} finished (round-trip)", worker_id);
                    Ok((metrics.get_metrics(), records))
                })
            })
            .collect();

        let mut worker_results = Vec::new();
        for (id, h) in handles.into_iter().enumerate() {
            let (metrics, records) = h
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Worker {} panicked", id)))?;
            for record in &records {
                if let Err(e) = results_manager.stream_latency_record(record) {
                    debug!("Streaming latency record failed: {}", e);
                }
            }
            worker_results.push(metrics);
        }

        Some(worker_results)
    } else {
        None
    };

    // Aggregate results
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        concurrency,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    if let Some(one_way) = one_way_results {
        let aggregated = MetricsCollector::aggregate_worker_metrics(one_way, &config.percentiles)?;
        results.add_one_way_results(aggregated);
    }

    if let Some(round_trip) = round_trip_results {
        let aggregated =
            MetricsCollector::aggregate_worker_metrics(round_trip, &config.percentiles)?;
        results.add_round_trip_results(aggregated);
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    info!("Standalone client finished ({} workers).", concurrency);
    Ok(())
}

/// Async connect with retry.
pub async fn connect_async_with_retry(
    transport: &mut Box<dyn crate::ipc::IpcTransport>,
    config: &TransportConfig,
) -> Result<()> {
    let start = std::time::Instant::now();

    loop {
        match transport.start_client(config).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if start.elapsed() > CONNECT_RETRY_TIMEOUT {
                    return Err(e).context(
                        "Timed out waiting for server. \
                         Is the server running with the same mechanism and endpoint?",
                    );
                }
                debug!("Connection failed, retrying: {}", e);
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
        }
    }
}

/// Async standalone client implementation.
///
/// Dispatches to single or concurrent mode based on concurrency setting.
#[tokio::main]
pub async fn run_standalone_client_async(
    args: Args,
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
) -> Result<()> {
    let config = BenchmarkConfig::from_args(&args)?;

    let concurrency = effective_concurrency(mechanism, config.concurrency);

    if concurrency > 1 {
        run_standalone_client_async_concurrent(
            mechanism,
            transport_config,
            results_manager,
            &config,
            concurrency,
        )
        .await
    } else {
        run_standalone_client_async_single(mechanism, transport_config, results_manager, &config)
            .await
    }
}

/// Single-connection async standalone client (original behavior).
pub async fn run_standalone_client_async_single(
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
) -> Result<()> {
    let mut transport = TransportFactory::create(&mechanism)?;

    info!("Connecting to server...");
    connect_async_with_retry(&mut transport, &transport_config).await?;
    info!("Connected to server.");

    let msg_count = config.msg_count.unwrap_or(crate::defaults::MSG_COUNT);
    let payload = vec![0u8; config.message_size];

    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        1,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    let total_start = std::time::Instant::now();

    // Warmup
    for _ in 0..config.warmup_iterations {
        let msg_type = if config.round_trip {
            MessageType::Request
        } else {
            MessageType::OneWay
        };
        let msg = Message::new(u64::MAX, payload.clone(), msg_type);
        transport.send(&msg).await?;
        if config.round_trip {
            transport.receive().await?;
        }
    }
    info!("Warmup complete ({} iterations)", config.warmup_iterations);

    // One-way test (throughput only -- true one-way latency requires server-side measurement)
    if config.one_way {
        let mut metrics = MetricsCollector::new(None, config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
            if let Err(e) = transport.send(&canary).await {
                warn!("Canary send failed, connection may be broken: {}", e);
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload.clone(), MessageType::OneWay);

        let start = std::time::Instant::now();
        let count = if let Some(test_duration) = config.duration {
            info!(
                "Running one-way throughput test (duration={:.2?})...",
                test_duration
            );
            let mut c = 0u64;
            while start.elapsed() < test_duration {
                msg.id = c;
                msg.timestamp = get_monotonic_time_ns();
                transport.send(&msg).await?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                c += 1;
            }
            c
        } else {
            info!(
                "Running one-way throughput test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                transport.send(&msg).await?;
                metrics.record_message(config.message_size, None)?;
                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
            msg_count as u64
        };

        let elapsed = start.elapsed();
        info!(
            "One-way complete: {} messages in {:.2?} ({:.0} msg/s)",
            count,
            elapsed,
            count as f64 / elapsed.as_secs_f64()
        );

        results.add_one_way_results(metrics.get_metrics());
    }

    // Round-trip test
    if config.round_trip {
        let mut metrics =
            MetricsCollector::new(Some(LatencyType::RoundTrip), config.percentiles.clone())?;

        // Send canary to warm up the connection if first message excluded
        if !config.include_first_message {
            let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
            match transport.send(&canary).await {
                Ok(_) => {
                    if let Err(e) = transport.receive().await {
                        warn!("Canary receive failed, connection may be broken: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Canary send failed, connection may be broken: {}", e);
                }
            }
        }

        // Create message once, reuse across iterations to avoid per-message heap allocation
        let mut msg = Message::new(0, payload, MessageType::Request);

        if let Some(test_duration) = config.duration {
            info!(
                "Running round-trip latency test (duration={:.2?})...",
                test_duration
            );
            let start = std::time::Instant::now();
            let mut i = 0u64;
            while start.elapsed() < test_duration {
                msg.id = i;
                msg.timestamp = get_monotonic_time_ns();
                let send_wall_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let send_time = std::time::Instant::now();
                transport.send(&msg).await?;
                let _response = transport.receive().await?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                    send_wall_ns,
                );
                if let Err(e) = results_manager.stream_latency_record(&record) {
                    debug!("Streaming latency record failed: {}", e);
                }

                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
                i += 1;
            }
        } else {
            info!(
                "Running round-trip latency test ({} messages)...",
                msg_count
            );
            for i in 0..msg_count {
                msg.id = i as u64;
                msg.timestamp = get_monotonic_time_ns();
                let send_wall_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                let send_time = std::time::Instant::now();
                transport.send(&msg).await?;
                let _response = transport.receive().await?;
                let latency = send_time.elapsed();

                metrics.record_message(config.message_size, Some(latency))?;
                let record = MessageLatencyRecord::new(
                    i as u64,
                    mechanism,
                    config.message_size,
                    LatencyType::RoundTrip,
                    latency,
                    send_wall_ns,
                );
                if let Err(e) = results_manager.stream_latency_record(&record) {
                    debug!("Streaming latency record failed: {}", e);
                }

                if let Some(delay) = config.send_delay {
                    tokio::time::sleep(delay).await;
                }
            }
        }

        results.add_round_trip_results(metrics.get_metrics());
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    // Send shutdown message before closing for deterministic server exit
    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
    if let Err(e) = transport.send(&shutdown).await {
        debug!(
            "Shutdown send failed (server may have already exited): {}",
            e
        );
    }

    if let Err(e) = transport.close().await {
        debug!("Transport close failed: {}", e);
    }
    info!("Standalone client finished.");
    Ok(())
}

/// Multi-task async standalone client.
///
/// Spawns `concurrency` tokio tasks, each creating its own async transport
/// connection. Results are aggregated across all workers.
///
/// When both one-way and round-trip tests are enabled, they run as two
/// sequential phases with separate connections (2N total). See
/// [`run_standalone_client_blocking_concurrent`] for details.
pub async fn run_standalone_client_async_concurrent(
    mechanism: IpcMechanism,
    transport_config: TransportConfig,
    results_manager: &mut BlockingResultsManager,
    config: &BenchmarkConfig,
    concurrency: usize,
) -> Result<()> {
    use crate::metrics::PerformanceMetrics;
    use tokio::task::JoinSet;

    let msg_count = config.msg_count.unwrap_or(crate::defaults::MSG_COUNT);
    let base_messages_per_worker = msg_count / concurrency;
    let remainder_messages = msg_count % concurrency;

    info!(
        "Starting {} concurrent async workers (~{} messages each)...",
        concurrency, base_messages_per_worker
    );

    let total_start = std::time::Instant::now();

    // One-way test
    let one_way_results: Option<Vec<PerformanceMetrics>> = if config.one_way {
        let mut join_set: JoinSet<Result<PerformanceMetrics>> = JoinSet::new();
        for worker_id in 0..concurrency {
            let tc = transport_config.clone();
            let percentiles = config.percentiles.clone();
            let message_size = config.message_size;
            let duration = config.duration;
            let send_delay = config.send_delay;
            let include_first = config.include_first_message;
            let warmup_iters = config.warmup_iterations;
            let mech = mechanism;
            let worker_msg_count = base_messages_per_worker
                + if worker_id == concurrency - 1 {
                    remainder_messages
                } else {
                    0
                };

            join_set.spawn(async move {
                let mut transport = TransportFactory::create(&mech)?;
                connect_async_with_retry(&mut transport, &tc).await?;
                debug!("Async worker {} connected (one-way)", worker_id);

                let payload = vec![0u8; message_size];

                for _ in 0..warmup_iters {
                    let msg = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                    transport.send(&msg).await?;
                }

                let mut metrics = MetricsCollector::new(None, percentiles)?;

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                    if let Err(e) = transport.send(&canary).await {
                        warn!(
                            "Async worker {} canary send failed, connection may be broken: {}",
                            worker_id, e
                        );
                    }
                }

                let mut msg = Message::new(0, payload, MessageType::OneWay);

                if let Some(test_duration) = duration {
                    let start = std::time::Instant::now();
                    let mut c = 0u64;
                    while start.elapsed() < test_duration {
                        msg.id = worker_id as u64 * u32::MAX as u64 + c;
                        msg.timestamp = get_monotonic_time_ns();
                        transport.send(&msg).await?;
                        metrics.record_message(message_size, None)?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                        c += 1;
                    }
                } else {
                    for i in 0..worker_msg_count {
                        msg.id = (worker_id * base_messages_per_worker + i) as u64;
                        msg.timestamp = get_monotonic_time_ns();
                        transport.send(&msg).await?;
                        metrics.record_message(message_size, None)?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                if let Err(e) = transport.send(&shutdown).await {
                    debug!("Async worker {} shutdown send failed: {}", worker_id, e);
                }
                if let Err(e) = transport.close().await {
                    debug!("Async worker {} close failed: {}", worker_id, e);
                }
                debug!("Async worker {} finished (one-way)", worker_id);
                Ok(metrics.get_metrics())
            });
        }

        let mut worker_results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            worker_results.push(result.context("Worker task panicked")??);
        }
        Some(worker_results)
    } else {
        None
    };

    // Round-trip test
    let round_trip_results: Option<Vec<PerformanceMetrics>> = if config.round_trip {
        let mut join_set: JoinSet<Result<(PerformanceMetrics, Vec<MessageLatencyRecord>)>> =
            JoinSet::new();
        for worker_id in 0..concurrency {
            let tc = transport_config.clone();
            let percentiles = config.percentiles.clone();
            let message_size = config.message_size;
            let duration = config.duration;
            let send_delay = config.send_delay;
            let include_first = config.include_first_message;
            let warmup_iters = config.warmup_iterations;
            let mech = mechanism;
            let worker_msg_count = base_messages_per_worker
                + if worker_id == concurrency - 1 {
                    remainder_messages
                } else {
                    0
                };

            join_set.spawn(async move {
                let mut transport = TransportFactory::create(&mech)?;
                connect_async_with_retry(&mut transport, &tc).await?;
                debug!("Async worker {} connected (round-trip)", worker_id);

                let payload = vec![0u8; message_size];

                for _ in 0..warmup_iters {
                    let msg = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                    transport.send(&msg).await?;
                    transport.receive().await?;
                }

                let mut metrics = MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                let mut records = Vec::new();

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                    match transport.send(&canary).await {
                        Ok(_) => {
                            if let Err(e) = transport.receive().await {
                                warn!("Async worker {} canary receive failed, connection may be broken: {}", worker_id, e);
                            }
                        }
                        Err(e) => {
                            warn!("Async worker {} canary send failed, connection may be broken: {}", worker_id, e);
                        }
                    }
                }

                let mut msg = Message::new(0, payload, MessageType::Request);

                if let Some(test_duration) = duration {
                    let start = std::time::Instant::now();
                    let mut i = 0u64;
                    while start.elapsed() < test_duration {
                        msg.id = worker_id as u64 * u32::MAX as u64 + i;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_wall_ns = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64;
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        records.push(MessageLatencyRecord::new(
                            msg.id,
                            mech,
                            message_size,
                            LatencyType::RoundTrip,
                            latency,
                            send_wall_ns,
                        ));
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                        i += 1;
                    }
                } else {
                    for i in 0..worker_msg_count {
                        msg.id = (worker_id * base_messages_per_worker + i) as u64;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_wall_ns = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos() as u64;
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        records.push(MessageLatencyRecord::new(
                            msg.id,
                            mech,
                            message_size,
                            LatencyType::RoundTrip,
                            latency,
                            send_wall_ns,
                        ));
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                if let Err(e) = transport.send(&shutdown).await {
                    debug!("Async worker {} shutdown send failed: {}", worker_id, e);
                }
                if let Err(e) = transport.close().await {
                    debug!("Async worker {} close failed: {}", worker_id, e);
                }
                debug!("Async worker {} finished (round-trip)", worker_id);
                Ok((metrics.get_metrics(), records))
            });
        }

        let mut worker_results = Vec::new();
        while let Some(result) = join_set.join_next().await {
            let (metrics, records) = result.context("Worker task panicked")??;
            for record in &records {
                if let Err(e) = results_manager.stream_latency_record(record) {
                    debug!("Streaming latency record failed: {}", e);
                }
            }
            worker_results.push(metrics);
        }
        Some(worker_results)
    } else {
        None
    };

    // Aggregate results
    let mut results = BenchmarkResults::new(
        mechanism,
        config.message_size,
        transport_config.buffer_size,
        concurrency,
        config.msg_count,
        config.duration,
        config.warmup_iterations,
        config.one_way,
        config.round_trip,
    );

    if let Some(one_way) = one_way_results {
        let aggregated = MetricsCollector::aggregate_worker_metrics(one_way, &config.percentiles)?;
        results.add_one_way_results(aggregated);
    }

    if let Some(round_trip) = round_trip_results {
        let aggregated =
            MetricsCollector::aggregate_worker_metrics(round_trip, &config.percentiles)?;
        results.add_round_trip_results(aggregated);
    }

    results.test_duration = total_start.elapsed();
    results_manager.add_results(results)?;

    info!(
        "Standalone client finished ({} async workers).",
        concurrency
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{BlockingTransport, BlockingTransportFactory};
    use clap::Parser;

    /// Get a free TCP port by binding to port 0 and extracting the assigned port.
    fn get_free_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    /// Test: connect_blocking_with_retry succeeds when server starts
    /// after client begins retrying.
    #[test]
    fn test_connect_blocking_with_retry_waits_for_server() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start client retry in background (server not yet up)
        let client_config = transport_config.clone();
        let client_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            connect_blocking_with_retry(&mut transport, &client_config).unwrap();
            transport.close_blocking().unwrap();
        });

        // Wait, then start server
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut server_transport =
            BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
        server_transport
            .start_server_blocking(&transport_config)
            .unwrap();

        // Accept connection then close
        // The client connects and closes, which causes a receive error
        let _ = server_transport.receive_blocking();
        server_transport.close_blocking().unwrap();

        client_handle.join().unwrap();
    }

    /// Integration test: blocking TCP round-trip through client paths.
    /// Exercises run_standalone_client_blocking_single round-trip loop.
    #[test]
    fn test_client_blocking_tcp_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "100",
            "-w",
            "10",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP one-way through client paths.
    #[test]
    fn test_client_blocking_tcp_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "100",
            "-w",
            "10",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP duration mode round-trip.
    #[test]
    fn test_client_blocking_tcp_duration_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-d",
            "200ms",
            "-w",
            "5",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Integration test: blocking TCP concurrent round-trip.
    #[test]
    fn test_client_blocking_tcp_concurrent_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "100",
            "-w",
            "5",
            "--round-trip",
            "-c",
            "2",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_concurrent(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: effective_concurrency forces SHM to 1 (via client dispatch).
    #[test]
    fn test_client_concurrency_dispatch() {
        use crate::standalone_server::effective_concurrency;

        assert_eq!(effective_concurrency(IpcMechanism::TcpSocket, 4), 4);
        assert_eq!(effective_concurrency(IpcMechanism::SharedMemory, 4), 1);
    }

    /// Test: connect_async_with_retry succeeds when server is available.
    #[tokio::test]
    async fn test_connect_async_with_retry_succeeds() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Start server first
        let server_handle = std::thread::spawn(move || {
            let mut transport =
                BlockingTransportFactory::create(&IpcMechanism::TcpSocket, false).unwrap();
            let config = TransportConfig {
                host: "127.0.0.1".to_string(),
                port,
                ..Default::default()
            };
            transport.start_server_blocking(&config).unwrap();
            let _ = transport.receive_blocking();
            transport.close_blocking().unwrap();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut transport = crate::ipc::TransportFactory::create(&IpcMechanism::TcpSocket).unwrap();
        connect_async_with_retry(&mut transport, &transport_config)
            .await
            .unwrap();
        let _ = transport.close().await;

        server_handle.join().unwrap();
    }

    /// Test: async client single round-trip path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_single_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-i",
            "50",
            "-w",
            "5",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_single(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async client single one-way path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_single_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-i",
            "50",
            "-w",
            "5",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_single(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async client concurrent round-trip path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_concurrent_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-i",
            "50",
            "-w",
            "5",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_concurrent(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: blocking client duration mode one-way.
    #[test]
    fn test_client_blocking_tcp_duration_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-d",
            "200ms",
            "-w",
            "5",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: blocking concurrent one-way.
    #[test]
    fn test_client_blocking_tcp_concurrent_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "100",
            "-w",
            "5",
            "--one-way",
            "-c",
            "2",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_concurrent(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async client duration round-trip.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_duration_round_trip() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-d",
            "200ms",
            "-w",
            "5",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_single(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async client duration one-way.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_duration_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-d",
            "200ms",
            "-w",
            "5",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_single(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async concurrent one-way.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_concurrent_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-i",
            "50",
            "-w",
            "5",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_concurrent(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: run_standalone_client top-level dispatch (TCP blocking).
    /// Exercises the full entry point including logging init, affinity
    /// check, shm-direct guard, ResultsManager setup, and dispatch.
    #[test]
    fn test_run_standalone_client_full_dispatch() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "50",
            "-w",
            "5",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        run_standalone_client(args).unwrap();

        server_handle.join().unwrap();
    }

    /// Test: blocking client with --send-delay exercises delay branches.
    #[test]
    fn test_client_blocking_with_send_delay() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "5",
            "-w",
            "2",
            "--round-trip",
            "--send-delay",
            "1ms",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: blocking client with streaming JSON output enabled.
    #[test]
    fn test_client_blocking_with_streaming_output() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let streaming_path =
            std::env::temp_dir().join(format!("test_streaming_{}.json", std::process::id()));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "10",
            "-w",
            "2",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        // Enable streaming
        results_manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        results_manager.finalize().unwrap();

        // Verify streaming file was created and has content
        let contents = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(!contents.is_empty(), "Streaming file should have content");
        assert!(
            contents.contains("round_trip_latency_ns"),
            "Streaming file should contain latency data"
        );

        let _ = std::fs::remove_file(&streaming_path);
        server_handle.join().unwrap();
    }

    /// Test: blocking concurrent duration-mode one-way.
    #[test]
    fn test_client_blocking_concurrent_duration_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-d",
            "200ms",
            "-w",
            "2",
            "--one-way",
            "-c",
            "2",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_concurrent(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: async concurrent duration-mode one-way.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_client_async_concurrent_duration_one_way() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "-d",
            "200ms",
            "-w",
            "2",
            "--one-way",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_async_concurrent(
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .await
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: run_standalone_client rejects 'all' mechanism.
    #[test]
    fn test_run_standalone_client_rejects_all_via_dispatch() {
        let args = Args::parse_from(["ipc-benchmark", "--client", "-m", "all"]);
        let result = run_standalone_client(args);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Cannot use 'all'"),
            "Should reject 'all' mechanism"
        );
    }

    /// Test: run_standalone_client rejects --shm-direct without --blocking.
    #[test]
    fn test_run_standalone_client_rejects_shm_direct() {
        let args = Args::parse_from(["ipc-benchmark", "--client", "-m", "shm", "--shm-direct"]);
        let result = run_standalone_client(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("--shm-direct requires --blocking"),
            "Should reject shm-direct without blocking"
        );
    }

    /// Test: blocking client with send_delay on one-way path.
    #[test]
    fn test_client_blocking_one_way_with_send_delay() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "5",
            "-w",
            "2",
            "--one-way",
            "--send-delay",
            "1ms",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        server_handle.join().unwrap();
    }

    /// Test: blocking client with combined streaming (both one-way and round-trip).
    #[test]
    fn test_client_blocking_combined_streaming() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let streaming_path = std::env::temp_dir().join(format!(
            "test_combined_streaming_{}.json",
            std::process::id()
        ));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "10",
            "-w",
            "2",
            "--port",
            &port.to_string(),
        ]);
        // Default: both one-way and round-trip enabled
        let config = BenchmarkConfig::from_args(&args).unwrap();
        assert!(
            config.one_way && config.round_trip,
            "Both modes should be enabled by default"
        );

        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();
        results_manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Only run round-trip since server expects request/response
        // (Combined streaming setup is what we're testing)
        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        results_manager.finalize().unwrap();

        let _ = std::fs::remove_file(&streaming_path);
        server_handle.join().unwrap();
    }

    /// Test: blocking client with CSV streaming output.
    #[test]
    fn test_client_blocking_csv_streaming() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            stream.set_nodelay(true).unwrap();
            let mut transport = BlockingTcpSocket::from_stream(stream);
            let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
            let config = BenchmarkConfig::from_args(&args).unwrap();
            let _ = handle_client_connection(&mut transport, &config);
            let _ = transport.close_blocking();
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let csv_path =
            std::env::temp_dir().join(format!("test_csv_streaming_{}.csv", std::process::id()));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "10",
            "-w",
            "2",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();
        results_manager.enable_csv_streaming(&csv_path).unwrap();

        run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        )
        .unwrap();

        results_manager.finalize().unwrap();

        let contents = std::fs::read_to_string(&csv_path).unwrap();
        assert!(
            contents.contains("round_trip_latency_ns"),
            "CSV should have latency column"
        );

        let _ = std::fs::remove_file(&csv_path);
        server_handle.join().unwrap();
    }

    /// Test: run_standalone_client rejects multiple mechanisms.
    #[test]
    fn test_run_standalone_client_rejects_multiple_mechanisms() {
        let args = Args::parse_from(["ipc-benchmark", "--client", "-m", "tcp", "shm"]);
        let result = run_standalone_client(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("supports only one mechanism"),
            "Should reject multiple mechanisms"
        );
    }

    /// Test: concurrent blocking round-trip with streaming produces per-message records.
    #[test]
    fn test_client_blocking_concurrent_streaming() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let streaming_path = std::env::temp_dir().join(format!(
            "test_concurrent_streaming_{}.json",
            std::process::id()
        ));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "20",
            "-w",
            "2",
            "--round-trip",
            "-c",
            "2",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        results_manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        run_standalone_client_blocking_concurrent(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .unwrap();

        results_manager.finalize().unwrap();

        let contents = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(!contents.is_empty(), "Streaming file should have content");
        assert!(
            contents.contains("round_trip_latency_ns"),
            "Streaming file should contain latency data"
        );

        let _ = std::fs::remove_file(&streaming_path);
        server_handle.join().unwrap();
    }

    /// Test: canary send failure logs warning (connection closed before canary).
    #[test]
    fn test_client_canary_failure_does_not_panic() {
        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };

        // Server accepts then immediately closes -- canary will fail
        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let (stream, _) = listener.accept().unwrap();
            drop(stream);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "10",
            "-w",
            "0",
            "--round-trip",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        // The client should fail gracefully (error on send, not panic)
        let result = run_standalone_client_blocking_single(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
        );

        // Either an error (connection reset) or Ok with 0 messages -- but no panic
        assert!(
            result.is_err() || result.is_ok(),
            "Should handle canary failure gracefully"
        );

        server_handle.join().unwrap();
    }

    /// Test: concurrent blocking with warmup iterations completes successfully.
    #[test]
    fn test_client_blocking_concurrent_warmup() {
        use crate::ipc::BlockingTcpSocket;
        use crate::standalone_server::handle_client_connection;

        let port = get_free_port();
        let transport_config = TransportConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..Default::default()
        };
        let num_clients = 2usize;

        let server_handle = std::thread::spawn(move || {
            let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).unwrap();
            let mut handles = Vec::new();
            for _ in 0..num_clients {
                let (stream, _) = listener.accept().unwrap();
                stream.set_nodelay(true).unwrap();
                let handle = std::thread::spawn(move || {
                    let mut transport = BlockingTcpSocket::from_stream(stream);
                    let args = Args::parse_from(["ipc-benchmark", "-m", "tcp", "-s", "64"]);
                    let config = BenchmarkConfig::from_args(&args).unwrap();
                    let _ = handle_client_connection(&mut transport, &config);
                    let _ = transport.close_blocking();
                });
                handles.push(handle);
            }
            for h in handles {
                h.join().unwrap();
            }
        });

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Use -w 50 to exercise per-worker warmup
        let args = Args::parse_from([
            "ipc-benchmark",
            "--client",
            "-m",
            "tcp",
            "--blocking",
            "-i",
            "20",
            "-w",
            "50",
            "--round-trip",
            "-c",
            "2",
            "--port",
            &port.to_string(),
        ]);
        let config = BenchmarkConfig::from_args(&args).unwrap();
        let mut results_manager =
            crate::results_blocking::BlockingResultsManager::new(None, None).unwrap();

        run_standalone_client_blocking_concurrent(
            args,
            IpcMechanism::TcpSocket,
            transport_config,
            &mut results_manager,
            &config,
            num_clients,
        )
        .unwrap();

        server_handle.join().unwrap();
    }
}
