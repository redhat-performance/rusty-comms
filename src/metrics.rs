use anyhow::Result;
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Latency measurement types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LatencyType {
    OneWay,
    RoundTrip,
}

/// Comprehensive latency metrics including percentiles and statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyMetrics {
    pub latency_type: LatencyType,
    pub min_ns: u64,
    pub max_ns: u64,
    pub mean_ns: f64,
    pub median_ns: f64,
    pub std_dev_ns: f64,
    pub percentiles: Vec<PercentileValue>,
    pub total_samples: usize,
    pub histogram_data: Vec<u64>,
}

/// Percentile value pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PercentileValue {
    pub percentile: f64,
    pub value_ns: u64,
}

/// Throughput metrics including message rate and bandwidth
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputMetrics {
    pub messages_per_second: f64,
    pub bytes_per_second: f64,
    pub total_messages: usize,
    pub total_bytes: usize,
    pub duration_ns: u64,
}

/// Combined performance metrics for a benchmark run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub latency: Option<LatencyMetrics>,
    pub throughput: ThroughputMetrics,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Latency collector using HDR histogram for accurate measurement
pub struct LatencyCollector {
    histogram: Histogram<u64>,
    latency_type: LatencyType,
    start_time: Instant,
    sample_count: usize,
}

impl LatencyCollector {
    /// Create a new latency collector
    pub fn new(latency_type: LatencyType) -> Result<Self> {
        // Create histogram with 3 significant figures, max value 1 minute
        let histogram = Histogram::<u64>::new(3)?;

        Ok(Self {
            histogram,
            latency_type,
            start_time: Instant::now(),
            sample_count: 0,
        })
    }

    /// Record a latency measurement
    pub fn record(&mut self, latency: Duration) -> Result<()> {
        let latency_ns = latency.as_nanos() as u64;
        self.histogram.record(latency_ns)?;
        self.sample_count += 1;
        Ok(())
    }

    /// Get the current metrics
    pub fn get_metrics(&self, percentiles: &[f64]) -> LatencyMetrics {
        let mut percentile_values = Vec::new();

        for &p in percentiles {
            let value = self.histogram.value_at_percentile(p);
            percentile_values.push(PercentileValue {
                percentile: p,
                value_ns: value,
            });
        }

        // Calculate standard deviation
        let mean = self.histogram.mean();
        let std_dev = self.histogram.stdev();

        LatencyMetrics {
            latency_type: self.latency_type,
            min_ns: self.histogram.min(),
            max_ns: self.histogram.max(),
            mean_ns: mean,
            median_ns: self.histogram.value_at_percentile(50.0) as f64,
            std_dev_ns: std_dev,
            percentiles: percentile_values,
            total_samples: self.sample_count,
            histogram_data: self.get_histogram_data(),
        }
    }

    /// Get histogram data for analysis
    fn get_histogram_data(&self) -> Vec<u64> {
        let mut data = Vec::new();
        for value in self.histogram.iter_quantiles(1) {
            data.push(value.value_iterated_to());
        }
        data
    }

    /// Reset the collector
    pub fn reset(&mut self) {
        self.histogram.reset();
        self.sample_count = 0;
        self.start_time = Instant::now();
    }
}

/// Throughput calculator for measuring message and data rates
pub struct ThroughputCalculator {
    start_time: Instant,
    message_count: usize,
    byte_count: usize,
}

impl ThroughputCalculator {
    /// Create a new throughput calculator
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            message_count: 0,
            byte_count: 0,
        }
    }

    /// Record a message transmission
    pub fn record_message(&mut self, message_size: usize) {
        self.message_count += 1;
        self.byte_count += message_size;
    }

    /// Get current throughput metrics
    pub fn get_metrics(&self) -> ThroughputMetrics {
        let elapsed = self.start_time.elapsed();
        let duration_ns = elapsed.as_nanos() as u64;
        let duration_secs = elapsed.as_secs_f64();

        let messages_per_second = if duration_secs > 0.0 {
            self.message_count as f64 / duration_secs
        } else {
            0.0
        };

        let bytes_per_second = if duration_secs > 0.0 {
            self.byte_count as f64 / duration_secs
        } else {
            0.0
        };

        ThroughputMetrics {
            messages_per_second,
            bytes_per_second,
            total_messages: self.message_count,
            total_bytes: self.byte_count,
            duration_ns,
        }
    }

    /// Reset the calculator
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.message_count = 0;
        self.byte_count = 0;
    }
}

/// Combined metrics collector for comprehensive performance measurement
pub struct MetricsCollector {
    pub latency_collector: Option<LatencyCollector>,
    pub throughput_calculator: ThroughputCalculator,
    pub percentiles: Vec<f64>,
}

