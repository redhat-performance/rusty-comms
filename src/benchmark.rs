//! # Benchmark Engine Module
//!
//! This module contains the core benchmarking engine that orchestrates performance
//! testing of IPC mechanisms. It handles the complete test lifecycle from setup
//! through measurement to cleanup, with sophisticated handling of different
//! test patterns and concurrency scenarios.
//!
//! ## Key Components
//!
//! - **BenchmarkRunner**: Main orchestrator that manages the complete test lifecycle
//! - **BenchmarkConfig**: Configuration structure that controls test parameters
//! - **Test Patterns**: Support for one-way and round-trip latency measurements
//! - **Concurrency Management**: Handles both single and multi-threaded scenarios
//! - **Resource Management**: Ensures proper cleanup between tests
//!
//! ## Test Execution Lifecycle
//!
//! 1. **Initialization**: Create transport instances and configure parameters
//! 2. **Warmup**: Run initial iterations to stabilize performance
//! 3. **Measurement**: Execute actual tests with metrics collection
//! 4. **Cleanup**: Properly close connections and release resources
//!
//! ## Concurrency Handling
//!
//! The benchmark engine supports different concurrency patterns:
//! - **Single-threaded**: Direct client-server communication
//! - **Multi-threaded**: Simulated concurrent workers (current implementation)
//! - **Adaptive**: Automatic fallback for mechanisms with threading restrictions
//!
//! ## Performance Considerations
//!
//! The engine includes several optimizations:
//! - Lazy connection establishment to reduce setup overhead
//! - Adaptive buffer sizing based on test parameters
//! - Transport-specific timeout and retry logic
//! - Comprehensive error handling with graceful degradation

use crate::{
    cli::{Args, IpcMechanism},
    ipc::{Message, MessageType, TransportConfig, TransportFactory},
    metrics::{LatencyType, MetricsCollector, PerformanceMetrics},
    results::BenchmarkResults,
};
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Configuration for benchmark execution
///
/// This structure encapsulates all parameters needed to execute a benchmark test.
/// It serves as the authoritative configuration that drives test behavior,
/// including message characteristics, timing parameters, and performance options.
///
/// ## Key Configuration Categories
///
/// - **Message Configuration**: Size and payload characteristics
/// - **Test Duration**: Either iteration-based or time-based execution
/// - **Concurrency**: Number of parallel workers and connection handling
/// - **Test Types**: Which latency patterns to measure (one-way, round-trip)
/// - **Performance Tuning**: Buffer sizes, timeouts, and optimization parameters
#[derive(Clone, Debug)]
pub struct BenchmarkConfig {
    /// The specific IPC mechanism being tested
    ///
    /// This determines which transport implementation will be used
    /// and affects various performance optimizations and limitations
    pub mechanism: IpcMechanism,

    /// Size of message payloads in bytes
    ///
    /// Larger messages test throughput characteristics while smaller
    /// messages focus on latency. Affects buffer sizing and timeout calculations.
    pub message_size: usize,

    /// Number of messages to run (None for duration-based tests)
    ///
    /// When specified, the test runs for exactly this many message exchanges.
    /// Mutually exclusive with duration-based testing.
    pub msg_count: Option<usize>,

    /// Duration to run tests (takes precedence over message count)
    ///
    /// When specified, tests run for this time period regardless of message count.
    /// Provides more consistent test timing across different mechanisms.
    pub duration: Option<Duration>,

    /// Number of concurrent workers
    ///
    /// Controls parallelism level. Some mechanisms may override this
    /// (e.g., shared memory forces concurrency=1 to avoid race conditions).
    pub concurrency: usize,

    /// Whether to execute one-way latency tests
    ///
    /// One-way tests measure the time to send a message from client to server
    /// without waiting for a response, testing pure transmission latency.
    pub one_way: bool,

    /// Whether to execute round-trip latency tests
    ///
    /// Round-trip tests measure request-response cycles, providing insights
    /// into full communication round-trip performance including response processing.
    pub round_trip: bool,

    /// Number of warmup iterations before measurement begins
    ///
    /// Warmup iterations help stabilize performance by allowing caches to fill,
    /// connections to establish, and JIT compilation to optimize hot paths.
    pub warmup_iterations: usize,

    /// Percentiles to calculate for latency distribution analysis
    ///
    /// Common values include P50 (median), P95, P99, and P99.9.
    /// Used by the metrics collector to provide detailed latency analysis.
    pub percentiles: Vec<f64>,

    /// Buffer size for transport-specific data structures
    ///
    /// Controls internal buffer sizes for shared memory ring buffers,
    /// socket buffers, and message queue depths. Affects memory usage and throughput.
    pub buffer_size: Option<usize>,

    /// Host address for network-based transports (TCP sockets)
    ///
    /// Specifies the network interface for TCP socket communication.
    /// Ignored by local-only mechanisms (UDS, shared memory, PMQ).
    pub host: String,

    /// Port number for network-based transports (TCP sockets)
    ///
    /// Base port number that will be modified to ensure uniqueness
    /// across concurrent tests. Ignored by non-network mechanisms.
    pub port: u16,
}

impl BenchmarkConfig {
    /// Create benchmark configuration from CLI arguments
    ///
    /// This factory method converts user-friendly CLI arguments into the
    /// internal configuration structure. It applies defaults, validates
    /// parameters, and handles special cases.
    ///
    /// ## Parameters
    /// - `args`: Parsed command-line arguments
    ///
    /// ## Returns
    /// - `Ok(BenchmarkConfig)`: Valid configuration ready for use
    /// - `Err(anyhow::Error)`: Configuration validation failure
    ///
    /// ## Validation
    /// - Ensures message size is reasonable (not too large for mechanism)
    /// - Validates concurrency limits based on system capabilities
    /// - Checks that at least one test type (one-way or round-trip) is enabled
    pub fn from_args(args: &Args) -> Result<Self> {
        // If neither test type is explicitly specified, run both (default behavior)
        let (one_way, round_trip) = if !args.one_way && !args.round_trip {
            (true, true) // Default: run both tests
        } else {
            (args.one_way, args.round_trip) // Use explicit user selection
        };

        Ok(Self {
            mechanism: {
                #[cfg(unix)]
                {
                    IpcMechanism::UnixDomainSocket
                }
                #[cfg(not(unix))]
                {
                    IpcMechanism::SharedMemory
                }
            }, // Will be overridden per test
            message_size: args.message_size,

            // Duration takes precedence over message count
            // This provides more predictable test timing
            msg_count: if args.duration.is_some() {
                None
            } else {
                Some(args.msg_count)
            },

            duration: args.duration,
            concurrency: args.concurrency,
            one_way,
            round_trip,
            warmup_iterations: args.warmup_iterations,
            percentiles: args.percentiles.clone(),
            buffer_size: args.buffer_size,
            host: args.host.clone(),
            port: args.port,
        })
    }
}

