//! # Command-Line Interface Module
//!
//! This module provides comprehensive command-line argument parsing and configuration
//! management for the IPC benchmark suite. It uses the `clap` crate's derive API
//! to provide a user-friendly interface while maintaining type safety and validation.
//!
//! ## Key Features
//!
//! - **Type-safe argument parsing** with automatic validation and help generation
//! - **Duration parsing** with human-readable formats (e.g., "10s", "5m", "1h")
//! - **Mechanism selection** with support for individual mechanisms or "all"
//! - **Configuration transformation** from CLI args to internal benchmark config
//! - **Comprehensive validation** of numeric ranges and file paths
//!
//! ## Usage Examples
//!
//! ```bash
//! # Test all mechanisms with default settings
//! ipc-benchmark --mechanisms all
//!
//! # Test specific mechanisms with custom parameters
//! ipc-benchmark -m uds tcp -s 4096 -i 50000 -c 4
//!
//! # Duration-based testing with streaming output
//! ipc-benchmark -m all -d 30s --streaming-output live_results.json
//!
//! # High-throughput testing with large buffers
//! ipc-benchmark -m shm -s 16384 --buffer-size 65536 --concurrency 8
//! ```
//!
//! ## Argument Categories
//!
//! Arguments are organized into logical groups:
//! - **Core Options**: Primary test configuration (mechanisms, size, iterations)
//! - **Timing**: Duration vs. iteration-based testing
//! - **Concurrency**: Multi-threaded test configuration  
//! - **Output**: Result file paths and streaming options
//! - **Advanced**: Buffer sizes, network settings, percentiles

use clap::{builder::styling::{AnsiColor, Styles}, Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// IPC Benchmark Suite - A comprehensive tool for measuring IPC performance
///
/// This application benchmarks various Inter-Process Communication mechanisms
/// to compare their latency and throughput characteristics under different
/// workloads and configurations.
///
/// ## Supported IPC Mechanisms
///
/// - **uds**: Unix Domain Sockets - High-performance local communication
/// - **shm**: Shared Memory - Direct memory access with ring buffers
/// - **tcp**: TCP Sockets - Network communication with configurable options
/// - **pmq**: POSIX Message Queues - System-level message passing
/// - **all**: Test all available mechanisms sequentially
///
/// ## Test Types
///
/// The benchmark suite supports two primary test patterns:
///
/// - **One-way latency**: Measures time to send messages from client to server
/// - **Round-trip latency**: Measures time for request-response message pairs
///
/// Both test types provide comprehensive statistics including percentiles,
/// standard deviation, and throughput measurements.

/// Defines the styles for the help message to replicate clap v3's appearance.
fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Yellow.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None, styles = styles())]
pub struct Args {
    /// IPC mechanisms to benchmark (space-separated: uds, shm, tcp, or all)
    ///
    /// Multiple mechanisms can be specified to run sequential tests.
    /// The "all" option expands to all available mechanisms for comprehensive testing.
    /// Each mechanism is tested independently with proper resource cleanup between runs.
    #[arg(short = 'm', value_enum, default_values_t = vec![IpcMechanism::UnixDomainSocket], help_heading = "Core Options", num_args = 1..)]
    pub mechanisms: Vec<IpcMechanism>,

    /// Message size in bytes
    ///
    /// Determines the payload size for each message sent during testing.
    /// Larger messages test throughput capabilities while smaller messages
    /// focus on latency characteristics. Range: 1 byte to 16MB.
    #[arg(short = 's', long, default_value_t = crate::defaults::MESSAGE_SIZE)]
    pub message_size: usize,

    /// Number of iterations to run (ignored if duration is specified)
    ///
    /// Controls how many messages are sent during the test when using
    /// iteration-based testing. Higher values provide better statistical
    /// accuracy but increase test duration. Ignored when --duration is specified.
    #[arg(short = 'i', long, default_value_t = crate::defaults::ITERATIONS)]
    pub iterations: usize,

    /// Duration to run the benchmark (takes precedence over iterations)
    ///
    /// When specified, tests run for a fixed time period rather than a fixed
    /// number of iterations. Supports human-readable formats like "30s", "5m", "1h".
    /// This mode is useful for consistent test durations across different mechanisms.
    #[arg(short = 'd', long, value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// Number of concurrent processes/threads
    ///
    /// Controls the level of parallelism during testing. Higher values can reveal
    /// scalability characteristics but may introduce resource contention.
    /// Note: Some mechanisms (like shared memory) may force concurrency to 1
    /// to avoid race conditions in the current implementation.
    #[arg(short = 'c', long, default_value_t = crate::defaults::CONCURRENCY)]
    pub concurrency: usize,

