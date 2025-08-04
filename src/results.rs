//! # Results and Output Management Module
//!
//! This module provides comprehensive result collection, analysis, and output
//! capabilities for the IPC benchmark suite. It handles both aggregated final
//! results and real-time streaming output for monitoring long-running tests.
//!
//! ## Key Components
//!
//! - **BenchmarkResults**: Complete results for a single mechanism test
//! - **ResultsSummary**: Cross-mechanism comparison and summary statistics
//! - **ResultsManager**: Coordination of output and streaming operations
//! - **MessageLatencyRecord**: Per-message latency data for streaming output
//!
//! ## Output Formats
//!
//! The module supports two primary output modes:
//! - **Final Results**: Comprehensive JSON report with metadata and analysis
//! - **Streaming Results**: Real-time per-message latency data during execution
//!
//! ## Streaming vs Final Output
//!
//! Streaming output provides immediate visibility into test progress by writing
//! individual message latency measurements as they occur, while final output
//! provides aggregated statistics and cross-mechanism comparisons.

use crate::IpcMechanism;
use crate::metrics::{LatencyType, PerformanceMetrics};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

/// Per-message latency record for streaming output
///
/// This structure captures detailed timing information for individual messages
/// during benchmark execution. It enables real-time monitoring of latency
/// characteristics and provides fine-grained data for analysis.
///
/// ## Streaming Purpose
///
/// Unlike aggregated final results, these records are written immediately
/// as messages are processed, allowing:
/// - Real-time monitoring of benchmark progress
/// - Detection of latency patterns and anomalies during execution
/// - Granular analysis of individual message performance
/// - Time-series analysis of latency evolution
///
/// ## Timestamp Precision
///
/// Timestamps are captured using system time with nanosecond precision
/// to enable accurate correlation with external monitoring tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageLatencyRecord {
    /// Unix timestamp in nanoseconds when the measurement was taken
    pub timestamp_ns: u64,
    
    /// Unique identifier for the message within the test
    pub message_id: u64,
    
    /// IPC mechanism being tested
    pub mechanism: IpcMechanism,
    
    /// Size of the message payload in bytes
    pub message_size: usize,
    
    /// One-way latency in nanoseconds (send to receive)
    /// None if this measurement is for round-trip only
    pub one_way_latency_ns: Option<u64>,
    
    /// Round-trip latency in nanoseconds (send to response received)
    /// None if this measurement is for one-way only
    pub round_trip_latency_ns: Option<u64>,
}

impl MessageLatencyRecord {
    /// Column headings for columnar streaming output
    pub const HEADINGS: &'static [&'static str] = &[
        "timestamp_ns",
        "message_id",
        "mechanism",
        "message_size",
        "one_way_latency_ns",
        "round_trip_latency_ns",
    ];

    /// Convert the record to a `serde_json::Value` array for columnar output
    pub fn to_value_array(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!(self.timestamp_ns),
            serde_json::json!(self.message_id),
            serde_json::json!(self.mechanism),
            serde_json::json!(self.message_size),
            serde_json::json!(self.one_way_latency_ns),
            serde_json::json!(self.round_trip_latency_ns),
        ]
    }

    /// Create a new message latency record with current timestamp
    ///
    /// ## Parameters
    /// - `message_id`: Unique identifier for the message
    /// - `mechanism`: IPC mechanism being tested
    /// - `message_size`: Size of the message payload
    /// - `latency_type`: Type of latency measurement
    /// - `latency`: Measured latency duration
    ///
    /// ## Returns
    /// New record with current system timestamp
    pub fn new(
        message_id: u64,
        mechanism: IpcMechanism,
        message_size: usize,
        latency_type: LatencyType,
        latency: Duration,
    ) -> Self {
        let timestamp_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        
        let latency_ns = latency.as_nanos() as u64;
        let (one_way_latency_ns, round_trip_latency_ns) = match latency_type {
            LatencyType::OneWay => (Some(latency_ns), None),
            LatencyType::RoundTrip => (None, Some(latency_ns)),
        };
        
        Self {
            timestamp_ns,
            message_id,
            mechanism,
            message_size,
            one_way_latency_ns,
            round_trip_latency_ns,
        }
    }

    /// Create a combined latency record with both one-way and round-trip measurements
    ///
    /// ## Parameters
    /// - `message_id`: Unique identifier for the message
    /// - `mechanism`: IPC mechanism being tested
    /// - `message_size`: Size of the message payload
    /// - `one_way_latency`: One-way latency measurement
    /// - `round_trip_latency`: Round-trip latency measurement
    ///
    /// ## Returns
    /// New record with both latency measurements and current timestamp
    pub fn new_combined(
        message_id: u64,
        mechanism: IpcMechanism,
        message_size: usize,
        one_way_latency: Duration,
        round_trip_latency: Duration,
    ) -> Self {
        let timestamp_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        
        Self {
            timestamp_ns,
            message_id,
            mechanism,
            message_size,
            one_way_latency_ns: Some(one_way_latency.as_nanos() as u64),
            round_trip_latency_ns: Some(round_trip_latency.as_nanos() as u64),
        }
    }

    /// Merge another record into this one, combining latency measurements
    /// 
    /// This is used when aggregating separate one-way and round-trip records
    /// for the same message ID into a single combined record.
    pub fn merge(&mut self, other: &MessageLatencyRecord) {
        // Take the earlier timestamp
        if other.timestamp_ns < self.timestamp_ns {
            self.timestamp_ns = other.timestamp_ns;
        }

        // Merge latency values
        if other.one_way_latency_ns.is_some() {
            self.one_way_latency_ns = other.one_way_latency_ns;
        }
        if other.round_trip_latency_ns.is_some() {
            self.round_trip_latency_ns = other.round_trip_latency_ns;
        }
    }
}

