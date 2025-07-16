use crate::{cli::IpcMechanism, metrics::PerformanceMetrics};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

/// Complete benchmark results for a specific IPC mechanism
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResults {
    pub mechanism: IpcMechanism,
    pub test_config: TestConfiguration,
    pub one_way_results: Option<PerformanceMetrics>,
    pub round_trip_results: Option<PerformanceMetrics>,
    pub summary: BenchmarkSummary,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub test_duration: Duration,
    pub system_info: SystemInfo,
}

/// Test configuration used for the benchmark
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfiguration {
    pub message_size: usize,
    pub concurrency: usize,
    pub iterations: Option<usize>,
    pub duration: Option<Duration>,
    pub one_way_enabled: bool,
    pub round_trip_enabled: bool,
    pub warmup_iterations: usize,
    pub percentiles: Vec<f64>,
    pub buffer_size: usize,
}

/// Summary of benchmark results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    pub total_messages_sent: usize,
    pub total_bytes_transferred: usize,
    pub average_throughput_mbps: f64,
    pub peak_throughput_mbps: f64,
    pub average_latency_ns: Option<f64>,
    pub min_latency_ns: Option<u64>,
    pub max_latency_ns: Option<u64>,
    pub p95_latency_ns: Option<u64>,
    pub p99_latency_ns: Option<u64>,
    pub error_count: usize,
}

/// System information for reproducibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub architecture: String,
    pub cpu_cores: usize,
    pub memory_gb: f64,
    pub rust_version: String,
    pub benchmark_version: String,
}

/// Results manager for handling output and streaming
pub struct ResultsManager {
    output_file: std::path::PathBuf,
    streaming_file: Option<std::path::PathBuf>,
    results: Vec<BenchmarkResults>,
    streaming_enabled: bool,
}

impl ResultsManager {
    /// Create a new results manager
    pub fn new(output_file: &Path) -> Result<Self> {
        Ok(Self {
            output_file: output_file.to_path_buf(),
            streaming_file: None,
            results: Vec::new(),
            streaming_enabled: false,
        })
    }

    /// Enable streaming results to a file
    pub fn enable_streaming<P: AsRef<Path>>(&mut self, streaming_file: P) -> Result<()> {
        self.streaming_file = Some(streaming_file.as_ref().to_path_buf());
        self.streaming_enabled = true;

        // Create/truncate the streaming file
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.streaming_file.as_ref().unwrap())?;

        // Write JSON array opening
        writeln!(file, "[")?;