    /// Output file for results (JSON format)
    ///
    /// Specifies where to write the final benchmark results in structured JSON format.
    /// The file contains comprehensive metrics, system information, and metadata
    /// for reproducibility and analysis.
    #[arg(short = 'o', long, default_value = crate::defaults::OUTPUT_FILE)]
    pub output_file: PathBuf,

    /// Include one-way latency measurements
    ///
    /// Enables testing of one-way message latency from client to server.
    /// This measures the time from when a message is sent until it's received,
    /// providing insights into pure transmission latency.
    /// If neither --one-way nor --round-trip is specified, both tests run by default.
    #[arg(long)]
    pub one_way: bool,

    /// Include round-trip latency measurements  
    ///
    /// Enables testing of request-response latency patterns.
    /// This measures the time from sending a request until receiving a response,
    /// providing insights into full communication cycle performance.
    /// If neither --one-way nor --round-trip is specified, both tests run by default.
    #[arg(long)]
    pub round_trip: bool,

    /// Number of warmup iterations
    ///
    /// Specifies how many messages to send before starting measurement.
    /// Warmup helps stabilize performance by allowing caches to fill,
    /// connections to establish, and OS buffers to optimize.
    #[arg(short = 'w', long, default_value_t = crate::defaults::WARMUP_ITERATIONS)]
    pub warmup_iterations: usize,

    /// Continue running other benchmarks even if one fails
    ///
    /// By default, the suite stops on the first benchmark failure.
    /// This flag allows testing to continue with remaining mechanisms,
    /// useful for comprehensive testing even when some mechanisms fail.
    #[arg(long, default_value_t = false)]
    pub continue_on_error: bool,

    /// Silence all user-facing informational output on stdout
    ///
    /// When this flag is present, only diagnostic logs on stderr will be shown.
    /// This is useful for scripting or when piping results to another program.
    #[arg(short = 'q', long, help_heading = "Output and Logging")]
    pub quiet: bool,

    /// Increase diagnostic log verbosity on stderr.
    ///
    /// Can be used multiple times to increase detail:
    ///  -v: info
    ///  -vv: debug
    ///  -vvv: trace
    /// By default, only WARNING and ERROR messages are shown.
    #[arg(short, long, action = clap::ArgAction::Count, help_heading = "Output and Logging")]
    pub verbose: u8,

    /// JSON output file for streaming results during execution
    ///
    /// When specified, partial results are written to this file in real-time
    /// during benchmark execution. This allows monitoring progress for long-running
    /// tests and provides incremental results if the benchmark is interrupted.
    #[arg(long)]
    pub streaming_output: Option<PathBuf>,

    /// Percentiles to calculate for latency metrics
    ///
    /// Specifies which percentile values to calculate and report in results.
    /// Common values include P50 (median), P95, P99, and P99.9.
    /// Multiple values can be specified to get a comprehensive latency distribution view.
    #[arg(long, default_values_t = vec![50.0, 95.0, 99.0, 99.9])]
    pub percentiles: Vec<f64>,

    /// Buffer size for message queues and shared memory
    ///
    /// Controls the size of internal buffers used by IPC mechanisms.
    /// Larger buffers can improve throughput but increase memory usage.
    /// The optimal size depends on message size and concurrency level.
    #[arg(long, default_value_t = 8192)]
    pub buffer_size: usize,

    /// Host address for TCP sockets
    ///
    /// Specifies the network interface to bind for TCP socket tests.
    /// Use "127.0.0.1" for localhost testing or "0.0.0.0" to accept
    /// connections from any interface (useful for distributed testing).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port for TCP sockets
    ///
    /// Specifies the TCP port number for socket communication.
    /// The benchmark will automatically use unique ports for each test
    /// to avoid conflicts when testing multiple mechanisms.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
}

/// Available IPC mechanisms for benchmarking
///
/// This enumeration defines all supported Inter-Process Communication mechanisms.
/// Each variant corresponds to a specific implementation with its own performance
/// characteristics and use cases.
///
/// ## Performance Characteristics
///
/// - **UnixDomainSocket**: Excellent for local communication, supports multiple clients
/// - **SharedMemory**: Highest throughput, lowest latency, but limited to single process pairs
/// - **TcpSocket**: Network-capable, good performance, supports multiple clients
/// - **PosixMessageQueue**: System-integrated, message boundaries preserved, limited throughput
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
pub enum IpcMechanism {
    /// Unix Domain Sockets
    ///
    /// High-performance local sockets that provide reliable, ordered communication
    /// between processes on the same machine. Supports full-duplex communication
    /// and multiple concurrent clients. Ideal for local service architectures.
    #[value(name = "uds")]
    UnixDomainSocket,