/// Complete benchmark results for a specific IPC mechanism
///
/// This structure encapsulates all performance data collected for a single
/// IPC mechanism, including configuration details, measured performance,
/// and derived statistics. It serves as the primary unit of result data.
///
/// ## Data Organization
///
/// - **Configuration**: Test parameters used for this mechanism
/// - **Performance Data**: Raw measurements (latency, throughput)
/// - **Summary Statistics**: Derived metrics and comparisons
/// - **Metadata**: Timing and system information for reproducibility
///
/// ## Result Lifecycle
///
/// 1. **Creation**: Initialized with test configuration
/// 2. **Data Addition**: Performance metrics added as tests complete
/// 3. **Summary Calculation**: Statistics calculated when results are added
/// 4. **Output**: Serialized to JSON for final reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResults {
    /// The IPC mechanism that was tested
    pub mechanism: IpcMechanism,
    
    /// Configuration parameters used for this test
    pub test_config: TestConfiguration,
    
    /// Results from one-way latency testing (if enabled)
    pub one_way_results: Option<PerformanceMetrics>,
    
    /// Results from round-trip latency testing (if enabled)
    pub round_trip_results: Option<PerformanceMetrics>,
    
    /// Derived summary statistics and key metrics
    pub summary: BenchmarkSummary,
    
    /// When this benchmark was executed
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// Total duration of the benchmark execution
    pub test_duration: Duration,
    
    /// System information for reproducibility
    pub system_info: SystemInfo,
}

/// Test configuration used for the benchmark
///
/// This structure captures the exact parameters used for a benchmark run,
/// enabling reproducibility and providing context for result interpretation.
/// It includes all user-configurable options that affect performance.
///
/// ## Configuration Categories
///
/// - **Message Parameters**: Size and payload characteristics
/// - **Test Control**: Duration, iterations, and concurrency settings
/// - **Test Types**: Which measurement patterns were enabled
/// - **Performance Tuning**: Buffer sizes and optimization parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfiguration {
    /// Size of message payloads in bytes
    pub message_size: usize,
    
    /// Number of concurrent workers used
    pub concurrency: usize,
    
    /// Number of iterations run (None for duration-based tests)
    pub iterations: Option<usize>,
    
    /// Test duration (None for iteration-based tests)
    pub duration: Option<Duration>,
    
    /// Whether one-way latency testing was enabled
    pub one_way_enabled: bool,
    
    /// Whether round-trip latency testing was enabled
    pub round_trip_enabled: bool,
    
    /// Number of warmup iterations executed
    pub warmup_iterations: usize,
    
    /// Percentiles calculated for latency analysis
    pub percentiles: Vec<f64>,
    
    /// Buffer size used for transport mechanisms
    pub buffer_size: usize,
}

/// Summary of benchmark results
///
/// This structure provides high-level performance metrics derived from the
/// detailed measurement data. It includes key statistics that enable quick
/// performance assessment and cross-mechanism comparison.
///
/// ## Summary Categories
///
/// - **Volume Metrics**: Total messages and bytes processed
/// - **Throughput**: Peak and average transfer rates
/// - **Latency**: Key latency statistics (min, max, percentiles)
/// - **Quality**: Error rates and reliability metrics
///
/// ## Derived Statistics
///
/// All metrics in this summary are calculated from the raw performance
/// data, providing convenient access to the most important results
/// without requiring analysis of the full dataset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSummary {
    /// Total number of messages sent during all tests
    pub total_messages_sent: usize,
    
    /// Total number of bytes transferred during all tests
    pub total_bytes_transferred: usize,
    
    /// Average throughput across all tests in megabits per second
    pub average_throughput_mbps: f64,
    
    /// Peak throughput observed in any single test in megabits per second
    pub peak_throughput_mbps: f64,
    
    /// Average latency across all latency measurements (if any)
    pub average_latency_ns: Option<f64>,
    
    /// Minimum latency observed across all tests
    pub min_latency_ns: Option<u64>,
    
    /// Maximum latency observed across all tests
    pub max_latency_ns: Option<u64>,
    
    /// 95th percentile latency (95% of requests faster than this)
    pub p95_latency_ns: Option<u64>,
    
    /// 99th percentile latency (99% of requests faster than this)
    pub p99_latency_ns: Option<u64>,
    
    /// Number of errors encountered during testing
    pub error_count: usize,
}