        debug!("Enabled streaming to: {:?}", self.streaming_file);
        Ok(())
    }

    /// Add benchmark results
    pub async fn add_results(&mut self, results: BenchmarkResults) -> Result<()> {
        info!("Adding results for {} mechanism", results.mechanism);

        // Stream results if enabled
        if self.streaming_enabled {
            self.stream_results(&results).await?;
        }

        self.results.push(results);
        Ok(())
    }

    /// Stream results to file during execution
    async fn stream_results(&self, results: &BenchmarkResults) -> Result<()> {
        if let Some(ref streaming_file) = self.streaming_file {
            let mut file = OpenOptions::new().append(true).open(streaming_file)?;

            // Add comma if not first result
            if !self.results.is_empty() {
                writeln!(file, ",")?;
            }

            // Write JSON result
            let json = serde_json::to_string_pretty(results)?;
            write!(file, "{}", json)?;
            file.flush()?;
        }

        Ok(())
    }

    /// Finalize results and write to output file
    pub fn finalize(&mut self) -> Result<()> {
        info!("Finalizing benchmark results");

        // Close streaming file if enabled
        if self.streaming_enabled {
            if let Some(ref streaming_file) = self.streaming_file {
                let mut file = OpenOptions::new().append(true).open(streaming_file)?;

                // Write JSON array closing
                writeln!(file, "\n]")?;
                file.flush()?;
            }
        }

        // Write final results
        self.write_final_results()?;

        info!("Results written to: {:?}", self.output_file);
        Ok(())
    }

    /// Write final consolidated results
    fn write_final_results(&self) -> Result<()> {
        let final_results = FinalBenchmarkResults {
            metadata: BenchmarkMetadata {
                version: crate::VERSION.to_string(),
                timestamp: chrono::Utc::now(),
                total_tests: self.results.len(),
                system_info: self.get_system_info(),
            },
            results: self.results.clone(),
            summary: self.calculate_overall_summary(),
        };

        let json = serde_json::to_string_pretty(&final_results)?;
        std::fs::write(&self.output_file, json)?;

        Ok(())
    }

    /// Calculate overall summary across all tests
    fn calculate_overall_summary(&self) -> OverallSummary {
        let mut total_messages = 0;
        let mut total_bytes = 0;
        let mut total_errors = 0;
        let mut mechanisms = HashMap::new();

        for result in &self.results {
            total_messages += result.summary.total_messages_sent;
            total_bytes += result.summary.total_bytes_transferred;
            total_errors += result.summary.error_count;

            mechanisms.insert(
                result.mechanism.to_string(),
                MechanismSummary {
                    mechanism: result.mechanism,
                    average_throughput_mbps: result.summary.average_throughput_mbps,
                    p95_latency_ns: result.summary.p95_latency_ns,
                    p99_latency_ns: result.summary.p99_latency_ns,
                    total_messages: result.summary.total_messages_sent,
                },
            );
        }

        OverallSummary {
            total_messages,
            total_bytes,
            total_errors,
            mechanisms,
            fastest_mechanism: self.find_fastest_mechanism(),
            lowest_latency_mechanism: self.find_lowest_latency_mechanism(),
        }
    }

    /// Find the mechanism with highest throughput
    fn find_fastest_mechanism(&self) -> Option<String> {
        self.results
            .iter()
            .max_by(|a, b| {
                a.summary
                    .average_throughput_mbps
                    .partial_cmp(&b.summary.average_throughput_mbps)
                    .unwrap()
            })
            .map(|result| result.mechanism.to_string())
    }

    /// Find the mechanism with lowest latency
    fn find_lowest_latency_mechanism(&self) -> Option<String> {
        self.results
            .iter()
            .filter(|r| r.summary.average_latency_ns.is_some())
            .min_by(|a, b| {
                a.summary
                    .average_latency_ns
                    .partial_cmp(&b.summary.average_latency_ns)
                    .unwrap()
            })
            .map(|result| result.mechanism.to_string())
    }

    /// Get system information
    fn get_system_info(&self) -> SystemInfo {
        SystemInfo {
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            cpu_cores: num_cpus::get(),
            memory_gb: Self::get_memory_gb(),
            rust_version: Self::get_rust_version(),
            benchmark_version: crate::VERSION.to_string(),
        }
    }

    /// Get available memory in GB
    fn get_memory_gb() -> f64 {
        // This is a simplified implementation
        // In a real implementation, you'd want to use a system info crate
        16.0 // Default assumption
    }

    /// Get Rust version
    fn get_rust_version() -> String {
        env!("CARGO_PKG_RUST_VERSION").to_string()
    }
}

/// Final benchmark results structure
#[derive(Debug, Serialize, Deserialize)]
pub struct FinalBenchmarkResults {
    pub metadata: BenchmarkMetadata,
    pub results: Vec<BenchmarkResults>,
    pub summary: OverallSummary,
}

/// Benchmark metadata
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkMetadata {
    pub version: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub total_tests: usize,
    pub system_info: SystemInfo,
}

/// Overall summary across all mechanisms
#[derive(Debug, Serialize, Deserialize)]
pub struct OverallSummary {
    pub total_messages: usize,
    pub total_bytes: usize,
    pub total_errors: usize,
    pub mechanisms: HashMap<String, MechanismSummary>,
    pub fastest_mechanism: Option<String>,
    pub lowest_latency_mechanism: Option<String>,
}

/// Summary for a specific mechanism
#[derive(Debug, Serialize, Deserialize)]
pub struct MechanismSummary {
    pub mechanism: IpcMechanism,
    pub average_throughput_mbps: f64,
    pub p95_latency_ns: Option<u64>,
    pub p99_latency_ns: Option<u64>,
    pub total_messages: usize,
}

impl BenchmarkResults {
    /// Create new benchmark results
    pub fn new(
        mechanism: IpcMechanism,
        message_size: usize,
        concurrency: usize,
        iterations: Option<usize>,
        duration: Option<Duration>,
    ) -> Self {
        let test_config = TestConfiguration {
            message_size,
            concurrency,
            iterations,
            duration,
            one_way_enabled: false,
            round_trip_enabled: false,
            warmup_iterations: 0,
            percentiles: vec![50.0, 95.0, 99.0, 99.9],
            buffer_size: 8192,
        };

        Self {
            mechanism,
            test_config,
            one_way_results: None,
            round_trip_results: None,
            summary: BenchmarkSummary::default(),
            timestamp: chrono::Utc::now(),
            test_duration: Duration::ZERO,
            system_info: SystemInfo::default(),
        }
    }

    /// Add one-way test results
    pub fn add_one_way_results(&mut self, results: PerformanceMetrics) {
        self.one_way_results = Some(results);
        self.update_summary();
    }

    /// Add round-trip test results
    pub fn add_round_trip_results(&mut self, results: PerformanceMetrics) {
        self.round_trip_results = Some(results);
        self.update_summary();
    }