impl MetricsCollector {
    /// Create a new metrics collector
    pub fn new(latency_type: Option<LatencyType>, percentiles: Vec<f64>) -> Result<Self> {
        let latency_collector = if let Some(lt) = latency_type {
            Some(LatencyCollector::new(lt)?)
        } else {
            None
        };

        Ok(Self {
            latency_collector,
            throughput_calculator: ThroughputCalculator::new(),
            percentiles,
        })
    }

    /// Record a message with optional latency measurement
    pub fn record_message(&mut self, message_size: usize, latency: Option<Duration>) -> Result<()> {
        self.throughput_calculator.record_message(message_size);

        if let (Some(collector), Some(lat)) = (&mut self.latency_collector, latency) {
            collector.record(lat)?;
        }

        Ok(())
    }

    /// Get current performance metrics
    pub fn get_metrics(&self) -> PerformanceMetrics {
        let latency = self
            .latency_collector
            .as_ref()
            .map(|collector| collector.get_metrics(&self.percentiles));

        let throughput = self.throughput_calculator.get_metrics();

        PerformanceMetrics {
            latency,
            throughput,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Reset all collectors
    pub fn reset(&mut self) {
        if let Some(collector) = &mut self.latency_collector {
            collector.reset();
        }
        self.throughput_calculator.reset();
    }

    /// Merge multiple worker metrics into a single aggregated result
    pub fn aggregate_worker_metrics(
        worker_metrics: Vec<PerformanceMetrics>,
        percentiles: &[f64],
    ) -> Result<PerformanceMetrics> {
        if worker_metrics.is_empty() {
            return Err(anyhow::anyhow!("Cannot aggregate empty metrics"));
        }

        // Aggregate throughput metrics
        let aggregated_throughput = Self::aggregate_throughput_metrics(
            worker_metrics.iter().map(|m| &m.throughput).collect(),
        );

        // Aggregate latency metrics if present
        let latency_metrics: Vec<&LatencyMetrics> = worker_metrics
            .iter()
            .filter_map(|m| m.latency.as_ref())
            .collect();

        let aggregated_latency = if !latency_metrics.is_empty() {
            Some(Self::aggregate_latency_metrics(
                latency_metrics,
                percentiles,
            )?)
        } else {
            None
        };

        Ok(PerformanceMetrics {
            latency: aggregated_latency,
            throughput: aggregated_throughput,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Aggregate throughput metrics from multiple workers
    fn aggregate_throughput_metrics(
        throughput_metrics: Vec<&ThroughputMetrics>,
    ) -> ThroughputMetrics {
        let total_messages: usize = throughput_metrics.iter().map(|m| m.total_messages).sum();
        let total_bytes: usize = throughput_metrics.iter().map(|m| m.total_bytes).sum();

        // Use the maximum duration among all workers
        let max_duration_ns = throughput_metrics
            .iter()
            .map(|m| m.duration_ns)
            .max()
            .unwrap_or(0);

        // Calculate aggregate rates
        let duration_secs = max_duration_ns as f64 / 1_000_000_000.0;
        let messages_per_second = if duration_secs > 0.0 {
            total_messages as f64 / duration_secs
        } else {
            0.0
        };
        let bytes_per_second = if duration_secs > 0.0 {
            total_bytes as f64 / duration_secs
        } else {
            0.0
        };

        ThroughputMetrics {
            messages_per_second,
            bytes_per_second,
            total_messages,
            total_bytes,
            duration_ns: max_duration_ns,
        }
    }

    /// Aggregate latency metrics from multiple workers
    fn aggregate_latency_metrics(
        latency_metrics: Vec<&LatencyMetrics>,
        percentiles: &[f64],
    ) -> Result<LatencyMetrics> {
        if latency_metrics.is_empty() {
            return Err(anyhow::anyhow!("Cannot aggregate empty latency metrics"));
        }

        // Collect all latency values from all workers
        let mut all_values = Vec::new();
        let mut total_samples = 0;
        let latency_type = latency_metrics[0].latency_type;

        // Reconstruct individual latency values from histogram data
        for metrics in &latency_metrics {
            total_samples += metrics.total_samples;
            // For proper aggregation, we'd need access to the raw histograms
            // This is a simplified approach using the histogram data
            all_values.extend(&metrics.histogram_data);
        }

        // Create a new histogram with aggregated data
        let mut aggregated_histogram = Histogram::<u64>::new(3)?;
        for &value in &all_values {
            aggregated_histogram.record(value)?;
        }

        // Calculate aggregated statistics
        let min_ns = latency_metrics.iter().map(|m| m.min_ns).min().unwrap_or(0);
        let max_ns = latency_metrics.iter().map(|m| m.max_ns).max().unwrap_or(0);
        let mean_ns = aggregated_histogram.mean();
        let median_ns = aggregated_histogram.value_at_percentile(50.0) as f64;
        let std_dev_ns = aggregated_histogram.stdev();

        // Calculate aggregated percentiles
        let mut percentile_values = Vec::new();
        for &p in percentiles {
            let value = aggregated_histogram.value_at_percentile(p);
            percentile_values.push(PercentileValue {
                percentile: p,
                value_ns: value,
            });
        }

        // Get histogram data for the aggregated result
        let mut histogram_data = Vec::new();
        for value in aggregated_histogram.iter_quantiles(1) {
            histogram_data.push(value.value_iterated_to());
        }

        Ok(LatencyMetrics {
            latency_type,
            min_ns,
            max_ns,
            mean_ns,
            median_ns,
            std_dev_ns,
            percentiles: percentile_values,
            total_samples,
            histogram_data,
        })
    }

    /// Create a metrics collector that aggregates results from multiple workers
    pub fn create_aggregating_collector(
        latency_type: Option<LatencyType>,
        percentiles: Vec<f64>,
    ) -> Result<Self> {
        Self::new(latency_type, percentiles)
    }

    /// Add metrics from a worker to this aggregating collector
    pub fn add_worker_metrics(&mut self, worker_metrics: &PerformanceMetrics) -> Result<()> {
        // Add throughput data
        self.throughput_calculator.message_count += worker_metrics.throughput.total_messages;
        self.throughput_calculator.byte_count += worker_metrics.throughput.total_bytes;

        // For latency, we'd need access to the raw histogram data for proper aggregation
        // This is a limitation of the current design - ideally we'd aggregate at the histogram level

        Ok(())
    }
}

/// Utility functions for metrics calculation
pub mod utils {
    use super::*;

    /// Calculate percentiles from raw latency data
    pub fn calculate_percentiles(
        mut latencies: Vec<u64>,
        percentiles: &[f64],
    ) -> Vec<PercentileValue> {
        if latencies.is_empty() {
            return Vec::new();
        }

        latencies.sort_unstable();
        let mut result = Vec::new();

        for &p in percentiles {
            let index = ((p / 100.0) * (latencies.len() - 1) as f64).round() as usize;
            let value = latencies[index.min(latencies.len() - 1)];
            result.push(PercentileValue {
                percentile: p,
                value_ns: value,
            });
        }

        result
    }

    /// Format latency value for human-readable output
    pub fn format_latency(latency_ns: u64) -> String {
        if latency_ns < 1_000 {
            format!("{}ns", latency_ns)
        } else if latency_ns < 1_000_000 {
            format!("{:.2}μs", latency_ns as f64 / 1_000.0)
        } else if latency_ns < 1_000_000_000 {
            format!("{:.2}ms", latency_ns as f64 / 1_000_000.0)
        } else {
            format!("{:.2}s", latency_ns as f64 / 1_000_000_000.0)
        }
    }

    /// Format throughput value for human-readable output
    pub fn format_throughput(bytes_per_second: f64) -> String {
        if bytes_per_second < 1024.0 {
            format!("{:.2} B/s", bytes_per_second)
        } else if bytes_per_second < 1024.0 * 1024.0 {
            format!("{:.2} KB/s", bytes_per_second / 1024.0)
        } else if bytes_per_second < 1024.0 * 1024.0 * 1024.0 {
            format!("{:.2} MB/s", bytes_per_second / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB/s", bytes_per_second / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_latency_collector() {
        let mut collector = LatencyCollector::new(LatencyType::OneWay).unwrap();

        collector.record(Duration::from_millis(1)).unwrap();
        collector.record(Duration::from_millis(2)).unwrap();
        collector.record(Duration::from_millis(3)).unwrap();

        let metrics = collector.get_metrics(&[50.0, 95.0, 99.0]);
        assert_eq!(metrics.latency_type, LatencyType::OneWay);
        assert_eq!(metrics.total_samples, 3);
        assert!(metrics.mean_ns > 0.0);
    }

    #[test]
    fn test_throughput_calculator() {
        let mut calculator = ThroughputCalculator::new();

        calculator.record_message(1024);
        calculator.record_message(2048);

        let metrics = calculator.get_metrics();
        assert_eq!(metrics.total_messages, 2);
        assert_eq!(metrics.total_bytes, 3072);
        assert!(metrics.messages_per_second >= 0.0);
        assert!(metrics.bytes_per_second >= 0.0);
    }

    #[test]
    fn test_format_latency() {
        assert_eq!(utils::format_latency(500), "500ns");
        assert_eq!(utils::format_latency(1500), "1.50μs");
        assert_eq!(utils::format_latency(1500000), "1.50ms");
        assert_eq!(utils::format_latency(1500000000), "1.50s");
    }

    #[test]
    fn test_format_throughput() {
        assert_eq!(utils::format_throughput(500.0), "500.00 B/s");
        assert_eq!(utils::format_throughput(1536.0), "1.50 KB/s");
        assert_eq!(utils::format_throughput(1572864.0), "1.50 MB/s");
        assert_eq!(utils::format_throughput(1610612736.0), "1.50 GB/s");
    }
}
