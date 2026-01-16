#![allow(clippy::needless_return)]
//! # Blocking Results and Output Management Module
//!
//! This module provides comprehensive result collection, analysis, and output
//! capabilities for the IPC benchmark suite in blocking mode. It handles both
//! aggregated final results and real-time streaming output for monitoring
//! long-running tests using synchronous I/O operations.
//!
//! ## Key Components
//!
//! - **BlockingResultsManager**: Coordination of output and streaming
//!   operations using blocking I/O
//!
//! ## Relationship to ResultsManager
//!
//! This module provides the same functionality as `results::ResultsManager`
//! but uses pure synchronous/blocking I/O operations. It reuses the data
//! structures from the `results` module (`BenchmarkResults`, `MessageLatencyRecord`,
//! etc.) but provides blocking I/O implementations of all file operations.
//!
//! ## Output Formats
//!
//! The module supports two primary output modes:
//! - **Final Results**: Comprehensive JSON report with metadata and analysis
//! - **Streaming Results**: Real-time per-message latency data during execution
//!
//! ## Blocking Behavior
//!
//! All I/O operations block the calling thread until complete:
//! - File writes block until data is written to disk
//! - File flushes block until buffers are synchronized
//! - No async/await or Tokio runtime required

use crate::results::{
    BenchmarkMetadata, BenchmarkResults, FinalBenchmarkResults, MechanismSummary,
    MessageLatencyRecord, OverallSummary, SystemInfo,
};
use anyhow::Result;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use tracing::{debug, info};

/// Blocking results manager for handling output and streaming
///
/// The `BlockingResultsManager` coordinates all result collection, processing,
/// and output operations using synchronous/blocking I/O. It manages both final
/// result generation and optional real-time streaming for long-running benchmarks.
///
/// ## Management Responsibilities
///
/// - **Collection**: Accumulate results from multiple benchmark runs
/// - **Streaming**: Optionally write results in real-time during execution
/// - **Aggregation**: Calculate cross-mechanism summaries and comparisons
/// - **Output**: Generate comprehensive final reports in structured format
/// - **Validation**: Ensure result consistency and completeness
///
/// ## Blocking I/O
///
/// All file operations use standard library blocking I/O (`std::fs::File`,
/// `std::io::Write`). Operations block the calling thread until complete.
/// No async/await or Tokio runtime is required.
///
/// ## Streaming vs Final Output
///
/// - **Streaming**: Incremental JSON array written during execution
/// - **Final**: Complete structure with metadata and summaries
///
/// The streaming format allows monitoring of long-running benchmarks,
/// while the final format provides complete analysis and comparison.
///
/// ## Examples
///
/// ```rust
/// use ipc_benchmark::results_blocking::BlockingResultsManager;
/// use std::path::Path;
///
/// # fn example() -> anyhow::Result<()> {
/// // Create manager with output file
/// let mut manager = BlockingResultsManager::new(
///     Some(Path::new("results.json")),
///     None,
/// )?;
///
/// // Enable per-message streaming
/// manager.enable_per_message_streaming(Path::new("streaming.json"))?;
///
/// // Add results and finalize
/// // manager.add_results(results)?;
/// // manager.finalize()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct BlockingResultsManager {
    /// Optional path for final results output
    output_file: Option<std::path::PathBuf>,

    /// Optional path for log file output
    log_file: Option<String>,

    /// Optional path for streaming results output
    streaming_file: Option<std::path::PathBuf>,

    /// Optional path for streaming CSV results output
    streaming_csv_file: Option<std::path::PathBuf>,

    /// Optional open file handle (buffered) for streaming JSON output.
    streaming_file_handle: Option<BufWriter<File>>,

    /// Optional open file handle (buffered) for streaming CSV output.
    streaming_csv_handle: Option<BufWriter<File>>,

    /// Accumulated results from all benchmark runs
    results: Vec<BenchmarkResults>,

    /// Whether streaming is enabled
    streaming_enabled: bool,

    /// Whether CSV streaming is enabled
    csv_streaming_enabled: bool,

    /// Whether to stream per-message latency records instead of final results
    per_message_streaming: bool,

    /// Counter for tracking if this is the first record being streamed
    first_record_streamed: bool,

    /// Whether both one-way and round-trip tests are enabled (for record
    /// aggregation)
    both_tests_enabled: bool,

    /// Buffer for collecting records when both tests are running (keyed by
    /// message ID)
    pending_records: HashMap<u64, MessageLatencyRecord>,
}

impl BlockingResultsManager {
    /// Create a new blocking results manager
    ///
    /// Initializes a results manager with the specified output file path.
    /// Streaming is disabled by default and can be enabled separately.
    ///
    /// ## Parameters
    /// - `output_file`: Optional path to the final JSON results file.
    /// - `log_file`: Optional path to the log file.
    ///
    /// ## Returns
    /// - `Ok(BlockingResultsManager)`: Configured manager ready for use
    /// - `Err(anyhow::Error)`: File path validation or access error
    ///
    /// ## File Handling
    ///
    /// The output file is not created until `finalize()` is called,
    /// allowing validation of the path without affecting existing files.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let manager = BlockingResultsManager::new(
    ///     Some(Path::new("results.json")),
    ///     Some("benchmark.log"),
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(output_file: Option<&Path>, log_file: Option<&str>) -> Result<Self> {
        Ok(Self {
            output_file: output_file.map(|p| p.to_path_buf()),
            log_file: log_file.map(|s| s.to_string()),
            streaming_file: None,
            streaming_csv_file: None,
            streaming_file_handle: None,
            streaming_csv_handle: None,
            results: Vec::new(),
            streaming_enabled: false,
            csv_streaming_enabled: false,
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
    /// ## Blocking Behavior
    ///
    /// File operations block until complete. This method blocks during
    /// file creation and initial setup.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let mut manager = BlockingResultsManager::new(None, None)?;
    /// manager.enable_streaming(Path::new("streaming.json"))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_streaming(&mut self, streaming_file: &Path) -> Result<()> {
        self.streaming_file = Some(streaming_file.to_path_buf());
        self.streaming_enabled = true;

        // Create the file with JSON array opening bracket
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.streaming_file.as_ref().unwrap())?;
        write!(file, "[")?;
        file.flush()?;

        debug!("Enabled streaming to: {:?}", self.streaming_file);
        Ok(())
    }

    /// Enable per-message latency streaming
    ///
    /// Configures real-time per-message latency streaming for detailed
    /// latency monitoring for analysis and debugging.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where per-message latency records will
    ///   be written
    ///
    /// ## Returns
    /// - `Ok(())`: Per-message streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    ///
    /// ## Streaming Format
    ///
    /// The streaming file contains a JSON object with columnar data:
    /// ```json
    /// {
    ///   "headings": ["timestamp_ns", "message_id", ...],
    ///   "data": [
    ///     [1640995200000000000, 1, "UnixDomainSocket", 1024, 50000, null],
    ///     ...
    ///   ]
    /// }
    /// ```
    ///
    /// ## Blocking Behavior
    ///
    /// File operations block until complete. This method blocks during
    /// file creation and initial setup.
    ///
    /// ## Performance Considerations
    ///
    /// Per-message streaming generates significant I/O activity and should
    /// be used carefully in high-throughput scenarios to avoid affecting
    /// benchmark results.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let mut manager = BlockingResultsManager::new(None, None)?;
    /// manager.enable_per_message_streaming(
    ///     Path::new("per_message.json")
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_per_message_streaming(&mut self, streaming_file: &Path) -> Result<()> {
        self.streaming_file = Some(streaming_file.to_path_buf());
        self.streaming_enabled = true;
        self.per_message_streaming = true;
        self.first_record_streamed = true;

        // Truncate and write header, then open append handle for repeated
        // writes.
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(self.streaming_file.as_ref().unwrap())?;

            // Write JSON object opening and headings array
            writeln!(file, "{{")?;
            let headings_json = serde_json::to_string(MessageLatencyRecord::HEADINGS)?;
            writeln!(file, "  \"headings\": {},", headings_json)?;
            write!(file, "  \"data\": [")?;
            file.flush()?;
        }

        // Open append handle and store buffered writer for repeated writes.
        let append_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.streaming_file.as_ref().unwrap())?;
        self.streaming_file_handle = Some(BufWriter::new(append_file));