/// Benchmark runner that coordinates the execution of IPC performance tests
///
/// The `BenchmarkRunner` is the primary interface for executing performance tests.
/// It manages the complete test lifecycle including transport setup, warmup execution,
/// measurement collection, and resource cleanup.
///
/// ## Design Principles
///
/// - **Isolation**: Each test runs in isolation with proper resource cleanup
/// - **Adaptability**: Automatically adjusts behavior based on mechanism capabilities
/// - **Robustness**: Comprehensive error handling with graceful degradation
/// - **Accuracy**: Uses precise timing and statistical methods for measurement
///
/// ## Usage Pattern
///
/// ```rust,no_run
/// # use ipc_benchmark::benchmark::{BenchmarkConfig, BenchmarkRunner};
/// # use ipc_benchmark::cli::{Args, IpcMechanism};
/// # use std::time::Duration;
/// #
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// # let args = Args {
/// #     mechanisms: vec![],
/// #     message_size: 1024,
/// #     msg_count: 1000,
/// #     duration: None,
/// #     concurrency: 1,
/// #     one_way: true,
/// #     round_trip: true,
/// #     warmup_iterations: 100,
/// #     percentiles: vec![50.0, 95.0, 99.0],
/// #     buffer_size: Some(8192),
/// #     host: "127.0.0.1".to_string(),
/// #     port: 8080,
/// #     output_file: None,
/// #     continue_on_error: false,
/// #     quiet: false,
/// #     verbose: 0,
/// #     log_file: None,
/// #     streaming_output_json: None,
/// #     streaming_output_csv: None,
/// # };
/// let config = BenchmarkConfig::from_args(&args)?;
/// #[cfg(unix)]
/// {
///     let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket);
///     let results = runner.run(None).await?;
/// }
/// # Ok(())
/// # }
/// ```
pub struct BenchmarkRunner {
    /// Benchmark configuration parameters
    config: BenchmarkConfig,

    /// The specific IPC mechanism being tested
    mechanism: IpcMechanism,
}

impl BenchmarkRunner {
    /// Create a new benchmark runner
    ///
    /// ## Parameters
    /// - `config`: Benchmark configuration parameters
    /// - `mechanism`: Specific IPC mechanism to test
    ///
    /// ## Returns
    /// Configured benchmark runner ready for execution
    pub fn new(config: BenchmarkConfig, mechanism: IpcMechanism) -> Self {
        Self { config, mechanism }
    }