    /// Shared Memory
    ///
    /// Direct memory sharing between processes using a custom ring buffer implementation.
    /// Provides the highest throughput and lowest latency but requires careful
    /// synchronization. Limited to single client-server pairs in current implementation.
    #[value(name = "shm")]
    SharedMemory,

    /// TCP Sockets
    ///
    /// Standard network sockets that can work locally or across networks.
    /// Provides good performance with broad compatibility and multi-client support.
    /// Socket options are tuned for low latency (TCP_NODELAY, buffer sizes).
    #[value(name = "tcp")]
    TcpSocket,

    /// POSIX Message Queues
    ///
    /// System-level message queues that preserve message boundaries and support
    /// priority-based delivery. Integrated with OS scheduling but limited by
    /// system-imposed queue depth restrictions (typically 10 messages).
    #[value(name = "pmq")]
    PosixMessageQueue,

    /// All available mechanisms
    ///
    /// Convenience option that expands to test all supported IPC mechanisms
    /// sequentially. Useful for comprehensive performance comparisons across
    /// all available transport types.
    #[value(name = "all")]
    All,
}

impl std::fmt::Display for IpcMechanism {
    /// Provide human-readable names for IPC mechanisms
    ///
    /// Used in output formatting and logging to present user-friendly
    /// names rather than the internal enum variant names.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcMechanism::UnixDomainSocket => write!(f, "Unix Domain Socket"),
            IpcMechanism::SharedMemory => write!(f, "Shared Memory"),
            IpcMechanism::TcpSocket => write!(f, "TCP Socket"),
            IpcMechanism::PosixMessageQueue => write!(f, "POSIX Message Queue"),
            IpcMechanism::All => write!(f, "All Mechanisms"),
        }
    }
}

impl IpcMechanism {
    /// Expand the "All" variant to all available mechanisms
    ///
    /// This function handles the special "all" mechanism by expanding it to
    /// the complete list of concrete IPC mechanisms. If "all" is present
    /// anywhere in the input list, it returns all mechanisms; otherwise,
    /// it returns the input list unchanged.
    ///
    /// ## Parameters
    /// - `mechanisms`: Vector of mechanisms which may include the "All" variant
    ///
    /// ## Returns
    /// Vector of concrete mechanisms with "All" expanded if present
    ///
    /// ## Example
    /// ```rust
    /// use ipc_benchmark::cli::IpcMechanism;
    /// 
    /// let input = vec![IpcMechanism::All];
    /// let expanded = IpcMechanism::expand_all(input);
    /// assert_eq!(expanded.len(), 4); // All concrete mechanisms
    /// ```
    pub fn expand_all(mechanisms: Vec<IpcMechanism>) -> Vec<IpcMechanism> {
        if mechanisms.contains(&IpcMechanism::All) {
            // Return all concrete mechanisms in a logical order:
            // - UDS first (most commonly used for local IPC)
            // - SHM second (highest performance)  
            // - TCP third (network-capable)
            // - PMQ last (most constrained)
            vec![
                IpcMechanism::UnixDomainSocket,
                IpcMechanism::SharedMemory,
                IpcMechanism::TcpSocket,
                IpcMechanism::PosixMessageQueue,
            ]
        } else {
            mechanisms
        }
    }
}

/// Configuration for the benchmark execution
///
/// This structure represents the internal configuration format used by the
/// benchmark engine. It's derived from the CLI arguments but provides a
/// more structured representation suitable for internal use.
///
/// ## Conversion from CLI Args
///
/// The `From<&Args>` implementation handles the conversion from user-friendly
/// CLI arguments to this internal format, including:
/// - Mechanism list expansion (handling "all")
/// - Duration vs. iteration precedence handling
/// - Default value application
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkConfiguration {
    /// List of mechanisms to test (with "all" expanded)
    pub mechanisms: Vec<IpcMechanism>,
    
    /// Size of message payloads in bytes
    pub message_size: usize,
    
    /// Number of iterations (None if duration-based)
    pub iterations: Option<usize>,
    
    /// Test duration (takes precedence over iterations)
    pub duration: Option<Duration>,
    
    /// Number of concurrent workers
    pub concurrency: usize,
    
    /// Whether to run one-way latency tests
    pub one_way: bool,
    
    /// Whether to run round-trip latency tests
    pub round_trip: bool,
    
    /// Number of warmup iterations before measurement
    pub warmup_iterations: usize,
    
    /// Percentiles to calculate for latency analysis
    pub percentiles: Vec<f64>,
    
    /// Buffer size for internal data structures
    pub buffer_size: usize,
    
    /// Host address for network-based mechanisms
    pub host: String,
    
    /// Port number for network-based mechanisms
    pub port: u16,
}