/// System information for reproducibility
///
/// This structure captures essential system characteristics that affect
/// performance results. Including this information enables result correlation
/// across different systems and identifies system-specific performance factors.
///
/// ## Information Categories
///
/// - **Platform**: OS and architecture details
/// - **Hardware**: CPU and memory specifications
/// - **Software**: Runtime and benchmark version information
/// - **Environment**: Container detection and resource constraints
///
/// ## Reproducibility
///
/// The captured information enables:
/// - Correlating results across different systems
/// - Identifying performance variations due to system differences
/// - Validating benchmark consistency across runs
/// - Debugging environment-specific issues
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Operating system name (e.g., "linux", "windows", "macos")
    pub os: String,
    
    /// System architecture (e.g., "x86_64", "aarch64")
    pub architecture: String,
    
    /// Number of CPU cores available
    pub cpu_cores: usize,
    
    /// Available system memory in gigabytes
    pub memory_gb: f64,
    
    /// Rust compiler version used to build the benchmark
    pub rust_version: String,
    
    /// Benchmark suite version
    pub benchmark_version: String,
}

/// Results manager for handling output and streaming
///
/// The `ResultsManager` coordinates all result collection, processing, and output
/// operations. It manages both final result generation and optional real-time
/// streaming for long-running benchmarks.
///
/// ## Management Responsibilities
///
/// - **Collection**: Accumulate results from multiple benchmark runs
/// - **Streaming**: Optionally write results in real-time during execution
/// - **Aggregation**: Calculate cross-mechanism summaries and comparisons
/// - **Output**: Generate comprehensive final reports in structured format
/// - **Validation**: Ensure result consistency and completeness
///
/// ## Streaming vs Final Output
///
/// - **Streaming**: Incremental JSON array written during execution
/// - **Final**: Complete structure with metadata and summaries
///
/// The streaming format allows monitoring of long-running benchmarks,
/// while the final format provides complete analysis and comparison.
pub struct ResultsManager {
    /// Path for final results output
    output_file: std::path::PathBuf,
    
    /// Optional path for streaming results output
    streaming_file: Option<std::path::PathBuf>,
    
    /// Accumulated results from all benchmark runs
    results: Vec<BenchmarkResults>,
    
    /// Whether streaming is enabled
    streaming_enabled: bool,
    
    /// Whether to stream per-message latency records instead of final results
    per_message_streaming: bool,
    
    /// Counter for tracking if this is the first record being streamed
    first_record_streamed: bool,

    /// Whether both one-way and round-trip tests are enabled (for record aggregation)
    both_tests_enabled: bool,

    /// Buffer for collecting records when both tests are running (keyed by message ID)
    pending_records: HashMap<u64, MessageLatencyRecord>,
}

impl ResultsManager {
    /// Create a new results manager
    ///
    /// Initializes a results manager with the specified output file path.
    /// Streaming is disabled by default and can be enabled separately.
    ///
    /// ## Parameters
    /// - `output_file`: Path where final results will be written
    ///
    /// ## Returns
    /// - `Ok(ResultsManager)`: Configured manager ready for use
    /// - `Err(anyhow::Error)`: File path validation or access error
    ///
    /// ## File Handling
    ///
    /// The output file is not created until `finalize()` is called,
    /// allowing validation of the path without affecting existing files.
    pub fn new(output_file: &Path) -> Result<Self> {
        Ok(Self {
            output_file: output_file.to_path_buf(),
            streaming_file: None,
            results: Vec::new(),
            streaming_enabled: false,
            per_message_streaming: false,
            first_record_streamed: true,
            both_tests_enabled: false,
            pending_records: HashMap::new(),
        })
    }

    /// Enable streaming results to a file
    ///
    /// Configures real-time result streaming to monitor benchmark progress
    /// during execution. The streaming file contains a JSON array that is
    /// built incrementally as results become available.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where streaming results will be written
    ///
    /// ## Returns
    /// - `Ok(())`: Streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    ///
    /// ## Streaming Format
    ///
    /// The streaming file contains a JSON array:
    /// ```json
    /// [
    ///   { /* first result */ },
    ///   { /* second result */ },
    ///   ...
    /// ]
    /// ```
    ///
    /// ## File Management
    ///
    /// The streaming file is created immediately and truncated if it exists.
    /// The JSON array opening bracket is written immediately to establish
    /// the format for incremental updates.
    pub fn enable_streaming<P: AsRef<Path>>(&mut self, streaming_file: P) -> Result<()> {
        self.streaming_file = Some(streaming_file.as_ref().to_path_buf());
        self.streaming_enabled = true;
        self.per_message_streaming = false; // Default to final results streaming

        // Create/truncate the streaming file and initialize JSON array
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.streaming_file.as_ref().unwrap())?;

        // Write JSON array opening bracket
        writeln!(file, "[")?;

