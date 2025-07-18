use crate::{
    cli::{Args, IpcMechanism},
    ipc::{Message, MessageType, TransportConfig, TransportFactory},
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
#[derive(Clone, Debug)]
pub struct BenchmarkConfig {
    pub mechanism: IpcMechanism,
    pub message_size: usize,
    pub iterations: Option<usize>,
    pub duration: Option<Duration>,
    pub concurrency: usize,
    pub one_way: bool,
    pub round_trip: bool,
    pub warmup_iterations: usize,
    pub percentiles: Vec<f64>,
    pub buffer_size: usize,
    pub host: String,
    pub port: u16,
}

impl BenchmarkConfig {
    /// Create benchmark configuration from CLI arguments
    pub fn from_args(args: &Args) -> Result<Self> {
        Ok(Self {
            mechanism: IpcMechanism::UnixDomainSocket, // Will be overridden per test
            message_size: args.message_size,
            iterations: if args.duration.is_some() {
                None
            } else {
                Some(args.iterations)
            },
            duration: args.duration,
            concurrency: args.concurrency,
            one_way: args.one_way,
            round_trip: args.round_trip,
            warmup_iterations: args.warmup_iterations,
            percentiles: args.percentiles.clone(),
            buffer_size: args.buffer_size,
            host: args.host.clone(),
            port: args.port,
        })
    }
}

/// Benchmark runner that coordinates the execution of IPC performance tests
pub struct BenchmarkRunner {
    config: BenchmarkConfig,
    mechanism: IpcMechanism,
}

impl BenchmarkRunner {
    /// Create a new benchmark runner
    pub fn new(config: BenchmarkConfig, mechanism: IpcMechanism) -> Self {
        Self { config, mechanism }
    }

    /// Run the benchmark and return results
    pub async fn run(&self) -> Result<BenchmarkResults> {
        info!("Starting benchmark for {} mechanism", self.mechanism);

        let mut results = BenchmarkResults::new(
            self.mechanism,
            self.config.message_size,
            self.config.concurrency,
            self.config.iterations,
            self.config.duration,
        );

        // Run warmup if configured
        if self.config.warmup_iterations > 0 {
            info!(
                "Running warmup with {} iterations",
                self.config.warmup_iterations
            );
            self.run_warmup().await?;
        }

        // Run one-way latency test if enabled
        if self.config.one_way {
            info!("Running one-way latency test");
            let one_way_results = self.run_one_way_test().await?;
            results.add_one_way_results(one_way_results);
        }

        // Run round-trip latency test if enabled
        if self.config.round_trip {
            info!("Running round-trip latency test");
            let round_trip_results = self.run_round_trip_test().await?;
            results.add_round_trip_results(round_trip_results);
        }

        info!("Benchmark completed for {} mechanism", self.mechanism);
        Ok(results)
    }

    /// Run warmup iterations to stabilize performance
    async fn run_warmup(&self) -> Result<()> {
        let transport_config = self.create_transport_config()?;
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server and client
        let server_handle = {
            let config = transport_config.clone();
            let mechanism = self.mechanism; // Copy mechanism to move into closure
            let warmup_iterations = self.config.warmup_iterations;
            tokio::spawn(async move {
                let mut transport = TransportFactory::create(&mechanism)?;
                transport.start_server(&config).await?;
                
                // Signal that server is ready
                server_ready_clone.wait().await;
                debug!("Server signaled ready for warmup");

                // Receive warmup messages
                for _ in 0..warmup_iterations {
                    let _ = transport.receive().await?;
                }

                transport.close().await?;
                Ok::<(), anyhow::Error>(())
            })
        };

        // Wait for server to be ready
        server_ready.wait().await;
        debug!("Client received server ready signal for warmup");

        // Connect client and send warmup messages
        client_transport.start_client(&transport_config).await?;

        let payload = vec![0u8; self.config.message_size];
        for i in 0..self.config.warmup_iterations {
            let message = Message::new(i as u64, payload.clone(), MessageType::OneWay);
            client_transport.send(&message).await?;
        }

        client_transport.close().await?;
        server_handle.await??;

        debug!("Warmup completed");
        Ok(())
    }

    /// Run one-way latency test
    async fn run_one_way_test(&self) -> Result<PerformanceMetrics> {
        let transport_config = self.create_transport_config()?;
        let mut metrics_collector =
            MetricsCollector::new(Some(LatencyType::OneWay), self.config.percentiles.clone())?;

        // Check for problematic configurations
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
            // Run single-threaded instead
            self.run_single_threaded_one_way(&transport_config, &mut metrics_collector)
                .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_one_way(&transport_config, &mut metrics_collector)
                .await?;
        } else {
            self.run_multi_threaded_one_way(&transport_config, &mut metrics_collector)
                .await?;
        }

        Ok(metrics_collector.get_metrics())
    }

    /// Run round-trip latency test
    async fn run_round_trip_test(&self) -> Result<PerformanceMetrics> {
        let transport_config = self.create_transport_config()?;
        let mut metrics_collector = MetricsCollector::new(
            Some(LatencyType::RoundTrip),
            self.config.percentiles.clone(),
        )?;

        // Check for problematic configurations
        if self.mechanism == IpcMechanism::SharedMemory && self.config.concurrency > 1 {
            warn!(
                "Shared memory with concurrency > 1 has race conditions. Forcing concurrency = 1."
            );
            // Run single-threaded instead
            self.run_single_threaded_round_trip(&transport_config, &mut metrics_collector)
                .await?;
        } else if self.config.concurrency == 1 {
            self.run_single_threaded_round_trip(&transport_config, &mut metrics_collector)
                .await?;
        } else {
            self.run_multi_threaded_round_trip(&transport_config, &mut metrics_collector)
                .await?;
        }

        Ok(metrics_collector.get_metrics())
    }

    /// Run single-threaded one-way test
    async fn run_single_threaded_one_way(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
    ) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server
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
            }
        }

        client_transport.close().await?;
        server_handle.await??;

        Ok(())
    }

    /// Run single-threaded round-trip test
    async fn run_single_threaded_round_trip(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
    ) -> Result<()> {
        let _server_transport = TransportFactory::create(&self.mechanism)?;
        let mut client_transport = TransportFactory::create(&self.mechanism)?;

        // Use a barrier to synchronize server startup
        let server_ready = Arc::new(Barrier::new(2));
        let server_ready_clone = Arc::clone(&server_ready);

        // Start server
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
                                i += 1;
                            }
                            Ok(Err(_)) => break, // Transport error
                            Err(_) => {
                                // Receive timeout, continue
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
            }
        }

        client_transport.close().await?;
        server_handle.await??;

        Ok(())
    }

    /// Run multi-threaded one-way test
    async fn run_multi_threaded_one_way(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
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
            self.run_single_threaded_one_way(transport_config, &mut worker_metrics)
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
    async fn run_multi_threaded_round_trip(
        &self,
        transport_config: &TransportConfig,
        metrics_collector: &mut MetricsCollector,
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
            self.run_single_threaded_round_trip(transport_config, &mut worker_metrics)
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

    /// Create transport configuration
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
            let queue_depth = 10; // Always use system default
            
            debug!("PMQ using conservative queue depth: {} iterations -> depth {}", iterations, queue_depth);
            queue_depth
        } else {
            10 // Default for other mechanisms
        };

        Ok(TransportConfig {
            buffer_size: adaptive_buffer_size,
            host: self.config.host.clone(),
            port: unique_port,
            socket_path: format!("/tmp/ipc_benchmark_{}.sock", unique_id),
            shared_memory_name: format!("ipc_benchmark_{}", unique_id),
            max_connections: self.config.concurrency.max(16), // Set based on concurrency level
            message_queue_depth: adaptive_queue_depth,
            message_queue_name: format!("ipc_benchmark_pmq_{}", unique_id),
        })
    }

    /// Calculate adaptive buffer size based on test parameters
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
    fn get_iteration_count(&self) -> usize {
        self.config.iterations.unwrap_or(10000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IpcMechanism;

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