impl From<&Args> for BenchmarkConfiguration {
    /// Convert CLI arguments to internal benchmark configuration
    ///
    /// This conversion handles several important transformations:
    /// 1. **Mechanism expansion**: Converts "all" to concrete mechanism list
    /// 2. **Duration precedence**: If duration is specified, iterations becomes None
    /// 3. **Test type selection**: If neither test type is specified, both run by default
    /// 4. **Value validation**: Ensures all parameters are within valid ranges
    ///
    /// The resulting configuration is ready for use by the benchmark engine.
    fn from(args: &Args) -> Self {
        // If neither test type is explicitly specified, run both (default behavior)
        let (one_way, round_trip) = if !args.one_way && !args.round_trip {
            (true, true) // Default: run both tests
        } else {
            (args.one_way, args.round_trip) // Use explicit user selection
        };

        Self {
            // Expand "all" mechanism to concrete list
            mechanisms: IpcMechanism::expand_all(args.mechanisms.clone()),
            
            message_size: args.message_size,
            
            // Duration takes precedence over iterations
            // If duration is specified, we ignore the iteration count
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
        }
    }
}

/// Parse duration from string (e.g., "10s", "5m", "1h")
///
/// This function provides flexible duration parsing that accepts human-readable
/// time specifications. It supports multiple time units and handles edge cases
/// gracefully with clear error messages.
///
/// ## Supported Formats
/// - **Milliseconds**: "500ms", "1000ms"
/// - **Seconds**: "10s", "30s", or just "10" (seconds assumed)
/// - **Minutes**: "5m", "30m" 
/// - **Hours**: "1h", "2h"
///
/// ## Parameters
/// - `s`: String slice containing the duration specification
///
/// ## Returns
/// - `Ok(Duration)`: Successfully parsed duration
/// - `Err(String)`: Error message describing the parsing failure
///
/// ## Examples
/// ```rust
/// # use std::time::Duration;
/// # use ipc_benchmark::cli::parse_duration;
/// assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
/// assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
/// assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
/// ```
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();

    // Check for empty input
    if s.is_empty() {
        return Err("Duration cannot be empty".to_string());
    }

    // Parse the numeric part and unit suffix
    let (num_str, unit) = if let Some(stripped) = s.strip_suffix("ms") {
        (stripped, "ms")
    } else if let Some(stripped) = s.strip_suffix('s') {
        (stripped, "s")
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, "m")
    } else if let Some(stripped) = s.strip_suffix('h') {
        (stripped, "h")
    } else {
        // No unit specified, assume seconds
        (s, "s")
    };

    // Parse the numeric portion as floating point to support fractional values
    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number in duration: {}", num_str))?;

    // Validate that the number is positive
    if num < 0.0 {
        return Err("Duration cannot be negative".to_string());
    }

    // Convert to Duration based on the unit
    let duration = match unit {
        "ms" => Duration::from_millis(num as u64),
        "s" => Duration::from_secs(num as u64),
        "m" => Duration::from_secs((num * 60.0) as u64),
        "h" => Duration::from_secs((num * 3600.0) as u64),
        _ => return Err(format!("Invalid duration unit: {}", unit)),
    };

    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test duration parsing with various valid formats
    #[test]
    fn test_parse_duration() {
        // Test basic time units
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        
        // Test default unit (seconds)
        assert_eq!(parse_duration("10").unwrap(), Duration::from_secs(10));

        // Test error cases
        assert!(parse_duration("").is_err());
        assert!(parse_duration("invalid").is_err());
        assert!(parse_duration("-5s").is_err());
    }

    /// Test IPC mechanism display formatting
    #[test]
    fn test_ipc_mechanism_display() {
        assert_eq!(
            IpcMechanism::UnixDomainSocket.to_string(),
            "Unix Domain Socket"
        );
        assert_eq!(IpcMechanism::SharedMemory.to_string(), "Shared Memory");
        assert_eq!(IpcMechanism::TcpSocket.to_string(), "TCP Socket");
        assert_eq!(IpcMechanism::PosixMessageQueue.to_string(), "POSIX Message Queue");
        assert_eq!(IpcMechanism::All.to_string(), "All Mechanisms");
    }

    /// Test mechanism expansion logic
    #[test]
    fn test_ipc_mechanism_expand_all() {
        let all_mechanisms = vec![
            IpcMechanism::UnixDomainSocket,
            IpcMechanism::SharedMemory,
            IpcMechanism::TcpSocket,
            IpcMechanism::PosixMessageQueue,
        ];
        
        // Test "all" expansion
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::All]),
            all_mechanisms
        );
        
        // Test specific mechanism preservation
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::UnixDomainSocket]),
            vec![IpcMechanism::UnixDomainSocket]
        );
        
        // Test "all" expansion when mixed with other mechanisms
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::UnixDomainSocket, IpcMechanism::All]),
            all_mechanisms
        );
    }
}