        debug!(
            "Enabled per-message streaming to: {:?}",
            self.streaming_file
        );
        Ok(())
    }

    /// Enable combined streaming mode when both one-way and round-trip tests
    /// are running
    ///
    /// When this mode is enabled, streaming records for the same message ID
    /// from both test types will be aggregated into single records with both
    /// latency values.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where combined latency records will be written
    /// - `both_tests_enabled`: Whether both one-way and round-trip tests are
    ///   running
    ///
    /// ## Returns
    /// - `Ok(())`: Combined streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let mut manager = BlockingResultsManager::new(None, None)?;
    /// manager.enable_combined_streaming(
    ///     Path::new("combined.json"),
    ///     true, // both tests enabled
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_combined_streaming(
        &mut self,
        streaming_file: &Path,
        both_tests_enabled: bool,
    ) -> Result<()> {
        self.enable_per_message_streaming(streaming_file)?;
        self.both_tests_enabled = both_tests_enabled;

        debug!(
            "Enabled combined streaming mode: both_tests={}",
            both_tests_enabled
        );
        Ok(())
    }

    /// Set combined mode for record merging
    ///
    /// When set to true, records with the same message ID will be merged
    /// so that one-way and round-trip latencies appear in the same record.
    pub fn set_combined_mode(&mut self, enabled: bool) {
        self.both_tests_enabled = enabled;
    }

    /// Enable CSV latency streaming to a file
    ///
    /// Configures real-time per-message latency streaming in CSV format.
    /// This provides a simple, efficient format for capturing individual
    /// message timing data for analysis in spreadsheets or data processing
    /// tools.
    ///
    /// ## Parameters
    /// - `streaming_file`: Path where CSV latency records will be written
    ///
    /// ## Returns
    /// - `Ok(())`: CSV streaming enabled successfully
    /// - `Err(anyhow::Error)`: File creation or write permission error
    ///
    /// ## CSV Format
    ///
    /// The file contains a header row followed by data rows:
    /// ```csv
    /// timestamp_ns,message_id,mechanism,message_size,one_way_latency_ns,round_trip_latency_ns
    /// 1640995200000000000,1,UnixDomainSocket,1024,50000,
    /// ...
    /// ```
    ///
    /// ## Blocking Behavior
    ///
    /// File operations block until complete. This method blocks during
    /// file creation and header write.
    ///
    /// ## Examples
    ///
    /// ```rust
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use std::path::Path;
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// let mut manager = BlockingResultsManager::new(None, None)?;
    /// manager.enable_csv_streaming(Path::new("latency.csv"))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn enable_csv_streaming(&mut self, streaming_file: &Path) -> Result<()> {
        self.streaming_csv_file = Some(streaming_file.to_path_buf());
        self.csv_streaming_enabled = true;
        // Enable per-message streaming so that records are actually written
        self.per_message_streaming = true;
        self.streaming_enabled = true;

        // Truncate and write header, then open append handle for repeated
        // writes.
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(self.streaming_csv_file.as_ref().unwrap())?;
            let header = MessageLatencyRecord::HEADINGS.join(",");
            writeln!(file, "{}", header)?;
            file.flush()?;
        }

        let append_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.streaming_csv_file.as_ref().unwrap())?;
        self.streaming_csv_handle = Some(BufWriter::new(append_file));

        debug!("Enabled CSV streaming to: {:?}", self.streaming_csv_file);
        Ok(())
    }

    /// Stream a single message latency record (blocking)
    ///
    /// Immediately writes a single message latency record to the streaming
    /// file using blocking I/O. This method is called for each message during
    /// benchmark execution when per-message streaming is enabled.
    ///
    /// ## Parameters
    /// - `record`: Message latency record to stream
    ///
    /// ## Returns
    /// - `Ok(())`: Record streamed successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the record is written and flushed to disk.
    ///
    /// ## JSON Formatting
    ///
    /// Records are written with proper JSON array formatting including
    /// comma separation between elements.
    pub fn stream_latency_record(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        // If streaming to per-message output is enabled, use write_streaming_record_direct
        // which handles both combined records and partial records that need to be merged.
        if self.per_message_streaming {
            self.write_streaming_record_direct(record)?;
            return Ok(());
        }

        // Non-streaming mode: aggregate in-memory for finalize().
        // Clone here because pending_records owns MessageLatencyRecord values.
        if self.both_tests_enabled {
            if let Some(existing) = self.pending_records.get_mut(&record.message_id) {
                existing.merge(record);
            } else {
                self.pending_records
                    .insert(record.message_id, record.clone());
            }
        } else {
            self.pending_records
                .insert(record.message_id, record.clone());
        }

        Ok(())
    }

    /// Write a streaming record to the file (blocking)
    ///
    /// Internal helper method that handles the actual file I/O for streaming
    /// records using blocking operations.
    ///
    /// ## Parameters
    /// - `record`: Message latency record to write
    ///
    /// ## Returns
    /// - `Ok(())`: Record written successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the record is written and flushed to disk.
    fn write_streaming_record(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        // Prefer a kept-open buffered writer when available to avoid repeated
        // open/append syscalls.
        if let Some(ref mut writer) = self.streaming_file_handle {
            if !self.first_record_streamed {
                writer.write_all(b",\n")?;
            } else {
                writer.write_all(b"\n")?;
            }
            let values = record.to_value_array();
            let json = serde_json::to_string(&values)?;
            writer.write_all(b"    ")?; // indent
            writer.write_all(json.as_bytes())?;
            writer.flush()?;
            self.first_record_streamed = false;
        } else if let Some(ref streaming_file) = self.streaming_file {
            // Fallback: open, append, write
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(streaming_file)?;
            let mut buf = std::io::BufWriter::new(f);

            // Add comma if not first record (proper JSON array formatting)
            if !self.first_record_streamed {
                // write a comma and newline between records
                buf.write_all(b",\n")?;
            } else {
                // For the first record, add a newline to separate from the
                // "data": [ line
                buf.write_all(b"\n")?;
            }

            // Write JSON array of values (compact representation) with
            // indentation for readability
            let values = record.to_value_array();
            let json = serde_json::to_string(&values)?;
            buf.write_all(b"    ")?; // indent
            buf.write_all(json.as_bytes())?;
            buf.flush()?;
            self.first_record_streamed = false;
        }

        // Stream to CSV if enabled
        if self.csv_streaming_enabled {
            if let Some(ref mut csv_writer) = self.streaming_csv_handle {
                let csv_record = record.to_csv_record();
                csv_writer.write_all(csv_record.as_bytes())?;
                csv_writer.write_all(b"\n")?;
                csv_writer.flush()?;
            } else if let Some(ref streaming_csv_file) = self.streaming_csv_file {
                let f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(streaming_csv_file)?;
                let mut buf = std::io::BufWriter::new(f);
                let csv_record = record.to_csv_record();
                buf.write_all(csv_record.as_bytes())?;
                buf.write_all(b"\n")?;
                buf.flush()?;
            }
        }

        Ok(())
    }

    /// Write a streaming record directly to file (bypassing aggregation)
    ///
    /// This method writes records directly to the streaming file without
    /// aggregation, used for combined tests where both latencies are measured
    /// for the same message simultaneously.
    ///
    /// ## Parameters
    /// - `record`: Message latency record to write
    ///
    /// ## Returns
    /// - `Ok(())`: Record written successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the record is written and flushed to disk.
    pub fn write_streaming_record_direct(&mut self, record: &MessageLatencyRecord) -> Result<()> {
        if !self.streaming_enabled || !self.per_message_streaming {
            return Ok(());
        }

        // If the record already contains both latencies, write it immediately.
        if record.is_combined() {
            self.write_streaming_record(record)?;
            return Ok(());
        }

        // When both one-way and round-trip tests are enabled we may receive
        // two partial records for the same message id. Buffer the first and
        // write only when its counterpart arrives (to produce a combined
        // record).
        if self.both_tests_enabled {
            if let Some(mut existing) = self.pending_records.remove(&record.message_id) {
                // Merge incoming partial into existing to form a combined
                // record.
                existing.merge(record);
                // Write the combined record out once.
                self.write_streaming_record(&existing)?;
            } else {
                // Store this partial record until the matching counterpart
                // arrives.
                self.pending_records
                    .insert(record.message_id, record.clone());
            }
            return Ok(());
        }

        // Not in combined mode: write immediately.
        self.write_streaming_record(record)
    }

    /// Add benchmark results (blocking)
    ///
    /// Incorporates results from a completed benchmark into the result
    /// collection using blocking I/O. If streaming is enabled, the results
    /// are immediately written to the streaming file for real-time monitoring.
    ///
    /// ## Parameters
    /// - `results`: Complete results from a single mechanism benchmark
    ///
    /// ## Returns
    /// - `Ok(())`: Results added successfully
    /// - `Err(anyhow::Error)`: Streaming write error or validation failure
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until any streaming writes are complete.
    ///
    /// ## Processing Steps
    ///
    /// 1. **Validation**: Ensure results are complete and consistent
    /// 2. **Streaming**: Write to streaming file if enabled (blocks until
    ///    complete)
    /// 3. **Storage**: Add to internal collection for final output
    /// 4. **Logging**: Record addition for monitoring and debugging
    ///
    /// ## Examples
    ///
    /// ```rust,no_run
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    /// use ipc_benchmark::results::BenchmarkResults;
    ///
    /// # fn example(results: BenchmarkResults) -> anyhow::Result<()> {
    /// let mut manager = BlockingResultsManager::new(None, None)?;
    /// manager.add_results(results)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn add_results(&mut self, results: BenchmarkResults) -> Result<()> {
        info!("Adding results for {} mechanism", results.mechanism);

        // Stream final results only if streaming is enabled but per-message
        // streaming is not
        if self.streaming_enabled && !self.per_message_streaming {
            self.stream_results(&results)?;
        }

        self.results.push(results);
        Ok(())
    }

    /// Stream results to file during execution (blocking)
    ///
    /// Writes a single benchmark result to the streaming file in JSON format
    /// using blocking I/O. This method handles proper JSON array formatting
    /// including comma separation between array elements.
    ///
    /// ## Parameters
    /// - `results`: Results to write to streaming file
    ///
    /// ## Returns
    /// - `Ok(())`: Results streamed successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the results are written and flushed to disk.
    ///
    /// ## JSON Formatting
    ///
    /// The method ensures proper JSON array formatting by adding commas
    /// between elements and pretty-printing each result for readability.
    fn stream_results(&self, results: &BenchmarkResults) -> Result<()> {
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

    /// Finalize results and write to output file (blocking)
    ///
    /// Completes the result collection process by generating comprehensive
    /// final output including metadata, cross-mechanism summaries, and
    /// closing any streaming files using blocking I/O operations.
    ///
    /// ## Returns
    /// - `Ok(())`: Finalization completed successfully
    /// - `Err(anyhow::Error)`: File write or analysis error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until all file operations are complete:
    /// - Flushing pending records blocks until written
    /// - Closing streaming files blocks until flushed
    /// - Writing final results blocks until complete
    ///
    /// ## Finalization Steps
    ///
    /// 1. **Close Streaming**: Complete streaming JSON array if enabled
    ///    (blocks until complete)
    /// 2. **Generate Summaries**: Calculate cross-mechanism comparisons
    /// 3. **Create Metadata**: Gather system and version information
    /// 4. **Write Final Output**: Generate comprehensive JSON report
    ///    (blocks until complete)
    /// 5. **Validation**: Ensure output completeness and consistency
    ///
    /// ## Examples
    ///
    /// ```rust,no_run
    /// use ipc_benchmark::results_blocking::BlockingResultsManager;
    ///
    /// # fn example(mut manager: BlockingResultsManager) -> anyhow::Result<()> {
    /// // After adding all results
    /// manager.finalize()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn finalize(&mut self) -> Result<()> {
        info!("Finalizing benchmark results");

        // Flush any remaining pending records before closing the file.
        // Best-effort: if flushing fails, log and continue with
        // closing/finalization.
        if let Err(e) = self.flush_pending_records() {
            debug!("flush_pending_records failed during finalize: {:?}", e);
        }

        // Close streaming JSON file if enabled (best-effort).
        if let Err(e) = self.close_streaming_json() {
            debug!("close_streaming_json failed during finalize: {:?}", e);
        }

        // Close streaming CSV file if enabled (best-effort).
        if let Err(e) = self.close_streaming_csv() {
            debug!("close_streaming_csv failed during finalize: {:?}", e);
        }

        // Write final comprehensive results if an output file was specified.
        // Writing final results is important; return error to caller if it
        // fails.
        if let Some(output_file) = &self.output_file {
            self.write_final_results(output_file)?;
        }

        Ok(())
    }

    /// Closes the streaming JSON file, writing the final closing brackets
    /// (blocking)
    ///
    /// This function is called by `finalize()` to ensure that the output
    /// is a valid JSON document. It opens the file in append mode and
    /// writes the necessary closing syntax using blocking I/O.
    ///
    /// ## Returns
    /// - `Ok(())`: File closed successfully
    /// - `Err(anyhow::Error)`: File write error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the closing brackets are written and
    /// flushed to disk.
    fn close_streaming_json(&mut self) -> Result<()> {
        if self.per_message_streaming {
            info!("Closing streaming JSON file.");
            // Prefer buffered handle if present
            if let Some(mut writer) = self.streaming_file_handle.take() {
                writer.write_all(b"\n  ]\n}\n")?;
                writer.flush()?;
            } else if let Some(streaming_file) = &self.streaming_file {
                let mut file = OpenOptions::new().append(true).open(streaming_file)?;
                // Write the closing brackets for the JSON data array and the
                // root object.
                write!(file, "\n  ]\n}}\n")?;
                file.flush()?;
            }
        } else if self.streaming_enabled {
            // This handles the case of non-per-message streaming, which is a
            // simple JSON array.
            if let Some(streaming_file) = &self.streaming_file {
                info!("Closing streaming JSON file.");
                if let Some(mut writer) = self.streaming_file_handle.take() {
                    writer.write_all(b"\n]\n")?;
                    writer.flush()?;
                } else {
                    let mut file = OpenOptions::new().append(true).open(streaming_file)?;
                    // Write the closing bracket for the JSON array.
                    write!(file, "\n]\n")?;
                    file.flush()?;
                }
            }
        }
        Ok(())
    }

    /// Closes the streaming CSV file (blocking)
    ///
    /// This function is called by `finalize()` to ensure that all buffered
    /// data is written to the file using blocking I/O. Since each write is
    /// flushed immediately, this function primarily serves for logging and
    /// symmetry.
    ///
    /// ## Returns
    /// - `Ok(())`: File closed successfully
    /// - `Err(anyhow::Error)`: File flush error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the file buffer is flushed to disk.
    fn close_streaming_csv(&mut self) -> Result<()> {
        if self.csv_streaming_enabled {
            if let Some(path) = &self.streaming_csv_file {
                info!("Finalizing streaming CSV file at {}", path.display());
                if let Some(mut csv_writer) = self.streaming_csv_handle.take() {
                    csv_writer.flush()?;
                }
            }
        }
        Ok(())
    }

    /// Flush any remaining pending records when streaming is complete
    /// (blocking)
    ///
    /// In combined streaming mode, a one-way result might be recorded
    /// before the corresponding round-trip result is available. This method
    /// ensures that any such pending records are written out to the streaming
    /// file before finalization using blocking I/O.
    ///
    /// ## Returns
    /// - `Ok(())`: Records flushed successfully
    /// - `Err(anyhow::Error)`: File write error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until all pending records are written and flushed
    /// to disk.
    pub fn flush_pending_records(&mut self) -> Result<()> {
        if self.both_tests_enabled && !self.pending_records.is_empty() {
            debug!(
                "Flushing {} pending streaming records",
                self.pending_records.len()
            );

            // Collect (id, record) pairs and sort to ensure deterministic
            // output order.
            let mut records_to_write: Vec<(u64, MessageLatencyRecord)> = self
                .pending_records
                .iter()
                .map(|(&id, r)| (id, r.clone()))
                .collect();
            records_to_write.sort_by_key(|(id, _)| *id);

            for (_id, record) in records_to_write {
                self.write_streaming_record(&record)?;
            }

            self.pending_records.clear();
        }

        Ok(())
    }

    /// Write final consolidated results (blocking)
    ///
    /// Generates the complete final output including all individual results,
    /// comprehensive metadata, and cross-mechanism analysis summaries using
    /// blocking I/O operations.
    ///
    /// ## Parameters
    /// - `output_file`: Path where final results will be written
    ///
    /// ## Returns
    /// - `Ok(())`: Final results written successfully
    /// - `Err(anyhow::Error)`: File write or JSON serialization error
    ///
    /// ## Blocking Behavior
    ///
    /// This method blocks until the final results are serialized, written,
    /// and the file is atomically renamed into place.
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
    ///
    /// ## Atomic Write
    ///
    /// The method writes to a temporary file first, then atomically renames
    /// it to avoid corrupting existing files on error.
    fn write_final_results(&self, output_file: &Path) -> Result<()> {
        info!("Writing final results to: {:?}", output_file);

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

        // Serialize pretty JSON
        let json = serde_json::to_string_pretty(&final_results)?;

        // Atomic write: write to a temp file next to the target, then rename.
        // Use a ".partial" extension to avoid clobbering the destination on
        // failure.
        let tmp = output_file.with_extension("partial");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, output_file)?;

        info!("Successfully wrote final results to: {:?}", output_file);
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
    /// Returns true when both test types are being aggregated into single
    /// records.
    ///
    /// ## Returns
    /// - `true`: Combined streaming is enabled
    /// - `false`: Combined streaming is not enabled
    pub fn is_combined_streaming_enabled(&self) -> bool {
        self.both_tests_enabled && self.per_message_streaming && self.streaming_enabled
    }

    /// Prints a human-readable summary of the benchmark run to the console.
    ///
    /// This method formats a summary that is displayed at the end of the
    /// benchmark execution. It lists all the output files that were generated
    /// during the run, matching the format of the configuration summary.
    ///
    /// ## Returns
    /// - `Ok(())`: Summary printed successfully
    /// - `Err(anyhow::Error)`: Print error (rare)
    pub fn print_summary(&self) -> Result<()> {
        println!("\nBenchmark Results:");
        println!("-----------------------------------------------------------------");

        println!("  Output Files Written:");
        if let Some(path) = &self.output_file {
            println!("    Final JSON Results:   {}", path.display());
        }
        if let Some(path) = &self.streaming_file {
            println!("    Streaming JSON:       {}", path.display());
        }
        if let Some(path) = &self.streaming_csv_file {
            println!("    Streaming CSV:        {}", path.display());
        }
        if let Some(path) = &self.log_file {
            println!("    Log File:             {}", path);
        }
        println!("-----------------------------------------------------------------");

        if self.results.is_empty() {
            println!("No benchmark results to display.");
        } else {
            for result in &self.results {
                println!("Mechanism: {}", result.mechanism);
                println!("  Message Size: {} bytes", result.test_config.message_size);
                println!("  Buffer Size:  {} bytes", result.test_config.buffer_size);

                match &result.status {
                    crate::results::BenchmarkStatus::Success => {
                        Self::print_summary_details(result, "  ");
                    }
                    crate::results::BenchmarkStatus::Failure(error_msg) => {
                        println!("  Status: FAILED");
                        println!("    Error: {}", error_msg);
                    }
                }
                println!("-----------------------------------------------------------------");
            }
        }

        std::io::Write::flush(&mut std::io::stdout())?;
        Ok(())
    }

    /// Helper function to format and print the details from LatencyMetrics.
    ///
    /// This function takes latency metrics and prints a formatted summary,
    /// including mean, P95, P99, min, and max values. It's used to create
    /// separate, clearly labeled sections for one-way and round-trip latencies.
    fn print_latency_details(latency: &crate::metrics::LatencyMetrics, indent: &str, title: &str) {
        let mut p95 = None;
        let mut p99 = None;
        for percentile in &latency.percentiles {
            if (percentile.percentile - 95.0).abs() < 0.1 {
                p95 = Some(percentile.value_ns);
            }
            if (percentile.percentile - 99.0).abs() < 0.1 {
                p99 = Some(percentile.value_ns);
            }
        }

        println!("{}{}:", indent, title);
        println!(
            "{}{:<8} Mean: {}, P95: {}, P99: {}",
            indent,
            "  ",
            format_latency(latency.mean_ns as u64),
            p95.map(format_latency).unwrap_or_else(|| "N/A".to_string()),
            p99.map(format_latency).unwrap_or_else(|| "N/A".to_string())
        );
        println!(
            "{}{:<8} Min:  {}, Max: {}",
            indent,
            "  ",
            format_latency(latency.min_ns),
            format_latency(latency.max_ns)
        );
    }

    /// Helper function to format and print the details from a BenchmarkResults struct.
    ///
    /// This function now prints separate, clearly labeled sections for one-way and
    /// round-trip latencies if they are present in the results. Throughput and
    /// total data transfer are printed in their own sections.
    fn print_summary_details(result: &BenchmarkResults, indent: &str) {
        if let Some(one_way) = &result.one_way_results {
            if let Some(latency) = &one_way.latency {
                Self::print_latency_details(latency, indent, "One-Way Latency");
            }
        }

        if let Some(round_trip) = &result.round_trip_results {
            if let Some(latency) = &round_trip.latency {
                Self::print_latency_details(latency, indent, "Round-Trip Latency");
            }
        }

        let summary = &result.summary;

        println!("{}Throughput:", indent);
        // Note: The summary field is named _mbps, but the calculation in update_summary
        // produces MB/s (Megabytes per second). The label reflects the calculation.
        println!(
            "{}{:<8} Average: {:.2} MB/s, Peak: {:.2} MB/s",
            indent, "  ", summary.average_throughput_mbps, summary.peak_throughput_mbps
        );

        println!("{}Totals:", indent);
        println!(
            "{}{:<8} Messages: {}, Data: {:.2} MB",
            indent,
            "  ",
            summary.total_messages_sent,
            summary.total_bytes_transferred as f64 / (1024.0 * 1024.0)
        );
    }
}