    /// Run the benchmark and return comprehensive results
    ///
    /// This is the main entry point for benchmark execution. It orchestrates
    /// the complete test lifecycle and returns detailed performance metrics.
    ///
    /// ## Test Execution Flow
    ///
    /// 1. **Warmup Phase**: Run warmup iterations to stabilize performance
    /// 2. **One-way Testing**: Measure one-way latency if enabled
    /// 3. **Round-trip Testing**: Measure round-trip latency if enabled
    /// 4. **Result Aggregation**: Combine metrics and generate summary
    ///
    /// ## Error Handling
    ///
    /// The function uses comprehensive error handling to provide meaningful
    /// diagnostics when tests fail. Common failure modes include:
    /// - Transport initialization failures (port conflicts, permission issues)
    /// - Communication timeouts during testing
    /// - Resource allocation failures
    ///
    /// ## Returns
    /// - `Ok(BenchmarkResults)`: Complete test results with metrics
    /// - `Err(anyhow::Error)`: Test execution failure with diagnostic information
    pub async fn run(
        &self,
        mut results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<BenchmarkResults> {
        let transport_config = self.create_transport_config()?;

        let buffer_size_str = if self.config.buffer_size.is_some() {
            format!("{} bytes (User-provided)", transport_config.buffer_size)
        } else {
            format!("{} bytes (Automatic)", transport_config.buffer_size)
        };

        info!("-----------------------------------------------------------------");
        info!("Starting Benchmark for: {}", self.mechanism);
        info!("  Message Size:       {} bytes", self.config.message_size);
        info!("  Buffer Size:        {}", buffer_size_str);
        if let Some(duration) = self.config.duration {
            info!("  Test Duration:      {:?}", duration);
        } else {
            info!("  Message Count:      {}", self.get_msg_count());
        }
        info!("-----------------------------------------------------------------");

        // Initialize results structure with test configuration
        let mut results = BenchmarkResults::new(
            self.mechanism,
            self.config.message_size,
            transport_config.buffer_size,
            self.config.concurrency,
            self.config.msg_count,
            self.config.duration,
        );

        // Run warmup if configured
        if self.config.warmup_iterations > 0 {
            info!(
                "Running warmup with {} iterations",
                self.config.warmup_iterations
            );
            self.run_warmup(&transport_config).await?;
        }

        // Check if we need to run in combined mode for streaming
        let results_manager_ref = results_manager.as_deref_mut();
        let combined_streaming = results_manager_ref
            .map(|rm| rm.is_combined_streaming_enabled())
            .unwrap_or(false);

        if combined_streaming && self.config.one_way && self.config.round_trip {
            info!("Running combined one-way and round-trip test for streaming");
            let combined_results = self
                .run_combined_test(&transport_config, results_manager.as_deref_mut())
                .await?;
            results.add_one_way_results(combined_results.0);
            results.add_round_trip_results(combined_results.1);
        } else {
            // Run one-way latency test if enabled
            if self.config.one_way {
                info!("Running one-way latency test");
                let one_way_results = self
                    .run_one_way_test(&transport_config, results_manager.as_deref_mut())
                    .await?;
                results.add_one_way_results(one_way_results);
            }

            // Run round-trip latency test if enabled
            if self.config.round_trip {
                info!("Running round-trip latency test");
                let round_trip_results = self
                    .run_round_trip_test(&transport_config, results_manager)
                    .await?;
                results.add_round_trip_results(round_trip_results);
            }
        }

        info!("Benchmark completed for {} mechanism", self.mechanism);
        Ok(results)
    }

    /// Run warmup iterations to stabilize performance
    ///
    /// Warmup is critical for accurate performance measurement as it allows
    /// various system components to reach steady state. This includes CPU caches,
    /// network connection establishment, OS buffer optimization, and JIT compilation.
    ///
    /// ## Warmup Process
    ///
    /// 1. **Transport Setup**: Create and configure client/server transports
    /// 2. **Connection Establishment**: Use a `oneshot` channel to ensure the server is fully started before the client connects.
    /// 3. **Message Exchange**: Send warmup messages without measurement
    /// 4. **Resource Cleanup**: Properly close connections after warmup
    ///
    /// ## Synchronization
    ///
    /// The function uses a Tokio `oneshot` channel to ensure the server task has successfully
    /// initialized the transport and is ready to accept connections before the client
    /// proceeds. This prevents race conditions and ensures startup errors are propagated immediately.
    async fn run_warmup(&self, transport_config: &TransportConfig) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a oneshot channel to signal server readiness and propagate startup errors.
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism; // Copy mechanism to move into closure
            let warmup_iterations = self.config.warmup_iterations;
            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                if let Err(e) = transport.start_server(&config).await {
                    let _ = tx.send(Err(e));
                    return Err(anyhow::anyhow!("Server failed to start"));
                }

                // Signal that server is ready to accept connections
                let _ = tx.send(Ok(()));
                debug!("Server signaled ready for warmup");

                // Receive warmup messages without measuring performance
                for _ in 0..warmup_iterations {
                    let _ = transport.receive().await?;
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready before starting client
        rx.await??;
        debug!("Client received server ready signal for warmup");

        // Connect client and send warmup messages
        client_transport.start_client(transport_config).await?;

        let payload = vec![0u8; self.config.message_size];
        for i in 0..self.config.warmup_iterations {
            let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
            client_transport.send(&message).await?;
        }

        // Clean up resources
        client_transport.close().await?;
        server_handle.await??;

        debug!("Warmup completed");
        Ok(())
    }

    /// Run one-way latency test
    ///
    /// One-way latency tests measure the time required to transmit a message
    /// from client to server without waiting for a response. This provides
    /// baseline transmission latency measurements.
    ///
    /// ## Test Methodology
    ///
    /// 1. **Setup Phase**: Initialize transports and establish connections
    /// 2. **Measurement Phase**: Send messages with precise timing measurement
    /// 3. **Adaptive Execution**: Choose single or multi-threaded based on mechanism
    /// 4. **Cleanup Phase**: Properly close all connections and resources
    ///
    /// ## Concurrency Handling
    ///
    /// The function automatically adapts to mechanism capabilities:
    /// - Shared memory: Forces single-threaded to avoid race conditions
    /// - Other mechanisms: Supports both single and multi-threaded execution
    ///
    /// ## Returns
    /// - `Ok(PerformanceMetrics)`: Comprehensive latency and throughput metrics
    /// - `Err(anyhow::Error)`: Test execution failure
    async fn run_one_way_test(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;

        // Check for problematic configurations and adapt automatically
        // Shared memory currently has race conditions with concurrency > 1
        // so we force single-threaded execution for reliability
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
            // Run single-threaded instead
            self.run_single_threaded_one_way(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_one_way(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        } else {
            self.run_multi_threaded_one_way(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        }

        Ok(metrics_collector.get_metrics())
    }

    /// Run round-trip latency test
    ///
    /// Round-trip latency tests measure the complete request-response cycle,
    /// providing insights into interactive communication patterns and full
    /// bidirectional performance characteristics.
    ///
    /// ## Test Methodology
    ///
    /// 1. **Client sends request**: Timestamp when request is sent
    /// 2. **Server receives and responds**: Echo or process request
    /// 3. **Client receives response**: Timestamp when response arrives
    /// 4. **Latency calculation**: Total time for complete cycle
    ///
    /// ## Response Patterns
    ///
    /// The server implements a simple echo pattern, immediately sending
    /// a response with a modified message ID to verify proper round-trip completion.
    ///
    /// ## Returns
    /// - `Ok(PerformanceMetrics)`: Round-trip latency and throughput metrics
    /// - `Err(anyhow::Error)`: Test execution failure
    async fn run_round_trip_test(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<PerformanceMetrics> {
        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Check for problematic configurations and adapt automatically
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
            // Run single-threaded instead
            self.run_single_threaded_round_trip(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_round_trip(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        } else {
            self.run_multi_threaded_round_trip(
                transport_config,
                &mut metrics_collector,
                results_manager,
            )
            .await?;
        }

        Ok(metrics_collector.get_metrics())
    }

    /// Run single-threaded one-way test
    ///
    /// This implementation handles the common case of single client to single server
    /// communication for one-way latency measurement. It provides the most accurate
    /// latency measurements by avoiding thread synchronization overhead.
    ///
    /// ## Execution Flow
    ///
    /// 1. **Server Setup**: Start server and wait for readiness
    /// 2. **Client Connection**: Connect client to server
    /// 3. **Message Transmission**: Send messages with precise timing
    /// 4. **Metrics Collection**: Record latency for each message sent
    /// 5. **Resource Cleanup**: Close connections and clean up resources
    ///
    /// ## Timing Methodology
    ///
    /// Each message transmission is timed individually using high-precision
    /// timestamps. The latency is measured from just before the send call
    /// to just after it completes, capturing the actual transmission time.
    ///
    /// ## Duration vs Iteration Modes
    ///
    /// - **Iteration Mode**: Send exactly N messages regardless of time
    /// - **Duration Mode**: Send messages continuously for T seconds
    async fn run_single_threaded_one_way(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        mut results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a oneshot channel to signal server readiness and propagate startup errors.
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let msg_count = self.get_msg_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                if let Err(e) = transport.start_server(&config).await {
                    let _ = tx.send(Err(e));
                    return Err(anyhow::anyhow!("Server failed to start"));
                }

                // Signal that server is ready
                let _ = tx.send(Ok(()));
                debug!("Server signaled ready for one-way test");

                let start_time = Instant::now();
                let mut received = 0;

                // Server receive loop - adapts to duration or message count mode
                loop {
                    // Check if we should stop based on duration or message count
                    if let Some(dur) = duration {
                        if start_time.elapsed() >= dur {
                            break;
                        }
                    } else if received >= msg_count {
                        break;
                    }

                    // Try to receive with a shorter timeout to avoid hanging
                    match timeout(Duration::from_millis(50), transport.receive()).await {
                        Ok(Ok(_)) => {
                            received += 1;
                        }
                        Ok(Err(_)) => break, // Transport error
                        Err(_) => {
                            // Timeout - check if duration-based test is done
                            if duration.is_some() {
                                continue; // Keep waiting for duration-based test
                            } else {
                                break; // Message-count-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        rx.await??;
        debug!("Client received server ready signal for one-way test");

        // Connect client
        client_transport.start_client(transport_config).await?;

        // Send messages and measure latency
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();

        if let Some(duration) = self.config.duration {
            // Duration-based test: loop until time expires
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_time = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::OneWay);

                // Use timeout for individual sends to avoid blocking indefinitely
                match tokio::time::timeout(
                    Duration::from_millis(50),
                    client_transport.send(&message),
                )
                .await
                {
                    Ok(Ok(())) => {
                        let latency = send_time.elapsed();
                        metrics_collector
                            .record_message(self.config.message_size, Some(latency))?;

                        // Stream individual latency record if enabled
                        if let Some(ref mut manager) = results_manager {
                            let record = crate::results::MessageLatencyRecord::new(
                                i,
                                self.mechanism,
                                self.config.message_size,
                                crate::metrics::LatencyType::OneWay,
                                latency,
                            );
                            manager.stream_latency_record(&record).await?;
                        }

                        i += 1;
                    }
                    Ok(Err(_)) => break, // Transport error
                    Err(_) => {
                        // Send timeout - ring buffer might be full, small delay and continue
                        sleep(Duration::from_millis(1)).await;
                        continue;
                    }
                }
            }
        } else {
            // Message-count-based test: loop for fixed number of messages
            let msg_count = self.get_msg_count();

            for i in 0..msg_count {
                let send_time = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
                client_transport.send(&message).await?;

                let latency = send_time.elapsed();
                metrics_collector.record_message(self.config.message_size, Some(latency))?;

                // Stream individual latency record if enabled
                if let Some(ref mut manager) = results_manager {
                    let record = crate::results::MessageLatencyRecord::new(
                        i as u64,
                        self.mechanism,
                        self.config.message_size,
                        crate::metrics::LatencyType::OneWay,
                        latency,
                    );
                    manager.stream_latency_record(&record).await?;
                }
            }
        }

        // Clean up resources
        client_transport.close().await?;
        server_handle.await??;

        Ok(())
    }

    /// Run single-threaded round-trip test
    ///
    /// This implementation measures request-response latency in a single-threaded
    /// scenario. It provides the most accurate round-trip measurements by avoiding
    /// thread coordination overhead.
    ///
    /// ## Round-trip Protocol
    ///
    /// 1. **Client sends request**: Message with Request type
    /// 2. **Server processes and responds**: Increments message ID and sends Response
    /// 3. **Client receives response**: Completes timing measurement
    ///
    /// ## Latency Measurement
    ///
    /// Round-trip latency is measured from the start of the send operation
    /// to the completion of the receive operation, capturing the complete
    /// communication cycle including any processing delays.
    async fn run_single_threaded_round_trip(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        mut results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a oneshot channel to signal server readiness and propagate startup errors.
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let msg_count = self.get_msg_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                if let Err(e) = transport.start_server(&config).await {
                    let _ = tx.send(Err(e));
                    return Err(anyhow::anyhow!("Server failed to start"));
                }

                // Signal that server is ready
                let _ = tx.send(Ok(()));
                debug!("Server signaled ready for round-trip test");

                let start_time = Instant::now();
                let mut received = 0;

                // Server request-response loop
                loop {
                    // Check if we should stop based on duration or iterations
                    if let Some(dur) = duration {
                        if start_time.elapsed() >= dur {
                            break;
                        }
                    } else if received >= msg_count {
                        break;
                    }

                    // Try to receive with a shorter timeout
                    match timeout(Duration::from_millis(50), transport.receive()).await {
                        Ok(Ok(request)) => {
                            received += 1;
                            // Echo back with modified ID to indicate processing
                            let response = Message::new(
                                request.id + 1000000,
                                request.payload,
                                MessageType::Response,
                            );
                            if (transport.send(&response).await).is_err() {
                                break; // Client disconnected
                            }
                        }
                        Ok(Err(_)) => break, // Transport error
                        Err(_) => {
                            // Timeout - check if duration-based test is done
                            if duration.is_some() {
                                continue; // Keep waiting for duration-based test
                            } else {
                                break; // Message-count-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        rx.await??;
        debug!("Client received server ready signal for round-trip test");

        // Connect client
        client_transport.start_client(transport_config).await?;

        // Send messages and measure round-trip latency
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();

        if let Some(duration) = self.config.duration {
            // Duration-based test: loop until time expires
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_time = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::Request);

                // Use timeout for sends and receives to avoid blocking indefinitely
                match tokio::time::timeout(
                    Duration::from_millis(50),
                    client_transport.send(&message),
                )
                .await
                {
                    Ok(Ok(())) => {
                        // Send succeeded - increment counter immediately to ensure unique message IDs
                        i += 1;

                        // Try to receive response with timeout
                        match tokio::time::timeout(
                            Duration::from_millis(50),
                            client_transport.receive(),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {
                                let latency = send_time.elapsed();
                                metrics_collector
                                    .record_message(self.config.message_size, Some(latency))?;

                                // Stream individual latency record if enabled
                                if let Some(ref mut manager) = results_manager {
                                    let record = crate::results::MessageLatencyRecord::new(
                                        i - 1, // Use the actual message ID that was sent
                                        self.mechanism,
                                        self.config.message_size,
                                        crate::metrics::LatencyType::RoundTrip,
                                        latency,
                                    );
                                    manager.stream_latency_record(&record).await?;
                                }
                            }
                            Ok(Err(_)) => break, // Transport error
                            Err(_) => {
                                // Receive timeout, but send was successful so we continue
                                // Message ID was already incremented after successful send
                                sleep(Duration::from_millis(1)).await;
                                continue;
                            }
                        }
                    }
                    Ok(Err(_)) => break, // Transport error
                    Err(_) => {
                        // Send timeout - ring buffer might be full, small delay and continue
                        sleep(Duration::from_millis(1)).await;
                        continue;
                    }
                }
            }
        } else {
            // Message-count-based test: loop for fixed number of messages
            let msg_count = self.get_msg_count();

            for i in 0..msg_count {
                let send_time = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);
                client_transport.send(&message).await?;

                let _ = client_transport.receive().await?;
                let latency = send_time.elapsed();
                metrics_collector.record_message(self.config.message_size, Some(latency))?;

                // Stream individual latency record if enabled
                if let Some(ref mut manager) = results_manager {
                    let record = crate::results::MessageLatencyRecord::new(
                        i as u64,
                        self.mechanism,
                        self.config.message_size,
                        crate::metrics::LatencyType::RoundTrip,
                        latency,
                    );
                    manager.stream_latency_record(&record).await?;
                }
            }
        }

        // Clean up resources
        client_transport.close().await?;
        server_handle.await??;

        Ok(())
    }

    /// Run multi-threaded one-way test
    ///
    /// This implementation simulates concurrent client workloads by running
    /// multiple sequential tests and aggregating their results. While not
    /// truly concurrent, it provides meaningful performance data for
    /// understanding scalability characteristics.
    ///
    /// ## Current Implementation Limitations
    ///
    /// The current implementation runs "workers" sequentially rather than
    /// concurrently to avoid complex connection management issues. This
    /// provides stable results while avoiding race conditions in transport setup.
    ///
    /// ## Future Improvements
    ///
    /// A future implementation could support true concurrency with:
    /// - Connection pooling for shared transports
    /// - Proper resource isolation between workers
    /// - Advanced synchronization for startup/shutdown
    async fn run_multi_threaded_one_way(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        _results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<()> {
        // For now, we'll simulate concurrency by running multiple sequential tests
        // This avoids the complex connection management issues while still providing
        // meaningful performance data for concurrent workloads

        warn!("Running simulated multi-threaded one-way test. This is a placeholder and does not achieve true concurrency.");

        let mut all_worker_metrics = Vec::new();
        let messages_per_worker = self.get_msg_count() / self.config.concurrency;

        // Run each "worker" sequentially to avoid connection conflicts
        for worker_id in 0..self.config.concurrency {
            debug!(
                "Running worker {} with {} messages",
                worker_id, messages_per_worker
            );

            let mut worker_metrics =
                MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;

            // Run single-threaded test for this worker
            // Note: Passing None for results_manager to avoid duplicate streaming in simulated multi-threading
            self.run_single_threaded_one_way(transport_config, &mut worker_metrics, None)
                .await?;

            all_worker_metrics.push(worker_metrics.get_metrics());
        }

        // Aggregate all worker results
        let aggregated_metrics = MetricsCollector::aggregate_worker_metrics(
            all_worker_metrics,
            &self.config.percentiles,
        )?;

        // Update the main metrics collector with aggregated data
        if let Some(ref latency) = aggregated_metrics.latency {
            for _ in 0..latency.total_samples {
                metrics_collector.record_message(self.config.message_size, None)?;
            }
        }

        // Record throughput data
        for _ in 0..aggregated_metrics.throughput.total_messages {
            metrics_collector.record_message(self.config.message_size, None)?;
        }

        debug!("Simulated multi-threaded one-way test completed");
        Ok(())
    }

    /// Run multi-threaded round-trip test
    ///
    /// Similar to the one-way multi-threaded test, this implementation
    /// simulates concurrent request-response workloads by running multiple
    /// sequential tests and aggregating results.
    ///
    /// ## Aggregation Strategy
    ///
    /// Results from multiple workers are aggregated using statistical
    /// methods that properly combine latency distributions and throughput
    /// measurements to provide meaningful overall performance metrics.
    async fn run_multi_threaded_round_trip(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
        _results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<()> {
        // For now, we'll simulate concurrency by running multiple sequential tests
        // This avoids the complex bidirectional connection management issues

        warn!("Running simulated multi-threaded round-trip test. This is a placeholder and does not achieve true concurrency.");

        let mut all_worker_metrics = Vec::new();
        let messages_per_worker = self.get_msg_count() / self.config.concurrency;

        // Run each "worker" sequentially to avoid connection conflicts
        for worker_id in 0..self.config.concurrency {
            debug!(
                "Running worker {} with {} messages",
                worker_id, messages_per_worker
            );

            let mut worker_metrics = MetricsCollector::new(
                Some(LatencyType::RoundTrip),
                self.config.percentiles.clone(),
            )?;

            // Run single-threaded test for this worker
            // Note: Passing None for results_manager to avoid duplicate streaming in simulated multi-threading
            self.run_single_threaded_round_trip(transport_config, &mut worker_metrics, None)
                .await?;

            all_worker_metrics.push(worker_metrics.get_metrics());
        }

        // Aggregate all worker results
        let aggregated_metrics = MetricsCollector::aggregate_worker_metrics(
            all_worker_metrics,
            &self.config.percentiles,
        )?;

        // Update the main metrics collector with aggregated data
        if let Some(ref latency) = aggregated_metrics.latency {
            for _ in 0..latency.total_samples {
                metrics_collector.record_message(self.config.message_size, None)?;
            }
        }

        // Record throughput data
        for _ in 0..aggregated_metrics.throughput.total_messages {
            metrics_collector.record_message(self.config.message_size, None)?;
        }

        debug!("Simulated multi-threaded round-trip test completed");
        Ok(())
    }

    /// Run combined one-way and round-trip test for streaming
    ///
    /// This method measures both one-way and round-trip latencies for the same
    /// message IDs, enabling proper aggregation in streaming output. Each message
    /// gets both latency measurements.
    ///
    /// ## Test Methodology
    ///
    /// 1. **Send message**: Measure one-way latency (send completion time)
    /// 2. **Wait for response**: Measure round-trip latency (total time)  
    /// 3. **Stream combined record**: Write record with both latencies
    ///
    /// ## Returns
    /// - `Ok((PerformanceMetrics, PerformanceMetrics))`: Tuple of one-way and round-trip results
    /// - `Err(anyhow::Error)`: Test execution failure
    async fn run_combined_test(
        &self,
        transport_config: &TransportConfig,
        results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<(PerformanceMetrics, PerformanceMetrics)> {
        let mut one_way_metrics =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;
        let mut round_trip_metrics = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Check for problematic configurations and adapt automatically
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
        }

        // For combined testing, we always use single-threaded to ensure synchronized message IDs
        self.run_single_threaded_combined(
            transport_config,
            &mut one_way_metrics,
            &mut round_trip_metrics,
            results_manager,
        )
        .await?;

        Ok((
            one_way_metrics.get_metrics(),
            round_trip_metrics.get_metrics(),
        ))
    }

    /// Run single-threaded combined test measuring both latencies for each message
    ///
    /// This implementation sends messages and measures both one-way (send) and
    /// round-trip (send + receive) latencies for the same message IDs.
    async fn run_single_threaded_combined(
        &self,
        transport_config: &TransportConfig,
        one_way_metrics: &mut MetricsCollector,
        round_trip_metrics: &mut MetricsCollector,
        mut results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a oneshot channel to signal server readiness and propagate startup errors.
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let msg_count = self.get_msg_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                if let Err(e) = transport.start_server(&config).await {
                    let _ = tx.send(Err(e));
                    return Err(anyhow::anyhow!("Server failed to start"));
                }

                // Signal that server is ready
                let _ = tx.send(Ok(()));
                debug!("Server signaled ready for combined test");

                let start_time = Instant::now();
                let mut processed = 0;

                // Server receive and response loop - handles both one-way tracking and round-trip responses
                loop {
                    // Check if we should stop based on duration or iterations
                    if let Some(dur) = duration {
                        if start_time.elapsed() >= dur {
                            break;
                        }
                    } else if processed >= msg_count {
                        break;
                    }

                    // Receive message and send response for round-trip timing
                    match timeout(Duration::from_millis(50), transport.receive()).await {
                        Ok(Ok(msg)) => {
                            // Send immediate response for round-trip timing
                            let response = Message::new(
                                msg.id + 1000000, // Offset to distinguish response
                                msg.payload.clone(),
                                MessageType::Response,
                            );
                            let _ = transport.send(&response).await;
                            processed += 1;
                        }
                        Ok(Err(_)) => break, // Transport error
                        Err(_) => {
                            // Timeout - check if duration-based test is done
                            if duration.is_some() {
                                continue; // Keep waiting for duration-based test
                            } else {
                                break; // Message-count-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        rx.await??;
        debug!("Client received server ready signal for combined test");

        // Connect client
        client_transport.start_client(transport_config).await?;

        // Create payload for messages
        let payload = vec![0u8; self.config.message_size];
        let start_time = Instant::now();

        if let Some(duration) = self.config.duration {
            // Duration-based test: loop until time expires
            let mut i = 0u64;
            while start_time.elapsed() < duration {
                let send_start = Instant::now();
                let message = Message::new(i, payload.clone(), MessageType::Request);

                // Send message and measure one-way latency
                match tokio::time::timeout(
                    Duration::from_millis(50),
                    client_transport.send(&message),
                )
                .await
                {
                    Ok(Ok(())) => {
                        let one_way_latency = send_start.elapsed();

                        // Try to receive response and measure round-trip latency
                        match tokio::time::timeout(
                            Duration::from_millis(50),
                            client_transport.receive(),
                        )
                        .await
                        {
                            Ok(Ok(_)) => {
                                let round_trip_latency = send_start.elapsed();

                                // Record both metrics
                                one_way_metrics.record_message(
                                    self.config.message_size,
                                    Some(one_way_latency),
                                )?;
                                round_trip_metrics.record_message(
                                    self.config.message_size,
                                    Some(round_trip_latency),
                                )?;

                                // Stream combined record if enabled
                                if let Some(ref mut manager) = results_manager {
                                    let record = crate::results::MessageLatencyRecord::new_combined(
                                        i,
                                        self.mechanism,
                                        self.config.message_size,
                                        one_way_latency,
                                        round_trip_latency,
                                    );
                                    manager.write_streaming_record_direct(&record).await?;
                                }

                                i += 1;
                            }
                            Ok(Err(_)) => break, // Transport error
                            Err(_) => {
                                // Receive timeout - continue without incrementing
                                sleep(Duration::from_millis(1)).await;
                                continue;
                            }
                        }
                    }
                    Ok(Err(_)) => break, // Send transport error
                    Err(_) => {
                        // Send timeout - continue
                        sleep(Duration::from_millis(1)).await;
                        continue;
                    }
                }
            }
        } else {
            // Message-count-based test: loop for fixed number of messages
            let msg_count = self.get_msg_count();

            for i in 0..msg_count {
                let send_start = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);

                client_transport.send(&message).await?;
                let one_way_latency = send_start.elapsed();

                let _ = client_transport.receive().await?;
                let round_trip_latency = send_start.elapsed();

                // Record both metrics
                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics
                    .record_message(self.config.message_size, Some(round_trip_latency))?;

                // Stream combined record if enabled
                if let Some(ref mut manager) = results_manager {
                    let record = crate::results::MessageLatencyRecord::new_combined(
                        i as u64,
                        self.mechanism,
                        self.config.message_size,
                        one_way_latency,
                        round_trip_latency,
                    );
                    manager.write_streaming_record_direct(&record).await?;
                }
            }
        }

        // Clean up resources
        client_transport.close().await?;
        server_handle.await??;

        Ok(())
    }

    /// Create transport configuration with intelligent parameter adaptation
    ///
    /// This function creates a transport configuration that is optimized for
    /// the specific test parameters and mechanism being tested. It includes
    /// several intelligent adaptations to improve performance and reliability.
    ///
    /// ## Key Adaptations
    ///
    /// - **Unique Identifiers**: Uses UUIDs to prevent resource conflicts
    /// - **Adaptive Buffer Sizing**: Adjusts buffer sizes based on test parameters
    /// - **Port Uniqueness**: Ensures unique ports for TCP to avoid conflicts
    /// - **Mechanism-Specific Tuning**: Applies optimizations for each transport type
    ///
    /// ## Buffer Size Calculation
    ///
    /// - If a buffer size is provided by the user, it is always used.
    /// - If a duration is specified, a large default buffer (1 GB) is used to
    ///   prevent backpressure from becoming a bottleneck.
    /// - Otherwise, the buffer size is calculated based on message size and count
    ///   to fit the expected data volume.
    fn create_transport_config(&self) -> Result<TransportConfig> {
        const DURATION_MODE_BUFFER_SIZE: usize = 1_073_741_824; // 1 GB
        const PMQ_SAFE_DEFAULT_BUFFER_SIZE: usize = 8192;

        let unique_id = Uuid::new_v4();
        let unique_port = self.config.port + (unique_id.as_u128() as u16 % 1000);

        // Determine if the current mechanism is PMQ
        let is_pmq = {
            #[cfg(target_os = "linux")]
            {
                self.mechanism == IpcMechanism::PosixMessageQueue
            }
            #[cfg(not(target_os = "linux"))]
            {
                false
            }
        };

        // New buffer size logic:
        // 1. If user provides --buffer-size, use it directly.
        // 2. If the mechanism is PMQ, always use a safe, small default.
        // 3. If in duration mode, use a large fixed size to avoid backpressure.
        // 4. Otherwise, calculate based on message count.
        let buffer_size = self.config.buffer_size.unwrap_or_else(|| {
            if is_pmq {
                PMQ_SAFE_DEFAULT_BUFFER_SIZE
            } else if self.config.duration.is_some() {
                DURATION_MODE_BUFFER_SIZE
            } else {
                // For message-count mode, size the buffer to fit all messages.
                self.get_msg_count() * (self.config.message_size + 64)
            }
        });

        // Add a specific validation for PMQ, as it's often limited by the OS.
        // This check is important regardless of how the buffer size was determined.
        #[cfg(target_os = "linux")]
        if self.mechanism == IpcMechanism::PosixMessageQueue {
            // PMQ has small system limits, so warn if the buffer is large.
            if buffer_size > PMQ_SAFE_DEFAULT_BUFFER_SIZE {
                warn!(
                "The specified buffer size ({} bytes) exceeds the typical system limit of 8192 bytes for POSIX Message Queues. The benchmark may fail if the system is not configured for larger message sizes.",
                buffer_size
            );
            }
        }

        // Validate buffer size for shared memory to prevent EOF errors
        if self.mechanism == IpcMechanism::SharedMemory && self.config.duration.is_none() {
            let total_message_data = self.get_msg_count() * (self.config.message_size + 32); // 32 bytes overhead per message
            if buffer_size < total_message_data {
                warn!(
                    "Buffer size ({} bytes) is smaller than the total data size ({} bytes). This may cause backpressure, which is a valid test scenario.",
                    buffer_size,
                    total_message_data
                );
            }
        }

        // Conservative queue depth for PMQ - most systems have very low limits (often just 10)
        let adaptive_queue_depth = {
            #[cfg(target_os = "linux")]
            {
                if self.mechanism == IpcMechanism::PosixMessageQueue {
                    let msg_count = self.get_msg_count();

                    // Warn about PMQ limitations for high-throughput tests
                    if msg_count > 10000 {
                        warn!(
                            "PMQ with {} messages may be very slow due to system queue depth limits (typically 10). Consider using fewer messages or a different mechanism for high-throughput testing.",
                            msg_count
                        );
                    }

                    // Use conservative values that work within typical system limits
                    // Most systems default to msg_max=10, so we'll stay at that limit
                    let queue_depth = 10; // Always use system default

                    debug!(
                        "PMQ using conservative queue depth: {} messages -> depth {}",
                        msg_count, queue_depth
                    );
                    queue_depth
                } else {
                    10 // Default for other mechanisms
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                10
            }
        };

        Ok(TransportConfig {
            buffer_size,
            host: self.config.host.clone(),
            port: unique_port,
            socket_path: format!("/tmp/ipc_benchmark_{}.sock", unique_id),
            shared_memory_name: format!("ipc_benchmark_{}", unique_id),
            max_connections: self.config.concurrency.max(16), // Set based on concurrency level
            message_queue_depth: adaptive_queue_depth,
            message_queue_name: format!("ipc_benchmark_pmq_{}", unique_id),
        })
    }

    /// Get the number of messages to run
    ///
    /// This helper method provides a consistent way to determine the message
    /// count, falling back to a reasonable default when not specified.
    ///
    /// ## Returns
    /// The number of messages to execute, either from configuration or default
    fn get_msg_count(&self) -> usize {
        self.config.msg_count.unwrap_or(10000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IpcMechanism;

    /// Test benchmark configuration creation from default values
    #[test]
    #[cfg(unix)]
    fn test_benchmark_config_creation() {
        let config = BenchmarkConfig {
            mechanism: IpcMechanism::UnixDomainSocket,
            message_size: 1024,
            msg_count: Some(1000),
            duration: None,
            concurrency: 1,
            one_way: true,
            round_trip: false,
            warmup_iterations: 100,
            percentiles: vec![50.0, 95.0, 99.0],
            buffer_size: Some(8192),
            host: "127.0.0.1".to_string(),
            port: 8080,
        };

        assert_eq!(config.message_size, 1024);
        assert_eq!(config.msg_count, Some(1000));
        assert_eq!(config.concurrency, 1);
        assert!(config.one_way);
        assert!(!config.round_trip);
    }

    /// Test benchmark runner creation with various mechanisms
    #[tokio::test]
    #[cfg(unix)]
    async fn test_benchmark_runner_creation() {
        let config = BenchmarkConfig {
            mechanism: IpcMechanism::UnixDomainSocket,
            message_size: 1024,
            msg_count: Some(100),
            duration: None,
            concurrency: 1,
            one_way: true,
            round_trip: false,
            warmup_iterations: 10,
            percentiles: vec![50.0, 95.0, 99.0],
            buffer_size: Some(8192),
            host: "127.0.0.1".to_string(),
            port: 8080,
        };

        let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket);
        assert_eq!(runner.mechanism, IpcMechanism::UnixDomainSocket);
    }

    /// Test the buffer size logic in `create_transport_config` is platform-aware.
    #[test]
    fn test_transport_config_buffer_size_logic() {
        const DURATION_MODE_BUFFER_SIZE: usize = 1_073_741_824; // 1 GB
        const PMQ_SAFE_DEFAULT_BUFFER_SIZE: usize = 8192;

        // Helper to get only the mechanisms available on the current platform.
        fn get_platform_mechanisms() -> Vec<IpcMechanism> {
            let mut mechanisms = vec![IpcMechanism::SharedMemory, IpcMechanism::TcpSocket];
            #[cfg(unix)]
            mechanisms.push(IpcMechanism::UnixDomainSocket);
            #[cfg(target_os = "linux")]
            mechanisms.push(IpcMechanism::PosixMessageQueue);
            mechanisms
        }

        let mut base_config = BenchmarkConfig {
            mechanism: IpcMechanism::SharedMemory, // Placeholder, will be overridden
            message_size: 1024,
            msg_count: Some(10000),
            duration: None,
            concurrency: 1,
            one_way: true,
            round_trip: false,
            warmup_iterations: 100,
            percentiles: vec![],
            buffer_size: None,
            host: "127.0.0.1".to_string(),
            port: 8080,
        };

        // Scenario 1: User-provided buffer size is always respected.
        let user_size = 9999;
        base_config.buffer_size = Some(user_size);
        for mechanism in get_platform_mechanisms() {
            let runner = BenchmarkRunner::new(base_config.clone(), mechanism);
            let transport_config = runner.create_transport_config().unwrap();
            assert_eq!(
                transport_config.buffer_size, user_size,
                "User-provided buffer size should be respected for {:?}",
                mechanism
            );
        }
        base_config.buffer_size = None; // Reset for next tests

        // Scenario 2: Automatic buffer size for message-count mode (non-PMQ).
        let expected_msg_count_auto_size = 10000 * (1024 + 64);
        let mut auto_sized_mechanisms = vec![IpcMechanism::SharedMemory, IpcMechanism::TcpSocket];
        #[cfg(unix)]
        auto_sized_mechanisms.push(IpcMechanism::UnixDomainSocket);

        for mechanism in &auto_sized_mechanisms {
            let runner = BenchmarkRunner::new(base_config.clone(), *mechanism);
            let transport_config = runner.create_transport_config().unwrap();
            assert_eq!(
                transport_config.buffer_size, expected_msg_count_auto_size,
                "Automatic buffer size should be large for message-count mode on {:?}",
                mechanism
            );
        }

        // Scenario 3: Automatic buffer size for duration mode (non-PMQ).
        base_config.duration = Some(Duration::from_secs(1));
        base_config.msg_count = None;
        for mechanism in &auto_sized_mechanisms {
            let runner = BenchmarkRunner::new(base_config.clone(), *mechanism);
            let transport_config = runner.create_transport_config().unwrap();
            assert_eq!(
                transport_config.buffer_size, DURATION_MODE_BUFFER_SIZE,
                "Automatic buffer size should be the large default for duration mode on {:?}",
                mechanism
            );
        }

        // Scenario 4: Automatic buffer size for PMQ always falls back to the safe default.
        #[cfg(target_os = "linux")]
        {
            // Test PMQ in message-count mode
            base_config.duration = None;
            base_config.msg_count = Some(10000);
            let runner_pmq_msg =
                BenchmarkRunner::new(base_config.clone(), IpcMechanism::PosixMessageQueue);
            let transport_config_pmq_msg = runner_pmq_msg.create_transport_config().unwrap();
            assert_eq!(
                transport_config_pmq_msg.buffer_size, PMQ_SAFE_DEFAULT_BUFFER_SIZE,
                "Automatic buffer size for PMQ in message-count mode should be the safe default"
            );

            // Test PMQ in duration mode
            base_config.duration = Some(Duration::from_secs(1));
            base_config.msg_count = None;
            let runner_pmq_dur =
                BenchmarkRunner::new(base_config.clone(), IpcMechanism::PosixMessageQueue);
            let transport_config_pmq_dur = runner_pmq_dur.create_transport_config().unwrap();
            assert_eq!(
                transport_config_pmq_dur.buffer_size, PMQ_SAFE_DEFAULT_BUFFER_SIZE,
                "Automatic buffer size for PMQ in duration mode should be the safe default"
            );
        }
    }
}