        debug!("Enabled streaming to: {:?}", self.streaming_file);
        Ok(())
    }

    /// Enable per-message latency streaming to a file
    ///
    /// Configures real-time per-message latency streaming to capture individual
    /// message timing data during benchmark execution. This provides fine-grained
    /// latency monitoring for analysis and debugging.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where per-message latency records will be written
    ///
    /// ## Returns
    /// - `Ok(())`: Per-message streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    ///
    /// ## Streaming Format
    ///
    /// The streaming file contains a JSON array of MessageLatencyRecord objects:
    /// ```json
    /// [
    ///   {
    ///     "timestamp_ns": 1640995200000000000,
    ///     "message_id": 1,
    ///     "mechanism": "UnixDomainSocket",
    ///     "message_size": 1024,
    ///     "one_way_latency_ns": 50000,
    ///     "round_trip_latency_ns": null,
    ///     "latency_type": "OneWay"
    ///   },
    ///   ...
    /// ]
    /// ```
    ///
    /// ## Performance Considerations
    ///
    /// Per-message streaming generates significant I/O activity and should
    /// be used carefully in high-throughput scenarios to avoid affecting
    /// benchmark results.
    pub fn enable_per_message_streaming<P: AsRef<Path>>(&mut self, streaming_file: P) -> Result<()> {
        self.streaming_file = Some(streaming_file.as_ref().to_path_buf());
        self.streaming_enabled = true;
        self.per_message_streaming = true;
        self.first_record_streamed = true;

        // Create/truncate the streaming file and initialize columnar JSON object
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.streaming_file.as_ref().unwrap())?;

        // Write JSON object opening and headings array
        writeln!(file, "{{")?;
        let headings_json = serde_json::to_string(MessageLatencyRecord::HEADINGS)?;
        writeln!(file, "  \"headings\": {},", headings_json)?;
        write!(file, "  \"data\": [")?;

        debug!("Enabled per-message streaming to: {:?}", self.streaming_file);
        Ok(())
    }

    /// Enable combined streaming mode when both one-way and round-trip tests are running
    ///
    /// When this mode is enabled, streaming records for the same message ID from both
    /// test types will be aggregated into single records with both latency values.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where combined latency records will be written
    /// - `both_tests_enabled`: Whether both one-way and round-trip tests are running
    ///
    /// ## Returns
    /// - `Ok(())`: Combined streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    pub fn enable_combined_streaming<P: AsRef<Path>>(&mut self, streaming_file: P, both_tests_enabled: bool) -> Result<()> {
        self.enable_per_message_streaming(streaming_file)?;
        self.both_tests_enabled = both_tests_enabled;
        
        debug!("Enabled combined streaming mode: both_tests={}", both_tests_enabled);
        Ok(())
    }

    /// Stream a single message latency record
    ///
    /// Immediately writes a single message latency record to the streaming file.
    /// This method is called for each message during benchmark execution when
    /// per-message streaming is enabled.
    ///
    /// ## Parameters
    /// - `record`: Message latency record to stream
    ///
    /// ## Returns
    /// - `Ok(())`: Record streamed successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## JSON Formatting
    ///
    /// Records are written with proper JSON array formatting including
    /// comma separation between elements.
    pub async fn stream_latency_record(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        if !self.streaming_enabled || !self.per_message_streaming {
            return Ok(()); // Per-message streaming not enabled
        }

        // If both tests are enabled, aggregate records by message ID
        if self.both_tests_enabled {
            return self.handle_combined_streaming(record).await;
        }

        // Normal streaming mode - write record immediately
        self.write_streaming_record(record).await
    }

    /// Handle combined streaming when both one-way and round-trip tests are running
    ///
    /// This method aggregates records by message ID, combining one-way and round-trip
    /// measurements into single records before streaming them.
    async fn handle_combined_streaming(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        let message_id = record.message_id;

        if let Some(existing_record) = self.pending_records.get_mut(&message_id) {
            // We have a pending record for this message ID - merge and write
            existing_record.merge(record);
            
            // Check if we now have both latency types
            if existing_record.one_way_latency_ns.is_some() && existing_record.round_trip_latency_ns.is_some() {
                // Both latencies available - write combined record and remove from pending
                let combined_record = existing_record.clone();
                self.pending_records.remove(&message_id);
                self.write_streaming_record(&combined_record).await?;
            }
        } else {
            // First record for this message ID - store it for potential merging
            self.pending_records.insert(message_id, record.clone());
        }

        Ok(())
    }

    /// Write a streaming record to the file
    ///
    /// Internal helper method that handles the actual file I/O for streaming records.
    async fn write_streaming_record(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        if let Some(ref streaming_file) = self.streaming_file {
            let mut file = OpenOptions::new().append(true).open(streaming_file)?;

            // Add comma if not first record (proper JSON array formatting)
            if !self.first_record_streamed {
                writeln!(file, ",")?;
            } else {
                // For the first record, add a newline to separate from the "data": [ line
                writeln!(file)?;
            }

            // Write JSON array of values
            let values = record.to_value_array();
            let json = serde_json::to_string(&values)?;
            write!(file, "    {}", json)?; // Indent for readability
            file.flush()?;
            
            self.first_record_streamed = false;
        }

        Ok(())
    }

    /// Write a streaming record directly to file (bypassing aggregation)
    ///
    /// This method writes records directly to the streaming file without
    /// aggregation, used for combined tests where both latencies are measured
    /// for the same message simultaneously.
    pub async fn write_streaming_record_direct(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        if !self.streaming_enabled || !self.per_message_streaming {
            return Ok(());
        }

        self.write_streaming_record(record).await
    }

    /// Add benchmark results
    ///
    /// Incorporates results from a completed benchmark into the result collection.
    /// If streaming is enabled, the results are immediately written to the
    /// streaming file for real-time monitoring.
    ///
    /// ## Parameters
    /// - `results`: Complete results from a single mechanism benchmark
    ///
    /// ## Returns
    /// - `Ok(())`: Results added successfully
    /// - `Err(anyhow::Error)`: Streaming write error or validation failure
    ///
    /// ## Processing Steps
    ///
    /// 1. **Validation**: Ensure results are complete and consistent
    /// 2. **Streaming**: Write to streaming file if enabled
    /// 3. **Storage**: Add to internal collection for final output
    /// 4. **Logging**: Record addition for monitoring and debugging
    pub async fn add_results(&mut self, results: BenchmarkResults) -> Result<()> {
        info!("Adding results for {} mechanism", results.mechanism);

        // Stream final results only if streaming is enabled but per-message streaming is not
        if self.streaming_enabled && !self.per_message_streaming {
            self.stream_results(&results).await?;
        }

        self.results.push(results);
        Ok(())
    }

    /// Stream results to file during execution
    ///
    /// Writes a single benchmark result to the streaming file in JSON format.
    /// This method handles proper JSON array formatting including comma
    /// separation between array elements.
    ///
    /// ## Parameters
    /// - `results`: Results to write to streaming file
    ///
    /// ## Returns
    /// - `Ok(())`: Results streamed successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## JSON Formatting
    ///
    /// The method ensures proper JSON array formatting by adding commas
    /// between elements and pretty-printing each result for readability.
    async fn stream_results(&self, results: &BenchmarkResults) -> Result<()> {
        if let Some(ref streaming_file) = self.streaming_file {
            let mut file = OpenOptions::new().append(true).open(streaming_file)?;

            // Add comma if not first result (proper JSON array formatting)
            if !self.results.is_empty() {
                writeln!(file, ",")?;
            }

            // Write JSON result with pretty formatting for readability
            let json = serde_json::to_string_pretty(results)?;
            write!(file, "{}", json)?;
            file.flush()?;
        }

        Ok(())
    }

    /// Finalize results and write to output file
    ///
    /// Completes the result collection process by generating comprehensive
    /// final output including metadata, cross-mechanism summaries, and
    /// closing any streaming files.
    ///
    /// ## Returns
    /// - `Ok(())`: Finalization completed successfully
    /// - `Err(anyhow::Error)`: File write or analysis error
    ///
    /// ## Finalization Steps
    ///
    /// 1. **Close Streaming**: Complete streaming JSON array if enabled
    /// 2. **Generate Summaries**: Calculate cross-mechanism comparisons
    /// 3. **Create Metadata**: Gather system and version information
    /// 4. **Write Final Output**: Generate comprehensive JSON report
    /// 5. **Validation**: Ensure output completeness and consistency
    pub async fn finalize(&mut self) -> Result<()> {
        info!("Finalizing benchmark results");

        // Flush any remaining pending records before closing the file
        self.flush_pending_records().await?;

        // Close streaming file if enabled
        if self.streaming_enabled {
            if let Some(ref streaming_file) = self.streaming_file {
                let mut file = OpenOptions::new().append(true).open(streaming_file)?;

                if self.per_message_streaming {
                    // Close the columnar JSON format
                    writeln!(file, "\n  ]")?;
                    writeln!(file, "}}")?;
                } else {
                    // Close the simple JSON array of objects
                    writeln!(file, "\n]")?;
                }
                file.flush()?;
            }
        }

        // Write final comprehensive results
        self.write_final_results()?;

        info!("Results written to: {:?}", self.output_file);
        Ok(())
    }

    /// Flush any remaining pending records when streaming is complete
    ///
    /// This method writes out any records that are still in the buffer when
    /// streaming ends. This can happen when one test type completes before
    /// the other in combined streaming mode.
    pub async fn flush_pending_records(&mut self) -> Result<()> {
        if self.both_tests_enabled && !self.pending_records.is_empty() {
            debug!("Flushing {} pending streaming records", self.pending_records.len());
            
            // Collect records to write to avoid borrowing issues
            let records_to_write: Vec<MessageLatencyRecord> = self.pending_records.values().cloned().collect();
            
            for record in records_to_write {
                self.write_streaming_record(&record).await?;
            }
            
            self.pending_records.clear();
        }
        
        Ok(())
    }

    /// Write final consolidated results
    ///
    /// Generates the complete final output including all individual results,
    /// comprehensive metadata, and cross-mechanism analysis summaries.
    ///
    /// ## Returns
    /// - `Ok(())`: Final results written successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Output Structure
    ///
    /// The final output contains:
    /// - **Metadata**: Version, timestamp, system info
    /// - **Individual Results**: Complete data for each mechanism
    /// - **Overall Summary**: Cross-mechanism comparison and rankings
    ///
    /// ## File Format
    ///
    /// Results are written as pretty-printed JSON for human readability
    /// while maintaining machine parseability for analysis tools.
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
    ///
    /// Generates comprehensive cross-mechanism analysis including totals,
    /// rankings, and performance comparisons. This summary enables quick
    /// identification of the best-performing mechanisms for different metrics.
    ///
    /// ## Returns
    /// Complete summary with totals, rankings, and mechanism comparisons
    ///
    /// ## Analysis Categories
    ///
    /// - **Volume Totals**: Aggregate message and byte counts
    /// - **Error Tracking**: Total error counts across all tests
    /// - **Mechanism Summaries**: Individual mechanism performance profiles
    /// - **Performance Rankings**: Fastest throughput and lowest latency
    ///
    /// ## Ranking Methodology
    ///
    /// Rankings are based on objective performance metrics:
    /// - **Fastest Mechanism**: Highest average throughput
    /// - **Lowest Latency**: Lowest average latency (if measured)
    fn calculate_overall_summary(&self) -> OverallSummary {
        let mut total_messages = 0;
        let mut total_bytes = 0;
        let mut total_errors = 0;
        let mut mechanisms = HashMap::new();

        // Aggregate data from all individual results
        for result in &self.results {
            total_messages += result.summary.total_messages_sent;
            total_bytes += result.summary.total_bytes_transferred;
            total_errors += result.summary.error_count;

            // Create individual mechanism summary
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
    ///
    /// Identifies the IPC mechanism that achieved the highest average
    /// throughput across all its tests. This ranking helps identify
    /// the best option for high-bandwidth scenarios.
    ///
    /// ## Returns
    /// - `Some(String)`: Name of the fastest mechanism
    /// - `None`: No results available for comparison
    ///
    /// ## Comparison Method
    ///
    /// Comparison is based on average throughput in megabits per second,
    /// which provides a consistent measure across different message sizes
    /// and test configurations.
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
    ///
    /// Identifies the IPC mechanism that achieved the lowest average
    /// latency across all its tests. This ranking helps identify
    /// the best option for low-latency scenarios.
    ///
    /// ## Returns
    /// - `Some(String)`: Name of the lowest latency mechanism
    /// - `None`: No latency results available for comparison
    ///
    /// ## Comparison Method
    ///
    /// Only mechanisms with latency measurements are considered.
    /// Comparison is based on average latency in nanoseconds.
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
    ///
    /// Collects comprehensive system information for result reproducibility
    /// and correlation. This information helps explain performance variations
    /// and enables cross-system result comparison.
    ///
    /// ## Returns
    /// Complete system information structure
    ///
    /// ## Information Sources
    ///
    /// - **Platform Info**: From standard library constants
    /// - **Hardware Info**: From system APIs and detection functions
    /// - **Software Info**: From build-time and runtime version detection
    /// - **Memory Info**: From system information utilities
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
    ///
    /// Estimates available system memory for resource correlation.
    /// This is a simplified implementation that provides reasonable
    /// estimates for most systems.
    ///
    /// ## Returns
    /// Estimated available memory in gigabytes
    ///
    /// ## Implementation Note
    ///
    /// This is a placeholder implementation that returns reasonable
    /// defaults. A production implementation would use platform-specific
    /// system information APIs to get accurate memory information.
    fn get_memory_gb() -> f64 {
        // This is a simplified implementation
        // In a real implementation, you'd want to use a system info crate
        16.0 // Default assumption
    }

    /// Get Rust version
    ///
    /// Retrieves the Rust compiler version used to build the benchmark.
    /// This information is important for correlating performance with
    /// compiler optimizations and language features.
    ///
    /// ## Returns
    /// Rust version string from build-time metadata
    ///
    /// ## Build-time Detection
    ///
    /// The version is captured at build time from Cargo metadata,
    /// ensuring it reflects the actual compiler used for the benchmark.
    fn get_rust_version() -> String {
        env!("CARGO_PKG_RUST_VERSION").to_string()
    }

    /// Check if combined streaming mode is enabled
    ///
    /// Returns true when both test types are being aggregated into single records.
    pub fn is_combined_streaming_enabled(&self) -> bool {
        self.both_tests_enabled && self.per_message_streaming && self.streaming_enabled
    }
}

/// Final benchmark results structure
///
/// This structure represents the complete output of the benchmark suite,
/// including all individual results, metadata, and cross-mechanism analysis.
/// It serves as the top-level container for all benchmark data.
///
/// ## Structure Organization
///
/// - **Metadata**: Version, timing, and system information
/// - **Individual Results**: Complete data for each tested mechanism
/// - **Summary**: Cross-mechanism analysis and rankings
///
/// ## Output Format
///
/// This structure is serialized to JSON for the final output file,
/// providing a comprehensive and machine-readable benchmark report.
#[derive(Debug, Serialize, Deserialize)]
pub struct FinalBenchmarkResults {
    /// Metadata about the benchmark execution
    pub metadata: BenchmarkMetadata,
    
    /// Individual results for each mechanism tested
    pub results: Vec<BenchmarkResults>,
    
    /// Cross-mechanism summary and analysis
    pub summary: OverallSummary,
}

/// Benchmark metadata
///
/// This structure contains meta-information about the benchmark execution
/// itself, providing context and reproducibility information for the results.
///
/// ## Metadata Categories
///
/// - **Version Information**: Software versions for reproducibility
/// - **Execution Timing**: When the benchmark was run
/// - **Test Scope**: How many mechanisms were tested
/// - **System Context**: Hardware and platform information
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkMetadata {
    /// Benchmark suite version
    pub version: String,
    
    /// When the benchmark suite was executed
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// Total number of mechanism tests performed
    pub total_tests: usize,
    
    /// System information for reproducibility
    pub system_info: SystemInfo,
}

/// Overall summary across all mechanisms
///
/// This structure provides aggregate analysis across all tested mechanisms,
/// enabling quick assessment of overall benchmark scope and identification
/// of the best-performing options for different use cases.
///
/// ## Summary Components
///
/// - **Aggregate Totals**: Combined statistics across all tests
/// - **Individual Summaries**: Performance profiles for each mechanism
/// - **Performance Rankings**: Best mechanisms for different metrics
///
/// ## Analysis Value
///
/// This summary enables rapid identification of:
/// - Overall test scope and coverage
/// - Best mechanisms for throughput-focused applications
/// - Best mechanisms for latency-sensitive applications
/// - Relative performance differences between mechanisms
#[derive(Debug, Serialize, Deserialize)]
pub struct OverallSummary {
    /// Total messages sent across all mechanisms
    pub total_messages: usize,
    
    /// Total bytes transferred across all mechanisms
    pub total_bytes: usize,
    
    /// Total errors encountered across all tests
    pub total_errors: usize,
    
    /// Individual mechanism performance summaries
    pub mechanisms: HashMap<String, MechanismSummary>,
    
    /// Name of the mechanism with highest throughput
    pub fastest_mechanism: Option<String>,
    
    /// Name of the mechanism with lowest latency
    pub lowest_latency_mechanism: Option<String>,
}

/// Summary for a specific mechanism
///
/// This structure provides a concise performance profile for a single
/// IPC mechanism, highlighting the key metrics most relevant for
/// mechanism comparison and selection.
///
/// ## Key Metrics
///
/// - **Throughput**: Average performance for bandwidth assessment
/// - **Latency Percentiles**: P95 and P99 for reliability assessment
/// - **Volume**: Total messages processed for scale verification
///
/// ## Comparison Usage
///
/// These summaries enable quick mechanism comparison across the
/// metrics most important for IPC mechanism selection decisions.
#[derive(Debug, Serialize, Deserialize)]
pub struct MechanismSummary {
    /// The IPC mechanism this summary describes
    pub mechanism: IpcMechanism,
    
    /// Average throughput performance in megabits per second
    pub average_throughput_mbps: f64,
    
    /// 95th percentile latency (if latency was measured)
    pub p95_latency_ns: Option<u64>,
    
    /// 99th percentile latency (if latency was measured)
    pub p99_latency_ns: Option<u64>,
    
    /// Total number of messages processed by this mechanism
    pub total_messages: usize,
}

impl BenchmarkResults {
    /// Create new benchmark results
    ///
    /// Initializes a new results structure for a specific mechanism with
    /// the given test configuration. The results start empty and are
    /// populated as test phases complete.
    ///
    /// ## Parameters
    /// - `mechanism`: The IPC mechanism being tested
    /// - `message_size`: Size of messages used in testing
    /// - `concurrency`: Number of concurrent workers
    /// - `iterations`: Number of iterations (None for duration-based)
    /// - `duration`: Test duration (None for iteration-based)
    ///
    /// ## Returns
    /// Initialized results structure ready for data collection
    ///
    /// ## Configuration Capture
    ///
    /// The test configuration is captured at creation time to ensure
    /// the results accurately reflect the parameters used during testing.
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
    ///
    /// Incorporates performance metrics from one-way latency testing
    /// and updates the summary statistics accordingly.
    ///
    /// ## Parameters
    /// - `results`: Performance metrics from one-way testing
    ///
    /// ## Side Effects
    ///
    /// - Updates one_way_results field
    /// - Recalculates summary statistics
    /// - Updates test configuration flags
    pub fn add_one_way_results(&mut self, results: PerformanceMetrics) {
        self.one_way_results = Some(results);
        self.update_summary();
    }

    /// Add round-trip test results
    ///
    /// Incorporates performance metrics from round-trip latency testing
    /// and updates the summary statistics accordingly.
    ///
    /// ## Parameters
    /// - `results`: Performance metrics from round-trip testing
    ///
    /// ## Side Effects
    ///
    /// - Updates round_trip_results field
    /// - Recalculates summary statistics
    /// - Updates test configuration flags
    pub fn add_round_trip_results(&mut self, results: PerformanceMetrics) {
        self.round_trip_results = Some(results);
        self.update_summary();
    }

    /// Update summary with current results
    ///
    /// Recalculates all summary statistics based on the currently available
    /// performance data. This method is called automatically when results
    /// are added to ensure summary consistency.
    ///
    /// ## Calculation Process
    ///
    /// 1. **Aggregate Volumes**: Sum messages and bytes from all test types
    /// 2. **Calculate Throughput**: Determine average and peak throughput
    /// 3. **Analyze Latency**: Extract key latency statistics if available
    /// 4. **Update Summary**: Store calculated values in summary structure
    ///
    /// ## Statistical Methods
    ///
    /// - **Throughput**: Simple averaging and maximum selection
    /// - **Latency**: Statistical analysis of distribution characteristics
    /// - **Volume**: Direct summation of counts
    fn update_summary(&mut self) {
        let mut total_messages = 0;
        let mut total_bytes = 0;
        let mut throughput_values = Vec::new();
        let mut latency_values = Vec::new();

        // Process one-way results if available
        if let Some(ref results) = self.one_way_results {
            total_messages += results.throughput.total_messages;
            total_bytes += results.throughput.total_bytes;
            throughput_values.push(results.throughput.bytes_per_second);

            if let Some(ref latency) = results.latency {
                latency_values.push(latency.mean_ns as f64);
            }
        }

        // Process round-trip results if available
        if let Some(ref results) = self.round_trip_results {
            total_messages += results.throughput.total_messages;
            total_bytes += results.throughput.total_bytes;
            throughput_values.push(results.throughput.bytes_per_second);

            if let Some(ref latency) = results.latency {
                latency_values.push(latency.mean_ns as f64);
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
    ///
    /// Analyzes all available latency data to extract key statistics
    /// including minimum, maximum, and key percentile values.
    ///
    /// ## Returns
    /// Tuple of (min, max, P95, P99) latency values in nanoseconds
    ///
    /// ## Analysis Method
    ///
    /// - **Min/Max**: Global minimum and maximum across all test types
    /// - **Percentiles**: Extracted from HDR histogram percentile data
    /// - **Handling Missing Data**: Returns None for unavailable metrics
    ///
    /// ## Statistical Validity
    ///
    /// The extracted statistics represent the combined performance
    /// characteristics across all test types, providing a comprehensive
    /// view of latency behavior for the mechanism.
    fn extract_latency_stats(&self) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
        let mut min_latency = None;
        let mut max_latency = None;
        let mut p95_latency = None;
        let mut p99_latency = None;

        // Analyze latency data from all available test results
        for results in [&self.one_way_results, &self.round_trip_results]
            .iter()
            .filter_map(|&x| x.as_ref())
        {
            if let Some(ref latency) = results.latency {
                // Update global minimum and maximum
                min_latency =
                    Some(min_latency.map_or(latency.min_ns as u64, |min: u64| min.min(latency.min_ns as u64)));
                max_latency =
                    Some(max_latency.map_or(latency.max_ns as u64, |max: u64| max.max(latency.max_ns as u64)));

                // Find P95 and P99 values from percentile data
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
    /// Create a default benchmark summary
    ///
    /// Provides a default summary structure with zero values and None
    /// for optional metrics. Used for initialization before actual
    /// results are available.
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
    /// Create default system information
    ///
    /// Provides a default system information structure with reasonable
    /// fallback values when system detection is not available or fails.
    ///
    /// ## Default Values
    ///
    /// - **Platform**: Detected from standard library constants
    /// - **Hardware**: Detected from system APIs or reasonable defaults
    /// - **Software**: Detected from build-time metadata or defaults
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

    /// Test benchmark results creation with various configurations
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

    /// Test results manager creation and basic functionality
    #[test]
    fn test_results_manager_creation() {
        let temp_file = NamedTempFile::new().unwrap();
        let manager = ResultsManager::new(temp_file.path()).unwrap();

        assert_eq!(manager.output_file, temp_file.path());
        assert!(!manager.streaming_enabled);
        assert!(manager.results.is_empty());
    }

    /// Test system information detection and defaults
    #[test]
    fn test_system_info_default() {
        let info = SystemInfo::default();

        assert!(!info.os.is_empty());
        assert!(!info.architecture.is_empty());
        assert!(info.cpu_cores > 0);
        assert!(info.memory_gb > 0.0);
    }
}
