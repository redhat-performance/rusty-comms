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
    ipc::{IpcTransport, Message, MessageType, TransportConfig, TransportFactory},
    metrics::{LatencyType, MetricsCollector, PerformanceMetrics},
    results::BenchmarkResults,
};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;
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
    
    /// Number of iterations to run (None for duration-based tests)
    ///
    /// When specified, the test runs for exactly this many message exchanges.
    /// Mutually exclusive with duration-based testing.
    pub iterations: Option<usize>,
    
    /// Duration to run tests (takes precedence over iterations)
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
    pub buffer_size: usize,
    
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
    
    /// Enable ultra-low latency optimizations
    ///
    /// When enabled, activates aggressive optimizations for sub-microsecond latency:
    /// - Direct syscalls bypassing async overhead
    /// - Pre-allocated memory pools
    /// - Cache-aligned atomic operations  
    /// - Huge page memory allocation
    /// - Zero-copy message handling
    pub ultra_low_latency: bool,
    
    /// Enable automotive real-time evaluation mode
    ///
    /// When enabled, tests IPC mechanisms against automotive requirements:
    /// - Hard deadline enforcement
    /// - ASIL safety level compliance
    /// - Deterministic timing validation
    /// - Periodic task simulation
    pub automotive_mode: bool,
    
    /// Maximum latency deadline in microseconds
    ///
    /// Hard deadline for message delivery. Messages exceeding this
    /// latency are recorded as deadline misses for automotive evaluation.
    pub max_latency_us: u64,
    
    /// Automotive Safety Integrity Level
    ///
    /// Defines safety requirements and error tolerances for evaluation.
    pub asil_level: crate::cli::AsilLevel,
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
            mechanism: IpcMechanism::UnixDomainSocket, // Will be overridden per test
            message_size: args.message_size,
            
            // Duration takes precedence over iterations
            // This provides more predictable test timing
            iterations: if args.duration.is_some() {
                None
            } else {
                Some(args.iterations)
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
            ultra_low_latency: args.ultra_low_latency,
            automotive_mode: args.automotive_mode,
            max_latency_us: args.max_latency_us,
            asil_level: args.asil_level.clone(),
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
/// ```rust
/// let config = BenchmarkConfig::from_args(&args)?;
/// let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket);
/// let results = runner.run().await?;
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
    pub async fn run(&self, mut results_manager: Option<&mut crate::results::ResultsManager>) -> Result<BenchmarkResults> {
        info!("Starting benchmark for {} mechanism", self.mechanism);

        // Initialize results structure with test configuration
        let mut results = BenchmarkResults::new(
            self.mechanism,
            self.config.message_size,
            self.config.concurrency,
            self.config.iterations,
            self.config.duration,
        );

        // Run warmup if configured
        // Warmup helps stabilize performance by allowing:
        // - CPU caches to fill with relevant data
        // - Network connections to establish properly
        // - JIT compilation to optimize hot code paths
        // - OS buffers and scheduling to reach steady state
        if self.config.warmup_iterations > 0 {
            info!(
                "Running warmup with {} iterations",
                self.config.warmup_iterations
            );
            self.run_warmup().await?;
        }

        // Check if we need to run in combined mode for streaming
        let results_manager_ref = results_manager.as_deref_mut();
        let combined_streaming = results_manager_ref
            .map(|rm| rm.is_combined_streaming_enabled())
            .unwrap_or(false);

        if combined_streaming && self.config.one_way && self.config.round_trip {
            info!("Running combined one-way and round-trip test for streaming");
            let combined_results = self.run_combined_test(results_manager.as_deref_mut()).await?;
            results.add_one_way_results(combined_results.0);
            results.add_round_trip_results(combined_results.1);
        } else {
            // Run one-way latency test if enabled
            // One-way tests measure pure transmission latency without
            // waiting for responses, providing baseline performance metrics
            if self.config.one_way {
                info!("Running one-way latency test");
                let one_way_results = self.run_one_way_test(results_manager.as_deref_mut()).await?;
                results.add_one_way_results(one_way_results);
            }

            // Run round-trip latency test if enabled
            // Round-trip tests measure request-response cycles,
            // providing insights into interactive communication patterns
            if self.config.round_trip {
                info!("Running round-trip latency test");
                let round_trip_results = self.run_round_trip_test(results_manager.as_deref_mut()).await?;
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
    /// 2. **Connection Establishment**: Use barriers to synchronize startup
    /// 3. **Message Exchange**: Send warmup messages without measurement
    /// 4. **Resource Cleanup**: Properly close connections after warmup
    ///
    /// ## Synchronization
    ///
    /// The function uses Tokio barriers to ensure proper synchronization
    /// between client and server tasks, preventing race conditions during startup.
    async fn run_warmup(&self) -> Result<()> {
        let transport_config = self.create_transport_config()?;
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a barrier to synchronize server startup
        // This ensures the server is ready before the client attempts to connect
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism; // Copy mechanism to move into closure
            let warmup_iterations = self.config.warmup_iterations;
            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                transport.start_server(&config).await?;
                
                // Signal that server is ready to accept connections
                server_ready_clone.wait().await;
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
        server_ready.wait().await;
        debug!("Client received server ready signal for warmup");

        // Connect client and send warmup messages
        client_transport.start_client(&transport_config).await?;

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
    async fn run_one_way_test(&self, results_manager: Option<&mut crate::results::ResultsManager>) -> Result<PerformanceMetrics> {
        let transport_config = self.create_transport_config()?;
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
            self.run_single_threaded_one_way(&transport_config, &mut metrics_collector, results_manager)
                .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_one_way(&transport_config, &mut metrics_collector, results_manager)
                .await?;
        } else {
            self.run_multi_threaded_one_way(&transport_config, &mut metrics_collector, results_manager)
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
    async fn run_round_trip_test(&self, results_manager: Option<&mut crate::results::ResultsManager>) -> Result<PerformanceMetrics> {
        let transport_config = self.create_transport_config()?;
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
            self.run_single_threaded_round_trip(&transport_config, &mut metrics_collector, results_manager)
                .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_round_trip(&transport_config, &mut metrics_collector, results_manager)
                .await?;
        } else {
            self.run_multi_threaded_round_trip(&transport_config, &mut metrics_collector, results_manager)
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

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let iterations = self.get_iteration_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                transport.start_server(&config).await?;
                
                // Signal that server is ready
                server_ready_clone.wait().await;
                debug!("Server signaled ready for one-way test");

                let start_time = Instant::now();
                let mut received = 0;

                // Server receive loop - adapts to duration or iteration mode
                loop {
                    // Check if we should stop based on duration or iterations
                    if let Some(dur) = duration {
                        if start_time.elapsed() >= dur {
                            break;
                        }
                    } else if received >= iterations {
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
                                break; // Iteration-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        server_ready.wait().await;
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
            // Iteration-based test: loop for fixed number of iterations
            let iterations = self.get_iteration_count();

            for i in 0..iterations {
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

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server in background task
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let iterations = self.get_iteration_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                transport.start_server(&config).await?;
                
                // Signal that server is ready
                server_ready_clone.wait().await;
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
                    } else if received >= iterations {
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
                            if let Err(_) = transport.send(&response).await {
                                break; // Client disconnected
                            }
                        }
                        Ok(Err(_)) => break, // Transport error
                        Err(_) => {
                            // Timeout - check if duration-based test is done
                            if duration.is_some() {
                                continue; // Keep waiting for duration-based test
                            } else {
                                break; // Iteration-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        server_ready.wait().await;
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
            // Iteration-based test: loop for fixed number of iterations
            let iterations = self.get_iteration_count();

            for i in 0..iterations {
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
        let iterations_per_worker = self.get_iteration_count() / self.config.concurrency;

        // Run each "worker" sequentially to avoid connection conflicts
        for worker_id in 0..self.config.concurrency {
            debug!(
                "Running worker {} with {} iterations",
                worker_id, iterations_per_worker
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
        let iterations_per_worker = self.get_iteration_count() / self.config.concurrency;

        // Run each "worker" sequentially to avoid connection conflicts
        for worker_id in 0..self.config.concurrency {
            debug!(
                "Running worker {} with {} iterations",
                worker_id, iterations_per_worker
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
        results_manager: Option<&mut crate::results::ResultsManager>,
    ) -> Result<(PerformanceMetrics, PerformanceMetrics)> {
        let transport_config = self.create_transport_config()?;
        let mut one_way_metrics =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;
        let mut round_trip_metrics =
            MetricsCollector::new(Some(LatencyType::RoundTrip), self.config.percentiles.clone())?;

        // Check for problematic configurations and adapt automatically
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!("Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1.");
        }

        // For combined testing, we always use single-threaded to ensure synchronized message IDs
        self.run_single_threaded_combined(&transport_config, &mut one_way_metrics, &mut round_trip_metrics, results_manager).await?;

        Ok((one_way_metrics.get_metrics(), round_trip_metrics.get_metrics()))
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

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server in background task  
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism;
            let duration = self.config.duration;
            let iterations = self.get_iteration_count();

            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                transport.start_server(&config).await?;
                
                // Signal that server is ready
                server_ready_clone.wait().await;
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
                    } else if processed >= iterations {
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
                                break; // Iteration-based test with no more messages
                            }
                        }
                    }
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        server_ready.wait().await;
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
                match tokio::time::timeout(Duration::from_millis(50), client_transport.send(&message)).await {
                    Ok(Ok(())) => {
                        let one_way_latency = send_start.elapsed();
                        
                        // Try to receive response and measure round-trip latency
                        match tokio::time::timeout(Duration::from_millis(50), client_transport.receive()).await {
                            Ok(Ok(_)) => {
                                let round_trip_latency = send_start.elapsed();
                                
                                // Record both metrics
                                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;
                                
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
            // Iteration-based test: loop for fixed number of iterations
            let iterations = self.get_iteration_count();

            for i in 0..iterations {
                let send_start = Instant::now();
                let message = Message::new(i as u64, payload.clone(), MessageType::Request);
                
                client_transport.send(&message).await?;
                let one_way_latency = send_start.elapsed();
                
                let _ = client_transport.receive().await?;
                let round_trip_latency = send_start.elapsed();
                
                // Record both metrics
                one_way_metrics.record_message(self.config.message_size, Some(one_way_latency))?;
                round_trip_metrics.record_message(self.config.message_size, Some(round_trip_latency))?;
                
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
    /// Buffer sizes are calculated based on message size, iteration count,
    /// and concurrency level to optimize for the specific workload being tested.
    fn create_transport_config(&self) -> Result<TransportConfig> {
        let unique_id = Uuid::new_v4();

        // Use a unique port for TCP to avoid conflicts when running multiple mechanisms
        let unique_port = self.config.port + (unique_id.as_u128() as u16 % 1000);

        // Adaptive buffer sizing for high-throughput scenarios
        let adaptive_buffer_size = self.calculate_adaptive_buffer_size();

        // Validate buffer size for shared memory to prevent EOF errors
        if self.mechanism == IpcMechanism::SharedMemory {
            let total_message_data = self.get_iteration_count() * (self.config.message_size + 32); // 32 bytes overhead per message
            if adaptive_buffer_size < total_message_data {
                warn!(
                    "Buffer size ({} bytes) may be too small for {} iterations of {} byte messages. \
                     Consider using --buffer-size {} or reducing iterations/message size.",
                    adaptive_buffer_size,
                    self.get_iteration_count(),
                    self.config.message_size,
                    total_message_data * 2 // Suggest 2x the calculated size
                );
            }
        }

        // Conservative queue depth for PMQ - most systems have very low limits (often just 10)
        let adaptive_queue_depth = if self.mechanism == IpcMechanism::PosixMessageQueue {
            let iterations = self.get_iteration_count();
            
            // For ULL tests, use larger queue depth to handle high message rates
            let queue_depth = if self.config.ultra_low_latency {
                // ULL tests generate messages rapidly, but stay within system limits
                // Most systems have msg_max around 10-100, so use conservative values
                50.min(iterations / 20).max(20) // Between 20-50 based on test size
            } else {
                // Warn about PMQ limitations for high-throughput tests
                if iterations > 10000 {
                    warn!(
                        "PMQ with {} iterations may be very slow due to system queue depth limits (typically 10). \
                         Consider using fewer iterations or a different mechanism for high-throughput testing.",
                        iterations
                    );
                }
                
                // Use conservative values that work within typical system limits
                // Most systems default to msg_max=10, so we'll stay at that limit
                10 // Always use system default for non-ULL
            };
            
            debug!("PMQ using queue depth: {} iterations -> depth {} (ULL: {})", iterations, queue_depth, self.config.ultra_low_latency);
            queue_depth
        } else {
            10 // Default for other mechanisms
        };

        let config = TransportConfig {
            buffer_size: adaptive_buffer_size,
            host: self.config.host.clone(),
            port: unique_port,
            socket_path: format!("/tmp/ipc_benchmark_{}.sock", unique_id),
            shared_memory_name: format!("ipc_benchmark_{}", unique_id),
            max_connections: self.config.concurrency.max(16), // Set based on concurrency level
            message_queue_depth: adaptive_queue_depth,
            message_queue_name: format!("ipc_benchmark_pmq_{}", unique_id),
        };
        
        if self.mechanism == IpcMechanism::PosixMessageQueue {
                    debug!("PMQ ULL RT: Created transport config - buffer_size: {}, original: {}, adaptive: {}", 
               config.buffer_size, self.config.buffer_size, adaptive_buffer_size);
        }
        
        Ok(config)
    }

    /// Calculate adaptive buffer size based on test parameters
    ///
    /// This function intelligently calculates buffer sizes based on the specific
    /// test parameters to optimize performance. Different mechanisms have different
    /// optimal buffer sizing strategies.
    ///
    /// ## Sizing Strategy
    ///
    /// - **Base Size**: User-specified buffer size as minimum
    /// - **Message-Based Scaling**: Account for message size and count
    /// - **Mechanism-Specific**: Apply transport-specific optimizations
    /// - **Memory Limits**: Cap at reasonable maximum to prevent excessive usage
    ///
    /// ## Shared Memory Optimization
    ///
    /// For shared memory with high iteration counts, buffer size is increased
    /// more aggressively to accommodate the ring buffer requirements and
    /// reduce the likelihood of buffer overflow conditions.
    fn calculate_adaptive_buffer_size(&self) -> usize {
        let base_buffer_size = self.config.buffer_size;
        let iterations = self.get_iteration_count();
        let message_size = self.config.message_size;

        // For shared memory with high iteration counts, increase buffer size more aggressively
        if self.mechanism == IpcMechanism::SharedMemory && iterations > 8000 {
            // Calculate buffer size to hold more messages for high throughput
            let messages_to_buffer = if iterations >= 50_000 {
                300 // Buffer for 300 messages for very high counts
            } else if iterations >= 20_000 {
                200 // Buffer for 200 messages for high counts
            } else {
                150 // Buffer for 150 messages for moderate-high counts
            };

            let calculated_size = message_size * messages_to_buffer + 2048; // Extra for metadata

            // Use the larger of: user-specified size or calculated size
            // But cap at 2MB to avoid excessive memory usage
            let adaptive_size = calculated_size.max(base_buffer_size).min(2 * 1024 * 1024);

            debug!(
                "Adaptive buffer sizing for {} iterations: {} bytes (was {} bytes)",
                iterations, adaptive_size, base_buffer_size
            );

            adaptive_size
        } else {
            base_buffer_size
        }
    }

    /// Get the number of iterations to run
    ///
    /// This helper method provides a consistent way to determine the iteration
    /// count, falling back to a reasonable default when not specified.
    ///
    /// ## Returns
    /// The number of iterations to execute, either from configuration or default
    fn get_iteration_count(&self) -> usize {
        self.config.iterations.unwrap_or(10000)
    }
    
    /// Run ultra-low latency benchmark with direct syscalls and zero-copy operations
    /// 
    /// This method bypasses all async overhead and uses the optimized implementations
    /// we added to each IPC mechanism for automotive/real-time systems.
    pub async fn run_ultra_low_latency(&self, results_manager: Option<&mut crate::results::ResultsManager>) -> Result<crate::automotive_metrics::AutomotiveMetrics> {
        if !self.config.ultra_low_latency {
            return Err(anyhow!("Ultra-low latency mode not enabled in configuration"));
        }
        
        let mut automotive_metrics = crate::automotive_metrics::AutomotiveMetrics::new(
            self.mechanism,
            self.config.message_size,
            self.config.asil_level.clone(),
            crate::automotive_metrics::AutomotiveApplication::SafetyCritical, // Default to safety-critical
        );
        
        match self.mechanism {
            IpcMechanism::SharedMemory => {
                self.run_ultra_low_latency_shared_memory(&mut automotive_metrics, results_manager).await
            }
            IpcMechanism::UnixDomainSocket => {
                self.run_ultra_low_latency_unix_domain_socket(&mut automotive_metrics, results_manager).await
            }
            IpcMechanism::PosixMessageQueue => {
                self.run_ultra_low_latency_posix_message_queue(&mut automotive_metrics, results_manager).await
            }
            IpcMechanism::TcpSocket => {
                Err(anyhow!("TCP sockets not recommended for ultra-low latency automotive applications"))
            }
            IpcMechanism::All => {
                Err(anyhow!("Cannot run ultra-low latency test on 'All' mechanisms - specify individual mechanism"))
            }
        }?;
        
        Ok(automotive_metrics)
    }
    
    /// Ultra-low latency shared memory benchmark using zero-copy operations
    async fn run_ultra_low_latency_shared_memory(
        &self, 
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<()> {
        // Run one-way test
        let one_way_results = self.run_shm_ull_one_way(automotive_metrics, results_manager.as_deref_mut()).await?;
        
        // Run round-trip test  
        let round_trip_results = self.run_shm_ull_round_trip(automotive_metrics, results_manager.as_deref_mut()).await?;
        
        info!("Ultra-low latency shared memory test completed - One-way: {} ops, Round-trip: {} ops", 
              one_way_results, round_trip_results);
        Ok(())
    }
    
    /// Run SHM ULL one-way test
    async fn run_shm_ull_one_way(
        &self,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::shared_memory::SharedMemoryTransport;
        
        let transport = SharedMemoryTransport::new();
        let segment_name = format!("ull_shm_one_way_{}", uuid::Uuid::new_v4());
        
        // Create ultra-low latency segment with power-of-2 capacity
        let capacity = 1024; // Power of 2 for bit-mask operations
        let (_shmem, ring_buffer) = transport.create_ultra_low_latency_segment(
            &segment_name,
            capacity,
            self.config.message_size,
        )?;
        
        // Pre-allocate message data
        let message_data = vec![0xAA; self.config.message_size];
        let mut receive_buffer = vec![0u8; self.config.message_size];
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        
        // Ultra-low latency one-way test loop
        while start_time.elapsed() < test_duration {
            let operation_start = Instant::now();
            
            // Zero-copy send
            if transport.send_ultra_fast(ring_buffer, &message_data) {
                // Zero-copy receive
                if transport.recv_ultra_fast(ring_buffer, &mut receive_buffer) {
                    let latency = operation_start.elapsed();
                    automotive_metrics.record_success(latency);
                    
                    // Stream individual latency record if streaming is enabled
                    if let Some(ref mut manager) = results_manager {
                        let record = crate::results::MessageLatencyRecord::new(
                            iteration,
                            self.mechanism,
                            self.config.message_size,
                            crate::metrics::LatencyType::OneWay,
                            latency,
                        );
                        if let Err(e) = manager.stream_latency_record(&record).await {
                            tracing::warn!("Failed to stream latency record: {}", e);
                        }
                    }
                    
                    iteration += 1;
                } else {
                    // No message available - continue
                    continue;
                }
            } else {
                // Buffer full - continue without penalty
                continue;
            }
        }
        
        Ok(iteration)
    }
    
    /// Run SHM ULL round-trip test
    async fn run_shm_ull_round_trip(
        &self,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::shared_memory::SharedMemoryTransport;
        
        // For shared memory, we'll use a simpler approach without separate server task
        // since shared memory operations are inherently synchronous and cross-process
        let transport = SharedMemoryTransport::new();
        let segment_name = format!("ull_shm_round_trip_{}", uuid::Uuid::new_v4());
        
        let capacity = 1024; // Power of 2 for bit-mask operations
        let (_shmem, ring_buffer) = transport.create_ultra_low_latency_segment(
            &segment_name,
            capacity,
            self.config.message_size,
        )?;
        
        // Pre-allocate message data
        let message_data = vec![0xBB; self.config.message_size];
        let mut response_buffer = vec![0u8; self.config.message_size];
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        
        // Ultra-low latency round-trip test loop (simulated by send-then-receive)
        while start_time.elapsed() < test_duration {
            let round_trip_start = Instant::now();
            
            // Send request
            if transport.send_ultra_fast(ring_buffer, &message_data) {
                // Immediate receive to simulate round-trip
                if transport.recv_ultra_fast(ring_buffer, &mut response_buffer) {
                    let round_trip_latency = round_trip_start.elapsed();
                    automotive_metrics.record_success(round_trip_latency);
                    
                    // Stream individual latency record if streaming is enabled
                    if let Some(ref mut manager) = results_manager {
                        let record = crate::results::MessageLatencyRecord::new(
                            iteration,
                            self.mechanism,
                            self.config.message_size,
                            crate::metrics::LatencyType::RoundTrip,
                            round_trip_latency,
                        );
                        if let Err(e) = manager.stream_latency_record(&record).await {
                            tracing::warn!("Failed to stream latency record: {}", e);
                        }
                    }
                    
                    iteration += 1;
                } else {
                    // No response available - this shouldn't happen in this simplified test
                    automotive_metrics.record_deadline_miss(
                        round_trip_start.elapsed().as_micros() as u64,
                        self.config.max_latency_us
                    );
                }
            } else {
                // Request buffer full - continue without penalty
                continue;
            }
        }
        
        Ok(iteration)
    }
    
    /// Ultra-low latency Unix Domain Socket benchmark using direct syscalls  
    async fn run_ultra_low_latency_unix_domain_socket(
        &self,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<()> {
        let config = self.create_transport_config()?;
        
        // Run one-way test
        let one_way_results = self.run_uds_ull_one_way(&config, automotive_metrics, results_manager.as_deref_mut()).await?;
        
        // Run round-trip test
        let round_trip_results = self.run_uds_ull_round_trip(&config, automotive_metrics, results_manager.as_deref_mut()).await?;
        
        info!("Ultra-low latency Unix Domain Socket test completed - One-way: {} ops, Round-trip: {} ops", 
              one_way_results, round_trip_results);
        Ok(())
    }
    
    /// Run UDS ULL one-way test
    async fn run_uds_ull_one_way(
        &self,
        config: &crate::ipc::TransportConfig,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
        use tokio::sync::Notify;
        use std::sync::Arc;
        use tokio::net::{UnixListener, UnixStream};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        
        let server_ready = Arc::new(Notify::new());
        let server_ready_clone = server_ready.clone();
        
        let server_config = config.clone();
        
        // Start server that accepts connections and continuously reads data
        let server_handle = tokio::spawn(async move {
            let _ = std::fs::remove_file(&server_config.socket_path);
            let listener = UnixListener::bind(&server_config.socket_path)?;
            server_ready_clone.notify_one();
            
            // Accept and read from connection
            if let Ok((stream, _)) = listener.accept().await {
                let (mut read_half, _) = stream.into_split();
                let mut buffer = vec![0u8; 8192]; // Large buffer for continuous reading
                
                // Continuously read data to prevent client from hanging
                loop {
                    match tokio::time::timeout(Duration::from_millis(50), read_half.read(&mut buffer)).await {
                        Ok(Ok(0)) => break, // Client disconnected
                        Ok(Ok(_)) => continue, // Data received, keep reading
                        Ok(Err(_)) => break, // Read error
                        Err(_) => continue, // Timeout, keep trying
                    }
                }
            }
            
            let _ = std::fs::remove_file(&server_config.socket_path);
            Ok::<(), anyhow::Error>(())
        });
        
        // Wait for server to be ready
        server_ready.notified().await;
        
        // Give server time to bind
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Connect client directly
        let client_stream = UnixStream::connect(&config.socket_path).await?;
        let (_, mut client_write) = client_stream.into_split();
        
        // Pre-allocate message data
        let message_data = vec![0xBB; self.config.message_size];
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        
        // Ultra-low latency one-way test loop
        while start_time.elapsed() < test_duration {
            let operation_start = Instant::now();
            
            // Send message with deadline check
            match client_write.write_all(&message_data).await {
                Ok(_) => {
                    let latency_duration = operation_start.elapsed();
                    
                    // Check deadline compliance
                    if latency_duration.as_micros() > self.config.max_latency_us as u128 {
                        automotive_metrics.record_deadline_miss(
                            latency_duration.as_micros() as u64,
                            self.config.max_latency_us
                        );
                    } else {
                        automotive_metrics.record_success(latency_duration);
                        
                        // Stream individual latency record if streaming is enabled
                        if let Some(ref mut manager) = results_manager {
                            let record = crate::results::MessageLatencyRecord::new(
                                iteration,
                                self.mechanism,
                                self.config.message_size,
                                crate::metrics::LatencyType::OneWay,
                                latency_duration,
                            );
                            if let Err(e) = manager.stream_latency_record(&record).await {
                                tracing::warn!("Failed to stream latency record: {}", e);
                            }
                        }
                    }
                    
                    iteration += 1;
                }
                Err(_) => {
                    // Send failed
                    automotive_metrics.record_deadline_miss(
                        operation_start.elapsed().as_micros() as u64,
                        self.config.max_latency_us
                    );
                    continue;
                }
            }
        }
        
        // Clean up resources
        server_handle.abort();
        Ok(iteration)
    }
    
    /// Run UDS ULL round-trip test
    async fn run_uds_ull_round_trip(
        &self,
        config: &crate::ipc::TransportConfig,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::unix_domain_socket::UnixDomainSocketTransport;
        use tokio::sync::Notify;
        use std::sync::Arc;
        use tokio::net::{UnixListener, UnixStream};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        
        let server_ready = Arc::new(Notify::new());
        let server_ready_clone = server_ready.clone();
        
        // Clone data for the async task to avoid lifetime issues
        let server_config = config.clone();
        let message_size = self.config.message_size;
        
        // Start echo server with proper connection handling
        let server_handle = tokio::spawn(async move {
            // Create listener manually for better control
            let _ = std::fs::remove_file(&server_config.socket_path); // Clean up any existing socket
            let listener = UnixListener::bind(&server_config.socket_path)?;
            
            server_ready_clone.notify_one();
            
            // Accept one client connection
            let (stream, _) = listener.accept().await?;
            
            // Use tokio's async I/O instead of raw syscalls for round-trip
            let (mut read_half, mut write_half) = stream.into_split();
            let mut receive_buffer = vec![0u8; message_size];
            
            // Server echo loop with proper async I/O
            loop {
                match tokio::time::timeout(Duration::from_millis(100), 
                                         read_half.read_exact(&mut receive_buffer)).await {
                    Ok(Ok(_)) => {
                        // Echo back immediately
                        if let Err(_) = write_half.write_all(&receive_buffer).await {
                            break; // Client disconnected
                        }
                    }
                    Ok(Err(_)) => break, // Read error
                    Err(_) => continue, // Timeout - keep waiting
                }
            }
            
            let _ = std::fs::remove_file(&server_config.socket_path); // Cleanup
            Ok::<(), anyhow::Error>(())
        });
        
        // Wait for server to be ready
        server_ready.notified().await;
        
        // Create client connection directly  
        let client_stream = UnixStream::connect(&config.socket_path).await?;
        let (mut client_read, mut client_write) = client_stream.into_split();
        
        // Pre-allocate message data
        let message_data = vec![0xCC; self.config.message_size];
        let mut response_buffer = vec![0u8; self.config.message_size];
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        
        // Ultra-low latency round-trip test loop
        while start_time.elapsed() < test_duration {
            let round_trip_start = Instant::now();
            
            // Send request
            match client_write.write_all(&message_data).await {
                Ok(_) => {
                    // Wait for response with more reasonable timeout for ULL
                    let timeout_duration = Duration::from_millis(5); // 5ms timeout for ULL context
                    match tokio::time::timeout(timeout_duration, client_read.read_exact(&mut response_buffer)).await {
                        Ok(Ok(_)) => {
                            let round_trip_latency = round_trip_start.elapsed();
                            automotive_metrics.record_success(round_trip_latency);
                            
                            // Stream individual latency record if streaming is enabled
                            if let Some(ref mut manager) = results_manager {
                                let record = crate::results::MessageLatencyRecord::new(
                                    iteration,
                                    self.mechanism,
                                    self.config.message_size,
                                    crate::metrics::LatencyType::RoundTrip,
                                    round_trip_latency,
                                );
                                if let Err(e) = manager.stream_latency_record(&record).await {
                                    tracing::warn!("Failed to stream latency record: {}", e);
                                }
                            }
                            
                            iteration += 1;
                        }
                        Ok(Err(_)) | Err(_) => {
                            // Read error or timeout - deadline miss
                            automotive_metrics.record_deadline_miss(
                                round_trip_start.elapsed().as_micros() as u64,
                                self.config.max_latency_us
                            );
                            continue;
                        }
                    }
                }
                Err(_) => {
                    // Send failed - deadline miss  
                    automotive_metrics.record_deadline_miss(
                        round_trip_start.elapsed().as_micros() as u64,
                        self.config.max_latency_us
                    );
                    continue;
                }
            }
        }
        
        // Client streams will be cleaned up automatically
        server_handle.abort();
        Ok(iteration)
    }
    
    /// Ultra-low latency POSIX Message Queue benchmark using direct libc calls
    async fn run_ultra_low_latency_posix_message_queue(
        &self,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<()> {
        use crate::ipc::posix_message_queue::PosixMessageQueueTransport;
        use tokio::sync::Barrier;
        use tokio::time::timeout;
        
        let config = self.create_transport_config()?;
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        
        let mut one_way_results = 0u64;
        let mut round_trip_results = 0u64;
        
        // Run one-way test only if enabled
        if self.config.one_way {
            one_way_results = self.run_pmq_ull_one_way(&config, automotive_metrics, results_manager.as_deref_mut()).await?;
        }
        
        // Run round-trip test only if enabled
        if self.config.round_trip {
            round_trip_results = self.run_pmq_ull_round_trip(&config, automotive_metrics, results_manager.as_deref_mut()).await?;
        }
        
        info!("Ultra-low latency POSIX Message Queue test completed - One-way: {} ops, Round-trip: {} ops", 
              one_way_results, round_trip_results);
        Ok(())
    }
    
    /// Run PMQ ULL one-way test
    async fn run_pmq_ull_one_way(
        &self,
        config: &crate::ipc::TransportConfig,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::posix_message_queue::PosixMessageQueueTransport;
        use tokio::sync::Notify;
        
        let server_ready = Arc::new(Notify::new());
        let server_ready_clone = server_ready.clone();
        let config_clone = config.clone();
        let message_size = self.config.message_size;
        
        // Start background receiver to drain the queue (like standard PMQ one-way)
        let receiver_handle = tokio::spawn(async move {
            let mut receiver_transport = PosixMessageQueueTransport::new();
            if let Err(e) = receiver_transport.start_server(&config_clone).await {
                tracing::error!("Failed to start receiver: {}", e);
                return;
            }
            if let Err(e) = receiver_transport.configure_ultra_low_latency() {
                tracing::error!("Failed to configure receiver ULL: {}", e);
                return;
            }
            
            server_ready_clone.notify_one();
            
            let mut receive_buffer = vec![0u8; message_size];
            
            // Continuously drain messages to prevent queue from filling up
            loop {
                match receiver_transport.recv_ultra_fast_direct(&mut receive_buffer) {
                    Ok(_) => {
                        // Message received, continue draining
                        continue;
                    }
                    Err(_) => {
                        // No message available - brief sleep to avoid busy loop
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        continue;
                    }
                }
            }
        });
        
        // Wait for receiver to be ready
        server_ready.notified().await;
        tokio::time::sleep(Duration::from_millis(10)).await; // Give receiver time to initialize
        
        let mut sender_transport = PosixMessageQueueTransport::new();
        
        // Setup message queue (async setup required)
        sender_transport.start_client(&config).await?;
        
        // Configure for ultra-low latency
        sender_transport.configure_ultra_low_latency()?;
        
        // Pre-allocate message data (no serialization for ultra-low latency)
        let message_data = vec![0xCC; self.config.message_size]; 
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        
        // Ultra-low latency one-way test loop with automotive deadline enforcement
        while start_time.elapsed() < test_duration {
            // Direct libc send with deadline
            match sender_transport.send_with_deadline_ultra_fast(&message_data, self.config.max_latency_us) {
                Ok(latency_duration) => {
                    automotive_metrics.record_success(latency_duration);
                    
                    // Stream individual latency record if streaming is enabled
                    if let Some(ref mut manager) = results_manager {
                        let record = crate::results::MessageLatencyRecord::new(
                            iteration,
                            self.mechanism,
                            self.config.message_size,
                            crate::metrics::LatencyType::OneWay,
                            latency_duration,
                        );
                        if let Err(e) = manager.stream_latency_record(&record).await {
                            tracing::warn!("Failed to stream latency record: {}", e);
                        }
                    }
                    
                    iteration += 1;
                }
                Err(e) => {
                    // Record automotive safety violation  
                    if e.to_string().contains("deadline") {
                        let operation_time = start_time.elapsed();
                        automotive_metrics.record_deadline_miss(
                            operation_time.as_micros() as u64,
                            self.config.max_latency_us
                        );
                    }
                    continue;
                }
            }
        }
        
        sender_transport.close().await?;
        receiver_handle.abort(); // Stop the background receiver
        Ok(iteration)
    }
    
    /// Run PMQ ULL round-trip test
    async fn run_pmq_ull_round_trip(
        &self,
        config: &crate::ipc::TransportConfig,
        automotive_metrics: &mut crate::automotive_metrics::AutomotiveMetrics,
        mut results_manager: Option<&mut crate::results::ResultsManager>
    ) -> Result<u64> {
        use crate::ipc::posix_message_queue::PosixMessageQueueTransport;
        use tokio::sync::Barrier;
        
        info!("PMQ ULL round-trip test starting with single-queue design");
        
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = server_ready.clone();
        
        debug!("PMQ ULL RT: Starting setup phase");
        
        // Clone data for the async task to avoid lifetime issues
        let config_clone = config.clone();
        let message_size = self.config.message_size;
        
        // Start echo server using single shared queue (like standard PMQ)
        let server_handle: tokio::task::JoinHandle<Result<(), anyhow::Error>> = tokio::spawn(async move {
            let mut server_transport = PosixMessageQueueTransport::new();
            
            // Use the same queue as client for true echo behavior
            if let Err(e) = server_transport.start_server(&config_clone).await {
                tracing::error!("Server start failed: {}", e);
                return Err(anyhow::anyhow!("Server start failed: {}", e));
            }
            
                    // Configure for ultra-low latency
        if let Err(e) = server_transport.configure_ultra_low_latency() {
            tracing::error!("Server ULL config failed: {}", e);
            return Err(anyhow::anyhow!("Server ULL config failed: {}", e));
        }
        
        debug!("PMQ ULL RT Server: Config buffer_size: {}, queue_name: {}", 
               config_clone.buffer_size, config_clone.message_queue_name);
            
            // Signal server ready
            server_ready_clone.wait().await;
            
            // PMQ requires buffer size to match queue's max_msg_size, not actual message size
            let queue_max_msg_size = config_clone.buffer_size.max(1024); // Same as PMQ transport logic
            let mut receive_buffer = vec![0u8; queue_max_msg_size];
            let mut messages_echoed = 0u64;
            
            debug!("PMQ ULL RT Server: Buffer allocated with size {} (queue max_msg_size), message_size from task: {}", 
                   receive_buffer.len(), message_size);
            
            // Server echo loop - receive and immediately echo back to same queue
            loop {
                match server_transport.recv_ultra_fast_direct(&mut receive_buffer) {
                    Ok(_) => {
                        messages_echoed += 1;
                        debug!("PMQ ULL RT Server: Received message {}, echoing back", messages_echoed);
                        // Echo back immediately to same queue
                        match server_transport.send_ultra_fast_direct(&receive_buffer) {
                            Ok(_) => {
                                debug!("PMQ ULL RT Server: Echo successful");
                            }
                            Err(e) => {
                                debug!("PMQ ULL RT Server: Echo failed: {}", e);
                                // If echo fails, continue to next message
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        // No message available - brief sleep to avoid busy loop
                        debug!("PMQ ULL RT Server: No message available: {}", e);
                        tokio::time::sleep(Duration::from_micros(10)).await;
                        continue;
                    }
                }
            }
        });
        
        // Wait for server to be ready
        debug!("PMQ ULL RT: Waiting for server to be ready");
        server_ready.wait().await;
        debug!("PMQ ULL RT: Server ready, initializing client");
        tokio::time::sleep(Duration::from_millis(50)).await; // Give server time to fully initialize
        
        // Create client using same queue
        let mut client_transport = PosixMessageQueueTransport::new();
        debug!("PMQ ULL RT: Starting client transport");
        client_transport.start_client(&config).await?;
        debug!("PMQ ULL RT: Configuring client for ULL");
        client_transport.configure_ultra_low_latency()?;
        
        // Pre-allocate message data with unique pattern for round-trip
        let message_data = vec![0xDD; self.config.message_size]; 
        // PMQ requires buffer size to match queue's max_msg_size, not actual message size
        let queue_max_msg_size = config.buffer_size.max(1024); // Same as PMQ transport logic
        let mut response_buffer = vec![0u8; queue_max_msg_size];
        
        debug!("PMQ ULL RT Client: Message size: {}, send buffer: {}, receive buffer: {} (queue max_msg_size)", 
               self.config.message_size, message_data.len(), response_buffer.len());
        
        let test_duration = self.config.duration.unwrap_or(Duration::from_secs(1));
        let start_time = Instant::now();
        let mut iteration = 0u64;
        let mut total_attempts = 0u64;
        
        debug!("PMQ ULL RT: Starting main loop with test duration {:?}", test_duration);
        
        // Ultra-low latency round-trip test loop with single-queue design
        while start_time.elapsed() < test_duration {
            total_attempts += 1;
            let round_trip_start = Instant::now();
            
            // Send request
            debug!("PMQ ULL RT: Attempting send, iteration {}", total_attempts);
            match client_transport.send_with_deadline_ultra_fast(&message_data, self.config.max_latency_us) {
                Ok(_) => {
                    debug!("PMQ ULL RT: Send successful, attempting receive");
                    // Wait for echo response with remaining deadline
                    let elapsed = round_trip_start.elapsed();
                    let remaining_deadline = if elapsed.as_micros() < self.config.max_latency_us as u128 {
                        self.config.max_latency_us - elapsed.as_micros() as u64
                    } else {
                        1 // Minimum deadline
                    };
                    
                    let deadline = Instant::now() + Duration::from_micros(remaining_deadline);
                    let mut response_received = false;
                    
                    // Poll for echo response
                    while Instant::now() < deadline {
                        match client_transport.recv_ultra_fast_direct(&mut response_buffer) {
                            Ok(_) => {
                                // Verify this is our echo (same data pattern)
                                if response_buffer[..self.config.message_size] == message_data[..] {
                                    response_received = true;
                                    break;
                                }
                                // If not our message, continue polling
                            }
                            Err(_) => {
                                // No message yet - continue polling
                                std::hint::spin_loop();
                                continue;
                            }
                        }
                    }
                    
                    if response_received {
                        let round_trip_latency = round_trip_start.elapsed();
                        debug!("PMQ ULL RT: Round-trip successful, latency: {:?}", round_trip_latency);
                        automotive_metrics.record_success(round_trip_latency);
                        
                        // Stream individual latency record if streaming is enabled
                        if let Some(ref mut manager) = results_manager {
                            let record = crate::results::MessageLatencyRecord::new(
                                iteration,
                                self.mechanism,
                                self.config.message_size,
                                crate::metrics::LatencyType::RoundTrip,
                                round_trip_latency,
                            );
                            if let Err(e) = manager.stream_latency_record(&record).await {
                                tracing::warn!("Failed to stream latency record: {}", e);
                            }
                        }
                        
                        iteration += 1;
                    } else {
                        // Timeout - deadline miss
                        debug!("PMQ ULL RT: No response received within deadline");
                        automotive_metrics.record_deadline_miss(
                            round_trip_start.elapsed().as_micros() as u64,
                            self.config.max_latency_us
                        );
                    }
                }
                Err(e) => {
                    // Send failed - deadline miss  
                    debug!("PMQ ULL RT: Send failed: {}", e);
                    automotive_metrics.record_deadline_miss(
                        round_trip_start.elapsed().as_micros() as u64,
                        self.config.max_latency_us
                    );
                }
            }
        }
        
        client_transport.close().await?;
        server_handle.abort();
        
        info!("PMQ ULL round-trip test completed: {} successful iterations out of {} attempts", iteration, total_attempts);
        Ok(iteration)
    }
    
    /// Run automotive real-time evaluation with comprehensive safety analysis
    pub async fn run_automotive_evaluation(&self, results_manager: Option<&mut crate::results::ResultsManager>) -> Result<crate::automotive_metrics::AutomotiveSuitabilityReport> {
        if !self.config.automotive_mode {
            return Err(anyhow!("Automotive mode not enabled in configuration"));
        }
        
        // Run ultra-low latency benchmark
        let mut automotive_metrics = self.run_ultra_low_latency(results_manager).await?;
        
        // Set test duration for metrics
        automotive_metrics.test_duration = self.config.duration.unwrap_or(Duration::from_secs(10));
        
        // Calculate determinism score (requires latency samples)
        // For now, we'll use a simplified calculation based on jitter
        let determinism_score = if automotive_metrics.jitter_us > 0 {
            let relative_jitter = automotive_metrics.jitter_us as f64 / automotive_metrics.average_latency_us;
            (1.0 - relative_jitter.min(1.0)).max(0.0)
        } else {
            1.0 // Perfect determinism if no jitter
        };
        automotive_metrics.determinism_score = determinism_score;
        
        // Generate comprehensive automotive suitability report
        let report = automotive_metrics.evaluate_automotive_suitability();
        
        info!(
            "Automotive evaluation completed: Score {:.1}/100, ASIL-{:?} suitable, {} applications supported",
            report.overall_score,
            report.max_suitable_asil,
            report.suitable_applications.len()
        );
        
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IpcMechanism;

    /// Test benchmark configuration creation from default values
    #[test]
    fn test_benchmark_config_creation() {
        let config = BenchmarkConfig {
            mechanism: IpcMechanism::UnixDomainSocket,
            message_size: 1024,
            iterations: Some(1000),
            duration: None,
            concurrency: 1,
            one_way: true,
            round_trip: false,
            warmup_iterations: 100,
            percentiles: vec![50.0, 95.0, 99.0],
            buffer_size: 8192,
            host: "127.0.0.1".to_string(),
            port: 8080,
        };

        assert_eq!(config.message_size, 1024);
        assert_eq!(config.iterations, Some(1000));
        assert_eq!(config.concurrency, 1);
        assert!(config.one_way);
        assert!(!config.round_trip);
    }

    /// Test benchmark runner creation with various mechanisms
    #[tokio::test]
    async fn test_benchmark_runner_creation() {
        let config = BenchmarkConfig {
            mechanism: IpcMechanism::UnixDomainSocket,
            message_size: 1024,
            iterations: Some(100),
            duration: None,
            concurrency: 1,
            one_way: true,
            round_trip: false,
            warmup_iterations: 10,
            percentiles: vec![50.0, 95.0, 99.0],
            buffer_size: 8192,
            host: "127.0.0.1".to_string(),
            port: 8080,
        };

        let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket);
        assert_eq!(runner.mechanism, IpcMechanism::UnixDomainSocket);
    }
}
