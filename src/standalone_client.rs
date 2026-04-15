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
use tracing::{debug, error, info};
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

        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(log_level)
            .event_format(ColorizedFormatter)
            .init();
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
            let _ = transport.send_blocking(&canary);
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
            if transport.send_blocking(&canary).is_ok() {
                let _ = transport.receive_blocking();
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
                let _ = results_manager.stream_latency_record(&record);

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
                let _ = results_manager.stream_latency_record(&record);

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
    let _ = transport.send_blocking(&shutdown);

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

                    let mut metrics = MetricsCollector::new(None, percentiles)?;
                    let payload = vec![0u8; message_size];

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                        let _ = transport.send_blocking(&canary);
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
                    let _ = transport.send_blocking(&shutdown);
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
                let shm_direct = args.shm_direct;
                let mech = mechanism;
                let worker_msg_count = base_messages_per_worker
                    + if worker_id == concurrency - 1 {
                        remainder_messages
                    } else {
                        0
                    };

                std::thread::spawn(move || -> Result<PerformanceMetrics> {
                    let mut transport = BlockingTransportFactory::create(&mech, shm_direct)?;
                    connect_blocking_with_retry(&mut transport, &tc)?;
                    debug!("Worker {} connected (round-trip)", worker_id);

                    let mut metrics =
                        MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                    let payload = vec![0u8; message_size];

                    if !include_first {
                        let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                        if transport.send_blocking(&canary).is_ok() {
                            let _ = transport.receive_blocking();
                        }
                    }

                    let mut msg = Message::new(0, payload, MessageType::Request);

                    if let Some(test_duration) = duration {
                        let start = std::time::Instant::now();
                        let mut i = 0u64;
                        while start.elapsed() < test_duration {
                            msg.id = worker_id as u64 * u32::MAX as u64 + i;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                            i += 1;
                        }
                    } else {
                        for i in 0..worker_msg_count {
                            msg.id = (worker_id * base_messages_per_worker + i) as u64;
                            msg.timestamp = get_monotonic_time_ns();
                            let send_time = std::time::Instant::now();
                            transport.send_blocking(&msg)?;
                            let _response = transport.receive_blocking()?;
                            let latency = send_time.elapsed();
                            metrics.record_message(message_size, Some(latency))?;
                            if let Some(delay) = send_delay {
                                std::thread::sleep(delay);
                            }
                        }
                    }

                    let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                    let _ = transport.send_blocking(&shutdown);
                    transport.close_blocking()?;
                    debug!("Worker {} finished (round-trip)", worker_id);
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
            let _ = transport.send(&canary).await;
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
            if transport.send(&canary).await.is_ok() {
                let _ = transport.receive().await;
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
                let _ = results_manager.stream_latency_record(&record);

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
                let _ = results_manager.stream_latency_record(&record);

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
    let _ = transport.send(&shutdown).await;

    let _ = transport.close().await;
    info!("Standalone client finished.");
    Ok(())
}

/// Multi-task async standalone client.
///
/// Spawns `concurrency` tokio tasks, each creating its own async transport
/// connection. Results are aggregated across all workers.
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

                let mut metrics = MetricsCollector::new(None, percentiles)?;
                let payload = vec![0u8; message_size];

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::OneWay);
                    let _ = transport.send(&canary).await;
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
                let _ = transport.send(&shutdown).await;
                let _ = transport.close().await;
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
        let mut join_set: JoinSet<Result<PerformanceMetrics>> = JoinSet::new();
        for worker_id in 0..concurrency {
            let tc = transport_config.clone();
            let percentiles = config.percentiles.clone();
            let message_size = config.message_size;
            let duration = config.duration;
            let send_delay = config.send_delay;
            let include_first = config.include_first_message;
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

                let mut metrics = MetricsCollector::new(Some(LatencyType::RoundTrip), percentiles)?;
                let payload = vec![0u8; message_size];

                if !include_first {
                    let canary = Message::new(u64::MAX, payload.clone(), MessageType::Request);
                    if transport.send(&canary).await.is_ok() {
                        let _ = transport.receive().await;
                    }
                }

                let mut msg = Message::new(0, payload, MessageType::Request);

                if let Some(test_duration) = duration {
                    let start = std::time::Instant::now();
                    let mut i = 0u64;
                    while start.elapsed() < test_duration {
                        msg.id = worker_id as u64 * u32::MAX as u64 + i;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                        i += 1;
                    }
                } else {
                    for i in 0..worker_msg_count {
                        msg.id = (worker_id * base_messages_per_worker + i) as u64;
                        msg.timestamp = get_monotonic_time_ns();
                        let send_time = std::time::Instant::now();
                        transport.send(&msg).await?;
                        let _response = transport.receive().await?;
                        let latency = send_time.elapsed();
                        metrics.record_message(message_size, Some(latency))?;
                        if let Some(delay) = send_delay {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }

                let shutdown = Message::new(u64::MAX, Vec::new(), MessageType::Shutdown);
                let _ = transport.send(&shutdown).await;
                let _ = transport.close().await;
                debug!("Async worker {} finished (round-trip)", worker_id);
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
    use crate::ipc::BlockingTransportFactory;

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
}