/// Helper function to format latency values (in nanoseconds) into a
/// human-readable string (us, ms).
fn format_latency(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1_000_000.0)
    } else if ns >= 1_000 {
        format!("{:.2} us", ns as f64 / 1_000.0)
    } else {
        format!("{} ns", ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::IpcMechanism;
    use crate::results::BenchmarkResults;
    use tempfile::TempDir;

    /// Helper function to create a test BenchmarkResults
    fn create_test_results() -> BenchmarkResults {
        // Use BenchmarkResults::new() which creates a properly structured
        // results object
        BenchmarkResults::new(
            IpcMechanism::TcpSocket,
            1024,       // message_size
            8192,       // buffer_size
            1,          // concurrency
            Some(1000), // msg_count
            None,       // duration
            0,          // warmup_iterations
            true,       // one_way
            true,       // round_trip
        )
    }

    #[test]
    fn test_new_manager() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("results.json");

        let manager = BlockingResultsManager::new(Some(&output_path), Some("test.log"));

        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert_eq!(manager.output_file, Some(output_path));
        assert_eq!(manager.log_file, Some("test.log".to_string()));
        assert!(!manager.streaming_enabled);
        assert!(!manager.csv_streaming_enabled);
        assert!(!manager.per_message_streaming);
    }

    #[test]
    fn test_enable_streaming() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("streaming.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        let result = manager.enable_streaming(&streaming_path);

        assert!(result.is_ok());
        assert!(manager.streaming_enabled);
        assert_eq!(manager.streaming_file, Some(streaming_path.clone()));

        // Verify file was created with opening bracket
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert_eq!(content, "[");
    }

    #[test]
    fn test_enable_per_message_streaming() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("per_message.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        let result = manager.enable_per_message_streaming(&streaming_path);

        assert!(result.is_ok());
        assert!(manager.streaming_enabled);
        assert!(manager.per_message_streaming);
        assert_eq!(manager.streaming_file, Some(streaming_path.clone()));

        // Verify file was created with proper header
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("\"headings\""));
        assert!(content.contains("\"data\""));
    }

    #[test]
    fn test_enable_csv_streaming() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("latency.csv");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        let result = manager.enable_csv_streaming(&csv_path);

        assert!(result.is_ok());
        assert!(manager.csv_streaming_enabled);
        assert!(manager.per_message_streaming);
        assert_eq!(manager.streaming_csv_file, Some(csv_path.clone()));

        // Verify file was created with CSV header
        let content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(content.contains("timestamp_ns"));
        assert!(content.contains("message_id"));
        assert!(content.contains("mechanism"));
    }

    #[test]
    fn test_enable_combined_streaming() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("combined.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        let result = manager.enable_combined_streaming(&streaming_path, true);

        assert!(result.is_ok());
        assert!(manager.streaming_enabled);
        assert!(manager.per_message_streaming);
        assert!(manager.both_tests_enabled);
        assert!(manager.is_combined_streaming_enabled());
    }

    #[test]
    fn test_add_results_and_finalize() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("final_results.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();
        let results = create_test_results();

        // Add results
        let add_result = manager.add_results(results.clone());
        assert!(add_result.is_ok());

        // Finalize
        let finalize_result = manager.finalize();
        assert!(finalize_result.is_ok());

        // Verify output file was created
        assert!(output_path.exists());

        // Verify content is valid JSON
        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.get("metadata").is_some());
        assert!(parsed.get("results").is_some());
        assert!(parsed.get("summary").is_some());
    }

    #[test]
    fn test_print_summary() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("results.json");

        let manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_stream_latency_record_with_streaming_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("streaming_records.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Create a test record
        let record = MessageLatencyRecord {
            timestamp_ns: 1000000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };

        let result = manager.stream_latency_record(&record);
        assert!(result.is_ok());

        // Finalize to close the file properly
        manager.finalize().unwrap();

        // Verify the record was written
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("1000000")); // timestamp
        assert!(content.contains("5000")); // latency
    }

    #[test]
    fn test_stream_latency_record_non_streaming_aggregates() {
        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Create a test record without enabling streaming
        let record = MessageLatencyRecord {
            timestamp_ns: 2000000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 200,
            one_way_latency_ns: Some(10000),
            round_trip_latency_ns: None,
        };

        let result = manager.stream_latency_record(&record);
        assert!(result.is_ok());

        // Verify record was stored in pending_records
        assert!(manager.pending_records.contains_key(&42));
    }

    #[test]
    fn test_stream_latency_record_merges_combined() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("combined_stream.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Create one-way record
        let one_way_record = MessageLatencyRecord {
            timestamp_ns: 3000000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };

        // Stream first record
        manager.stream_latency_record(&one_way_record).unwrap();

        // Create round-trip record for same message
        let round_trip_record = MessageLatencyRecord {
            timestamp_ns: 3000000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(12000),
        };

        // Stream second record - should merge with first
        manager.stream_latency_record(&round_trip_record).unwrap();

        // Finalize
        manager.finalize().unwrap();

        // Verify merged record was written
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("5000")); // one-way latency
        assert!(content.contains("12000")); // round-trip latency
    }

    #[test]
    fn test_csv_streaming_writes_records() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("latency_records.csv");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager.enable_csv_streaming(&csv_path).unwrap();

        // Create and stream a test record
        let record = MessageLatencyRecord {
            timestamp_ns: 4000000,
            message_id: 5,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 512,
            one_way_latency_ns: Some(8000),
            round_trip_latency_ns: Some(15000),
        };

        manager.stream_latency_record(&record).unwrap();

        // Finalize
        manager.finalize().unwrap();

        // Verify CSV content
        let content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(content.contains("4000000")); // timestamp
        assert!(content.contains("5")); // message_id
        assert!(content.contains("8000")); // one-way
        assert!(content.contains("15000")); // round-trip
    }

    #[test]
    fn test_message_latency_record_is_combined() {
        let one_way_only = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        assert!(!one_way_only.is_combined());

        let round_trip_only = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(10000),
        };
        assert!(!round_trip_only.is_combined());

        let combined = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: Some(10000),
        };
        assert!(combined.is_combined());
    }

    #[test]
    fn test_message_latency_record_merge() {
        let mut record1 = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };

        let record2 = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(12000),
        };

        record1.merge(&record2);

        assert_eq!(record1.one_way_latency_ns, Some(5000));
        assert_eq!(record1.round_trip_latency_ns, Some(12000));
        assert!(record1.is_combined());
    }

    #[test]
    fn test_message_latency_record_to_value_array() {
        let record = MessageLatencyRecord {
            timestamp_ns: 123456789,
            message_id: 42,
            mechanism: IpcMechanism::SharedMemory,
            message_size: 256,
            one_way_latency_ns: Some(7500),
            round_trip_latency_ns: Some(14000),
        };

        let values = record.to_value_array();

        assert_eq!(values.len(), 6);
        assert_eq!(values[0], serde_json::json!(123456789)); // timestamp
        assert_eq!(values[1], serde_json::json!(42)); // message_id
        assert_eq!(values[2], serde_json::json!("SharedMemory")); // mechanism
        assert_eq!(values[3], serde_json::json!(256)); // message_size
        assert_eq!(values[4], serde_json::json!(7500)); // one_way
        assert_eq!(values[5], serde_json::json!(14000)); // round_trip
    }

    #[test]
    fn test_message_latency_record_to_csv_record() {
        let record = MessageLatencyRecord {
            timestamp_ns: 999888777,
            message_id: 123,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 1024,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: Some(9500),
        };

        let csv = record.to_csv_record();

        assert!(csv.contains("999888777"));
        assert!(csv.contains("123"));
        // Display format uses "TCP Socket" with spaces
        assert!(csv.contains("TCP Socket"));
        assert!(csv.contains("1024"));
        assert!(csv.contains("5000"));
        assert!(csv.contains("9500"));
    }

    #[test]
    fn test_message_latency_record_to_csv_record_with_nulls() {
        let record = MessageLatencyRecord {
            timestamp_ns: 111222333,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 64,
            one_way_latency_ns: None,
            round_trip_latency_ns: None,
        };

        let csv = record.to_csv_record();

        // Nulls should appear as empty or null values
        assert!(csv.contains("111222333"));
        assert!(csv.contains("1"));
    }

    #[test]
    fn test_streaming_multiple_records() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("multi_records.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Stream multiple records
        for i in 0..5 {
            let record = MessageLatencyRecord {
                timestamp_ns: 1000000 + i * 1000,
                message_id: i,
                mechanism: IpcMechanism::TcpSocket,
                message_size: 100,
                one_way_latency_ns: Some(5000 + i * 100),
                round_trip_latency_ns: None,
            };
            manager.stream_latency_record(&record).unwrap();
        }

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        // Should contain all 5 message IDs
        for i in 0..5u64 {
            assert!(
                content.contains(&format!("{}", 5000 + i * 100)),
                "Should contain latency for message {}",
                i
            );
        }
    }

    #[test]
    fn test_streaming_without_handle_uses_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("fallback_stream.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Enable streaming but manually set handle to None to test fallback
        manager.streaming_enabled = true;
        manager.streaming_file = Some(streaming_path.clone());
        manager.streaming_file_handle = None; // Force fallback path
        manager.first_record_streamed = true;

        // Write header manually since we bypassed enable_per_message_streaming
        std::fs::write(
            &streaming_path,
            "{\n  \"headings\": [\"a\"],\n  \"data\": [",
        )
        .unwrap();

        let record = MessageLatencyRecord {
            timestamp_ns: 5000000,
            message_id: 99,
            mechanism: IpcMechanism::SharedMemory,
            message_size: 200,
            one_way_latency_ns: Some(8000),
            round_trip_latency_ns: None,
        };

        // This should use the fallback path (open/append)
        let result = manager.write_streaming_record(&record);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("8000")); // latency value
    }

    #[test]
    fn test_csv_streaming_without_handle_uses_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("fallback_csv.csv");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Enable CSV streaming but force fallback path
        manager.csv_streaming_enabled = true;
        manager.per_message_streaming = true;
        manager.streaming_csv_file = Some(csv_path.clone());
        manager.streaming_csv_handle = None; // Force fallback path

        // Write CSV header
        std::fs::write(&csv_path, "timestamp_ns,message_id,mechanism,message_size,one_way_latency_ns,round_trip_latency_ns\n").unwrap();

        let record = MessageLatencyRecord {
            timestamp_ns: 7000000,
            message_id: 77,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 512,
            one_way_latency_ns: Some(6500),
            round_trip_latency_ns: Some(12000),
        };

        // This should use the fallback path
        let result = manager.write_streaming_record(&record);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(content.contains("7000000")); // timestamp
        assert!(content.contains("77")); // message_id
        assert!(content.contains("6500")); // one_way
        assert!(content.contains("12000")); // round_trip
    }

    #[test]
    fn test_add_multiple_results() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("multi_results.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        // Add multiple results
        for _ in 0..3 {
            let results = create_test_results();
            manager.add_results(results).unwrap();
        }

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Should have results array
        assert!(parsed.get("results").is_some());
    }

    #[test]
    fn test_finalize_without_output_file() {
        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Finalize without any output file should succeed (no-op)
        let result = manager.finalize();
        assert!(result.is_ok());
    }

    #[test]
    fn test_finalize_closes_streaming_handles() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("close_test.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Stream a record
        let record = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        manager.stream_latency_record(&record).unwrap();

        // Finalize should close handles and complete JSON
        manager.finalize().unwrap();

        // Verify handles are closed (set to None)
        assert!(manager.streaming_file_handle.is_none());

        // Verify JSON file was written with data
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("5000"), "Should contain the latency value");
        assert!(content.contains("data"), "Should contain data array");
    }

    #[test]
    fn test_write_streaming_record_direct_not_enabled() {
        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        let record = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };

        // When streaming is not enabled, should return Ok without doing anything
        let result = manager.write_streaming_record_direct(&record);
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_streaming_record_direct_combined_record() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("direct_combined.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Create a combined record (both latencies present)
        let record = MessageLatencyRecord {
            timestamp_ns: 2000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: Some(10000),
        };

        // Should write immediately since it's already combined
        let result = manager.write_streaming_record_direct(&record);
        assert!(result.is_ok());

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("5000"));
        assert!(content.contains("10000"));
    }

    #[test]
    fn test_write_streaming_record_direct_merges_partial_records() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("direct_merge.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Send first partial record (one-way only)
        let record1 = MessageLatencyRecord {
            timestamp_ns: 3000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(6000),
            round_trip_latency_ns: None,
        };
        manager.write_streaming_record_direct(&record1).unwrap();

        // Verify it's stored in pending
        assert!(manager.pending_records.contains_key(&42));

        // Send second partial record (round-trip only) for same message
        let record2 = MessageLatencyRecord {
            timestamp_ns: 3000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(11000),
        };
        manager.write_streaming_record_direct(&record2).unwrap();

        // Pending should be empty now (record was written)
        assert!(!manager.pending_records.contains_key(&42));

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("6000")); // one-way
        assert!(content.contains("11000")); // round-trip
    }

    #[test]
    fn test_write_streaming_record_direct_non_combined_mode() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("direct_non_combined.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();
        // Note: both_tests_enabled is false by default

        let record = MessageLatencyRecord {
            timestamp_ns: 4000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(7000),
            round_trip_latency_ns: None,
        };

        // Should write immediately since not in combined mode
        manager.write_streaming_record_direct(&record).unwrap();

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("7000"));
    }

    #[test]
    fn test_stream_results_with_streaming_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("stream_results.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Enable streaming but NOT per-message streaming
        manager.streaming_enabled = true;
        manager.per_message_streaming = false;
        manager.streaming_file = Some(streaming_path.clone());

        // Write opening bracket
        std::fs::write(&streaming_path, "[").unwrap();

        // Add results - this should trigger stream_results
        let results = create_test_results();
        manager.add_results(results).unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("TCP Socket") || content.contains("TcpSocket"));
    }

    #[test]
    fn test_stream_results_multiple_results() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("multi_stream_results.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        manager.streaming_enabled = true;
        manager.per_message_streaming = false;
        manager.streaming_file = Some(streaming_path.clone());

        std::fs::write(&streaming_path, "[").unwrap();

        // Add first results
        let results1 = create_test_results();
        manager.add_results(results1).unwrap();

        // Add second results - should add comma separator
        let results2 = create_test_results();
        manager.add_results(results2).unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        // Should have comma between results
        assert!(content.contains(","));
    }

    #[test]
    fn test_pending_records_flushed_on_finalize() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("pending_flush.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Add a partial record that won't be matched
        let record = MessageLatencyRecord {
            timestamp_ns: 5000,
            message_id: 99,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(8000),
            round_trip_latency_ns: None,
        };
        manager.write_streaming_record_direct(&record).unwrap();

        // Should be pending
        assert!(manager.pending_records.contains_key(&99));

        // Finalize should flush pending records
        manager.finalize().unwrap();

        // Pending should be empty after finalize
        assert!(manager.pending_records.is_empty());

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("8000")); // The partial record should be written
    }

    #[test]
    fn test_close_streaming_json_per_message_with_handle() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("close_json_handle.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Stream a record
        let record = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        manager.stream_latency_record(&record).unwrap();

        // Close the JSON file
        let result = manager.close_streaming_json();
        assert!(result.is_ok());

        // Handle should be taken
        assert!(manager.streaming_file_handle.is_none());
    }

    #[test]
    fn test_close_streaming_json_non_per_message() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("close_json_non_pm.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Enable regular streaming (not per-message)
        manager.streaming_enabled = true;
        manager.per_message_streaming = false;
        manager.streaming_file = Some(streaming_path.clone());

        // Write opening bracket
        std::fs::write(&streaming_path, "[").unwrap();

        // Close should write closing bracket
        let result = manager.close_streaming_json();
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.ends_with("]\n") || content.ends_with("]"));
    }

    #[test]
    fn test_close_streaming_csv() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("close_csv.csv");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager.enable_csv_streaming(&csv_path).unwrap();

        // Stream a record
        let record = MessageLatencyRecord {
            timestamp_ns: 2000,
            message_id: 2,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(6000),
            round_trip_latency_ns: None,
        };
        manager.stream_latency_record(&record).unwrap();

        // Close CSV
        let result = manager.close_streaming_csv();
        assert!(result.is_ok());

        // Handle should be taken
        assert!(manager.streaming_csv_handle.is_none());
    }

    #[test]
    fn test_find_lowest_latency_mechanism() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("latency_compare.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        // Add results with different latencies
        let mut results1 = create_test_results();
        results1.summary.average_latency_ns = Some(10000.0);
        manager.add_results(results1).unwrap();

        // Create a second result with lower latency
        let mut results2 = BenchmarkResults::new(
            IpcMechanism::SharedMemory,
            1024,
            8192,
            1,
            Some(1000),
            None,
            0,
            true,  // one_way
            true,  // round_trip
        );
        results2.summary.average_latency_ns = Some(5000.0); // Lower
        manager.add_results(results2).unwrap();

        // The lowest latency should be SharedMemory
        let lowest = manager.find_lowest_latency_mechanism();
        assert!(lowest.is_some());
        assert!(lowest.unwrap().contains("Shared Memory"));
    }

    #[test]
    fn test_find_fastest_mechanism() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("throughput_compare.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        // Add results with different throughputs
        let mut results1 = create_test_results();
        results1.summary.average_throughput_mbps = 100.0;
        manager.add_results(results1).unwrap();

        // Create a second result with higher throughput
        let mut results2 = BenchmarkResults::new(
            IpcMechanism::SharedMemory,
            1024,
            8192,
            1,
            Some(1000),
            None,
            0,
            true,  // one_way
            true,  // round_trip
        );
        results2.summary.average_throughput_mbps = 200.0; // Higher
        manager.add_results(results2).unwrap();

        // The fastest should be SharedMemory
        let fastest = manager.find_fastest_mechanism();
        assert!(fastest.is_some());
        assert!(fastest.unwrap().contains("Shared Memory"));
    }

    #[test]
    fn test_get_system_info() {
        let manager = BlockingResultsManager::new(None, None).unwrap();
        let sys_info = manager.get_system_info();

        // Should have valid system info
        assert!(!sys_info.os.is_empty());
        assert!(!sys_info.architecture.is_empty());
        assert!(sys_info.cpu_cores > 0);
        assert!(sys_info.memory_gb > 0.0);
    }

    #[test]
    fn test_write_final_results_creates_valid_json() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("final_output.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        // Add some results
        let results = create_test_results();
        manager.add_results(results).unwrap();

        // Finalize writes the final results
        manager.finalize().unwrap();

        // Verify the file exists and contains valid JSON
        assert!(output_path.exists());
        let content = std::fs::read_to_string(&output_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Check structure
        assert!(parsed.get("metadata").is_some());
        assert!(parsed.get("results").is_some());
        assert!(parsed.get("summary").is_some());

        // Check metadata fields
        let metadata = parsed.get("metadata").unwrap();
        assert!(metadata.get("version").is_some());
        assert!(metadata.get("timestamp").is_some());
        assert!(metadata.get("system_info").is_some());
    }

    #[test]
    fn test_finalize_with_all_streaming_types() {
        let temp_dir = TempDir::new().unwrap();
        let json_path = temp_dir.path().join("all_stream.json");
        let csv_path = temp_dir.path().join("all_stream.csv");
        let output_path = temp_dir.path().join("all_output.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();
        manager.enable_per_message_streaming(&json_path).unwrap();
        manager.enable_csv_streaming(&csv_path).unwrap();

        // Stream some records
        for i in 0..3 {
            let record = MessageLatencyRecord {
                timestamp_ns: 1000 * (i + 1),
                message_id: i,
                mechanism: IpcMechanism::TcpSocket,
                message_size: 100,
                one_way_latency_ns: Some(5000 + i * 100),
                round_trip_latency_ns: None,
            };
            manager.stream_latency_record(&record).unwrap();
        }

        // Add final results
        let results = create_test_results();
        manager.add_results(results).unwrap();

        // Finalize everything
        manager.finalize().unwrap();

        // All files should exist
        assert!(json_path.exists());
        assert!(csv_path.exists());
        assert!(output_path.exists());

        // JSON should be valid
        let json_content = std::fs::read_to_string(&json_path).unwrap();
        assert!(json_content.contains("5000"));

        // CSV should have header and data
        let csv_content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_content.contains("timestamp_ns"));
        assert!(csv_content.contains("5000"));
    }

    #[test]
    fn test_flush_pending_records_multiple() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("flush_multi.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // Add multiple pending records
        for i in 0..5 {
            let record = MessageLatencyRecord {
                timestamp_ns: 1000 * (i + 1),
                message_id: i,
                mechanism: IpcMechanism::TcpSocket,
                message_size: 100,
                one_way_latency_ns: Some(5000 + i * 100),
                round_trip_latency_ns: None,
            };
            manager.write_streaming_record_direct(&record).unwrap();
        }

        // All should be pending
        assert_eq!(manager.pending_records.len(), 5);

        // Flush pending records
        manager.flush_pending_records().unwrap();

        // Should be empty now
        assert!(manager.pending_records.is_empty());

        // Finalize to close the file
        manager.finalize().unwrap();

        // All records should be in the file
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        for i in 0..5 {
            let latency = 5000 + i * 100;
            assert!(
                content.contains(&latency.to_string()),
                "Should contain latency {}",
                latency
            );
        }
    }

    #[test]
    fn test_flush_pending_records_empty() {
        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager.both_tests_enabled = true;

        // Flushing empty pending records should succeed
        let result = manager.flush_pending_records();
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_combined_streaming_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("combined_check.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        assert!(!manager.is_combined_streaming_enabled());

        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();
        assert!(manager.is_combined_streaming_enabled());
    }

    #[test]
    fn test_message_latency_record_clone() {
        let record = MessageLatencyRecord {
            timestamp_ns: 12345,
            message_id: 99,
            mechanism: IpcMechanism::SharedMemory,
            message_size: 512,
            one_way_latency_ns: Some(7500),
            round_trip_latency_ns: Some(15000),
        };

        let cloned = record.clone();
        assert_eq!(record.timestamp_ns, cloned.timestamp_ns);
        assert_eq!(record.message_id, cloned.message_id);
        assert_eq!(record.message_size, cloned.message_size);
        assert_eq!(record.one_way_latency_ns, cloned.one_way_latency_ns);
        assert_eq!(record.round_trip_latency_ns, cloned.round_trip_latency_ns);
    }

    #[test]
    fn test_manager_with_log_file() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("results.json");
        let log_path = temp_dir.path().join("benchmark.log");

        let manager =
            BlockingResultsManager::new(Some(&output_path), Some(log_path.to_str().unwrap()))
                .unwrap();

        assert_eq!(
            manager.log_file,
            Some(log_path.to_str().unwrap().to_string())
        );
    }

    #[test]
    fn test_streaming_record_ordering() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("ordering.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Stream records out of order
        let ids = [5, 2, 8, 1, 9];
        for &id in &ids {
            let record = MessageLatencyRecord {
                timestamp_ns: 1000 * id,
                message_id: id,
                mechanism: IpcMechanism::TcpSocket,
                message_size: 100,
                one_way_latency_ns: Some(id * 100),
                round_trip_latency_ns: None,
            };
            manager.stream_latency_record(&record).unwrap();
        }

        manager.finalize().unwrap();

        // All records should be in the file
        let content = std::fs::read_to_string(&streaming_path).unwrap();
        for &id in &ids {
            assert!(
                content.contains(&(id * 100).to_string()),
                "Should contain latency for id {}",
                id
            );
        }
    }

    #[test]
    fn test_print_summary_empty_results() {
        let manager = BlockingResultsManager::new(None, None).unwrap();
        // Should not panic with empty results
        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_summary_with_results() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("summary_test.json");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();

        let results = create_test_results();
        manager.add_results(results).unwrap();

        // Should not panic with results
        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_summary_with_streaming_files() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("summary.json");
        let streaming_json = temp_dir.path().join("streaming.json");
        let streaming_csv = temp_dir.path().join("streaming.csv");

        let mut manager = BlockingResultsManager::new(Some(&output_path), None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_json)
            .unwrap();
        manager.enable_csv_streaming(&streaming_csv).unwrap();

        let results = create_test_results();
        manager.add_results(results).unwrap();

        // Should print all file paths
        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_summary_with_log_file() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("summary.json");
        let log_path = temp_dir.path().join("benchmark.log");

        let manager =
            BlockingResultsManager::new(Some(&output_path), Some(log_path.to_str().unwrap()))
                .unwrap();

        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_add_results_updates_internal_state() {
        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        assert!(manager.results.is_empty());

        let results = create_test_results();
        manager.add_results(results).unwrap();

        assert_eq!(manager.results.len(), 1);

        // Add another result
        let results2 =
            BenchmarkResults::new(IpcMechanism::SharedMemory, 512, 4096, 1, Some(500), None, 0, true, true);
        manager.add_results(results2).unwrap();

        assert_eq!(manager.results.len(), 2);
    }

    #[test]
    fn test_enable_combined_streaming_creates_file() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("combined.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Enable combined streaming with both tests
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        assert!(manager.streaming_enabled);
        assert!(manager.per_message_streaming);
        assert!(manager.both_tests_enabled);
        assert!(streaming_path.exists());

        manager.finalize().unwrap();
    }

    #[test]
    fn test_streaming_csv_writes_header() {
        let temp_dir = TempDir::new().unwrap();
        let csv_path = temp_dir.path().join("header_test.csv");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager.enable_csv_streaming(&csv_path).unwrap();

        // Finalize without any records
        manager.finalize().unwrap();

        // File should exist with header
        let content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(content.contains("timestamp_ns"));
        assert!(content.contains("message_id"));
        assert!(content.contains("mechanism"));
    }

    #[test]
    fn test_multiple_records_same_message_id() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("same_id.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // First record with one-way latency
        let record1 = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        manager.write_streaming_record_direct(&record1).unwrap();

        // Second record with round-trip latency for same message
        let record2 = MessageLatencyRecord {
            timestamp_ns: 2000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(10000),
        };
        manager.write_streaming_record_direct(&record2).unwrap();

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        // Combined record should have both latencies
        assert!(content.contains("5000"));
        assert!(content.contains("10000"));
    }

    #[test]
    fn test_print_summary_with_failed_result() {
        use crate::results::BenchmarkStatus;

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        // Create a failed result
        let mut results =
            BenchmarkResults::new(IpcMechanism::TcpSocket, 1024, 8192, 1, Some(100), None, 0, true, true);
        results.status = BenchmarkStatus::Failure("Test failure message".to_string());
        manager.add_results(results).unwrap();

        // Should print without panicking
        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_summary_with_latency_results() {
        use crate::metrics::{
            LatencyMetrics, LatencyType, PercentileValue, PerformanceMetrics, ThroughputMetrics,
        };
        use crate::results::BenchmarkStatus;

        let mut manager = BlockingResultsManager::new(None, None).unwrap();

        let mut results =
            BenchmarkResults::new(IpcMechanism::TcpSocket, 1024, 8192, 1, Some(100), None, 0, true, true);
        results.status = BenchmarkStatus::Success;

        // Add one-way latency results
        let latency = LatencyMetrics {
            latency_type: LatencyType::OneWay,
            mean_ns: 5000.0,
            min_ns: 1000,
            max_ns: 10000,
            median_ns: 4500.0,
            std_dev_ns: 1500.0,
            percentiles: vec![
                PercentileValue {
                    percentile: 95.0,
                    value_ns: 8000,
                },
                PercentileValue {
                    percentile: 99.0,
                    value_ns: 9500,
                },
            ],
            total_samples: 100,
            histogram_data: vec![],
        };

        let throughput = ThroughputMetrics {
            bytes_per_second: 1000000.0,
            messages_per_second: 1000.0,
            total_bytes: 100000,
            total_messages: 100,
            duration_ns: 1000000000,
        };

        results.one_way_results = Some(PerformanceMetrics {
            latency: Some(latency.clone()),
            throughput: throughput.clone(),
            timestamp: chrono::Utc::now(),
        });

        results.round_trip_results = Some(PerformanceMetrics {
            latency: Some(latency),
            throughput,
            timestamp: chrono::Utc::now(),
        });

        manager.add_results(results).unwrap();

        // Should print detailed latency info without panicking
        let result = manager.print_summary();
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_streaming_record_without_handle() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("no_handle.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // Take the handle so write falls back to file open/append
        let _ = manager.streaming_file_handle.take();

        // Stream a record - should use fallback path
        let record = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };

        let result = manager.stream_latency_record(&record);
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_streaming_record_second_record() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("second_record.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_per_message_streaming(&streaming_path)
            .unwrap();

        // First record
        let record1 = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 1,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        manager.stream_latency_record(&record1).unwrap();

        // Second record - should add comma separator
        let record2 = MessageLatencyRecord {
            timestamp_ns: 2000,
            message_id: 2,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(6000),
            round_trip_latency_ns: None,
        };
        manager.stream_latency_record(&record2).unwrap();

        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        assert!(content.contains("5000"));
        assert!(content.contains("6000"));
    }

    #[test]
    fn test_pending_records_merge_existing() {
        let temp_dir = TempDir::new().unwrap();
        let streaming_path = temp_dir.path().join("merge_pending.json");

        let mut manager = BlockingResultsManager::new(None, None).unwrap();
        manager
            .enable_combined_streaming(&streaming_path, true)
            .unwrap();

        // First partial record
        let record1 = MessageLatencyRecord {
            timestamp_ns: 1000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: Some(5000),
            round_trip_latency_ns: None,
        };
        manager.write_streaming_record_direct(&record1).unwrap();

        // Check it's pending
        assert!(manager.pending_records.contains_key(&42));

        // Second partial record for same ID
        let record2 = MessageLatencyRecord {
            timestamp_ns: 2000,
            message_id: 42,
            mechanism: IpcMechanism::TcpSocket,
            message_size: 100,
            one_way_latency_ns: None,
            round_trip_latency_ns: Some(10000),
        };
        manager.write_streaming_record_direct(&record2).unwrap();

        // Finalize should flush merged record
        manager.finalize().unwrap();

        let content = std::fs::read_to_string(&streaming_path).unwrap();
        // Merged record should contain both latencies
        assert!(content.contains("5000"));
        assert!(content.contains("10000"));
    }

    #[test]
    fn test_format_latency_milliseconds() {
        // >= 1,000,000 ns should format as ms
        let result = super::format_latency(1_500_000);
        assert!(result.contains("ms"));
        assert!(result.contains("1.50"));
    }

    #[test]
    fn test_format_latency_microseconds() {
        // >= 1,000 ns but < 1,000,000 ns should format as us
        let result = super::format_latency(5_000);
        assert!(result.contains("us"));
        assert!(result.contains("5.00"));
    }

    #[test]
    fn test_format_latency_nanoseconds() {
        // < 1,000 ns should format as ns
        let result = super::format_latency(500);
        assert!(result.contains("ns"));
        assert!(result.contains("500"));
    }

    #[test]
    fn test_format_latency_boundary_milliseconds() {
        // Exactly 1,000,000 ns = 1 ms
        let result = super::format_latency(1_000_000);
        assert!(result.contains("ms"));
        assert!(result.contains("1.00"));
    }

    #[test]
    fn test_format_latency_boundary_microseconds() {
        // Exactly 1,000 ns = 1 us
        let result = super::format_latency(1_000);
        assert!(result.contains("us"));
        assert!(result.contains("1.00"));
    }

    #[test]
    fn test_format_latency_zero() {
        let result = super::format_latency(0);
        assert!(result.contains("0 ns"));
    }
}