    /// Update summary with current results
    fn update_summary(&mut self) {
        let mut total_messages = 0;
        let mut total_bytes = 0;
        let mut throughput_values = Vec::new();
        let mut latency_values = Vec::new();

        // Process one-way results
        if let Some(ref results) = self.one_way_results {
            total_messages += results.throughput.total_messages;
            total_bytes += results.throughput.total_bytes;
            throughput_values.push(results.throughput.bytes_per_second);

            if let Some(ref latency) = results.latency {
                latency_values.push(latency.mean_ns);
            }
        }

        // Process round-trip results
        if let Some(ref results) = self.round_trip_results {
            total_messages += results.throughput.total_messages;
            total_bytes += results.throughput.total_bytes;
            throughput_values.push(results.throughput.bytes_per_second);

            if let Some(ref latency) = results.latency {
                latency_values.push(latency.mean_ns);
            }
        }

        // Calculate summary metrics
        let average_throughput_mbps =
            throughput_values.iter().sum::<f64>() / throughput_values.len() as f64 / 1_000_000.0;
        let peak_throughput_mbps =
            throughput_values.iter().cloned().fold(0.0, f64::max) / 1_000_000.0;

        let average_latency_ns = if !latency_values.is_empty() {
            Some(latency_values.iter().sum::<f64>() / latency_values.len() as f64)
        } else {
            None
        };

        let (min_latency_ns, max_latency_ns, p95_latency_ns, p99_latency_ns) =
            self.extract_latency_stats();

        self.summary = BenchmarkSummary {
            total_messages_sent: total_messages,
            total_bytes_transferred: total_bytes,
            average_throughput_mbps,
            peak_throughput_mbps,
            average_latency_ns,
            min_latency_ns,
            max_latency_ns,
            p95_latency_ns,
            p99_latency_ns,
            error_count: 0, // TODO: Track errors
        };
    }

    /// Extract latency statistics from results
    fn extract_latency_stats(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        let mut min_latency = None;
        let mut max_latency = None;
        let mut p95_latency = None;
        let mut p99_latency = None;

        for results in [&self.one_way_results, &self.round_trip_results]
            .iter()
            .filter_map(|&x| x.as_ref())
        {
            if let Some(ref latency) = results.latency {
                min_latency =
                    Some(min_latency.map_or(latency.min_ns, |min: u64| min.min(latency.min_ns)));
                max_latency =
                    Some(max_latency.map_or(latency.max_ns, |max: u64| max.max(latency.max_ns)));

                // Find P95 and P99 values
                for percentile in &latency.percentiles {
                    if (percentile.percentile - 95.0).abs() < 0.1 {
                        p95_latency = Some(percentile.value_ns);
                    }
                    if (percentile.percentile - 99.0).abs() < 0.1 {
                        p99_latency = Some(percentile.value_ns);
                    }
                }
            }
        }

        (min_latency, max_latency, p95_latency, p99_latency)
    }
}

impl Default for BenchmarkSummary {
    fn default() -> Self {
        Self {
            total_messages_sent: 0,
            total_bytes_transferred: 0,
            average_throughput_mbps: 0.0,
            peak_throughput_mbps: 0.0,
            average_latency_ns: None,
            min_latency_ns: None,
            max_latency_ns: None,
            p95_latency_ns: None,
            p99_latency_ns: None,
            error_count: 0,
        }
    }
}

impl Default for SystemInfo {
    fn default() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            cpu_cores: num_cpus::get(),
            memory_gb: 16.0,
            rust_version: "1.75.0".to_string(),
            benchmark_version: crate::VERSION.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IpcMechanism;
    use tempfile::NamedTempFile;

    #[test]
    fn test_benchmark_results_creation() {
        let results =
            BenchmarkResults::new(IpcMechanism::UnixDomainSocket, 1024, 1, Some(1000), None);

        assert_eq!(results.mechanism, IpcMechanism::UnixDomainSocket);
        assert_eq!(results.test_config.message_size, 1024);
        assert_eq!(results.test_config.concurrency, 1);
        assert_eq!(results.test_config.iterations, Some(1000));
        assert!(results.one_way_results.is_none());
        assert!(results.round_trip_results.is_none());
    }

    #[test]
    fn test_results_manager_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let manager = ResultsManager::new(temp_file.path()).unwrap();

        assert_eq!(manager.output_file, temp_file.path());
        assert!(!manager.streaming_enabled);
        assert!(manager.results.is_empty());
    }

    #[test]
    fn test_system_info_default() {
        let info = SystemInfo::default();

        assert!(!info.os.is_empty());
        assert!(!info.architecture.is_empty());
        assert!(info.cpu_cores > 0);
        assert!(info.memory_gb > 0.0);
    }
}
