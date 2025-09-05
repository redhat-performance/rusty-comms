//! # IPC Benchmark Suite Library
//!
//! A comprehensive interprocess communication benchmark suite implemented in Rust.
//! This library provides tools for measuring latency and throughput of various IPC mechanisms.
//!
//! ## Supported IPC Mechanisms
//!
//! The library supports benchmarking of the following IPC mechanisms:
//!
//! - **Unix Domain Sockets (UDS)**: High-performance local sockets with full duplex communication
//! - **Shared Memory (SHM)**: Direct memory sharing with custom ring buffer implementation  
//! - **TCP Sockets**: Network sockets with configurable buffer sizes and socket options
//! - **POSIX Message Queues (PMQ)**: System-level message queues with priority support
//!
//! ## Architecture Overview
//!
//! The library is organized into several key modules:
//!
//! - `benchmark`: Core benchmarking engine and test execution logic
//! - `cli`: Command-line interface parsing and configuration management
//! - `ipc`: Transport abstraction layer and specific IPC implementations
//! - `metrics`: Performance measurement using HDR histograms and statistical analysis
//! - `results`: Result aggregation, formatting, and output management
//! - `utils`: Utility functions for formatting, validation, and system information
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use ipc_benchmark::{BenchmarkRunner, BenchmarkConfig, IpcMechanism};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = BenchmarkConfig {
//!         mechanism: IpcMechanism::UnixDomainSocket,
//!         message_size: 1024,
//!         msg_count: Some(10000),
//!         duration: None,
//!         concurrency: 1,
//!         one_way: true,
//!         round_trip: false,
//!         warmup_iterations: 100,
//!         percentiles: vec![50.0, 95.0, 99.0],
//!         buffer_size: 8192,
//!         host: "127.0.0.1".to_string(),
//!         port: 8080,
//!     };
//!     
//!     let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket);
//!     let results = runner.run(None).await?;
//!     
//!     println!("Average latency: {:?}", results.summary.average_latency_ns);
//!     Ok(())
//! }
//! ```
//!
//! ## Performance Characteristics
//!
//! The library is designed for high-performance benchmarking with:
//!
//! - **Zero-copy serialization** where possible using bincode
//! - **HDR histograms** for accurate latency measurement without coordination omission
//! - **Async I/O** throughout using Tokio for scalable concurrent operations
//! - **Configurable buffer sizes** and queue depths for optimal performance tuning
//! - **Comprehensive metrics** including percentiles, throughput, and error rates

/// Core benchmarking functionality
///
/// Contains the main `BenchmarkRunner` and `BenchmarkConfig` types that orchestrate
/// performance testing. The benchmark module handles:
/// - Test execution lifecycle (warmup, measurement, cleanup)
/// - Concurrency management for multi-threaded scenarios  
/// - Resource management and cleanup between tests
/// - Adaptive configuration based on mechanism capabilities
pub mod benchmark;

/// Command-line interface and configuration
///
/// Provides argument parsing using clap and converts user-friendly CLI options
/// into internal configuration structures. Includes:
/// - Comprehensive argument validation and type checking
/// - Duration parsing with human-readable formats (e.g., "10s", "5m")
/// - Mechanism selection with "all" expansion capability
/// - Output file and streaming configuration
pub mod cli;

/// IPC transport implementations and abstractions
///
/// Contains the core transport abstraction (`IpcTransport` trait) and specific
/// implementations for each supported IPC mechanism. Features:
/// - Unified interface for all IPC mechanisms via trait abstraction
/// - Multi-client support where applicable (UDS, TCP, SHM)
/// - Message serialization/deserialization using bincode
/// - Transport-specific optimizations (e.g., TCP_NODELAY, ring buffers)
pub mod ipc;

/// Performance measurement and statistical analysis
///
/// Implements comprehensive performance metrics collection using HDR histograms
/// and statistical analysis. Provides:
/// - Latency measurement with nanosecond precision
/// - Throughput calculation for messages and bytes per second
/// - Percentile analysis (P50, P95, P99, P99.9, etc.)
/// - Histogram aggregation for multi-worker scenarios
pub mod metrics;

/// Result collection, aggregation, and output formatting
///
/// Manages the collection and presentation of benchmark results with support for:
/// - Structured JSON output with comprehensive metadata
/// - Real-time streaming results during execution
/// - Cross-mechanism comparison and ranking
/// - System information collection for reproducibility
pub mod results;

pub mod utils;

// Re-export key types for convenient library usage
// These are the primary types that library users will interact with

/// Main benchmark execution engine
///
/// Re-exported from the benchmark module for easy access. The `BenchmarkRunner`
/// is the primary interface for executing performance tests.
pub use benchmark::{BenchmarkConfig, BenchmarkRunner};

/// Command-line interface types
///
/// Re-exported for applications that want to use the same CLI parsing logic
/// or need access to the `IpcMechanism` enumeration for programmatic usage.
pub use cli::{Args, IpcMechanism};

/// Core IPC abstractions
///
/// The `IpcTransport` trait and `Message` structure are the fundamental
/// building blocks for implementing new IPC mechanisms or custom transports.
pub use ipc::{IpcTransport, Message};

/// Performance measurement types
///
/// Essential metrics types for collecting and analyzing benchmark results.
/// These provide detailed latency and throughput measurements.
pub use metrics::{LatencyMetrics, ThroughputMetrics};

/// Result collection and management
///
/// Key types for handling benchmark results, including the main `BenchmarkResults`
/// structure and the `ResultsManager` for output handling.
pub use results::{BenchmarkResults, ResultsManager};

/// The current version of the IPC benchmark suite
///
/// This version string is automatically populated from Cargo.toml and used
/// in result output for reproducibility and debugging purposes.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default configuration values
///
/// This module provides sensible defaults for all configurable parameters.
/// These values are chosen based on common usage patterns and performance
/// characteristics of typical IPC workloads.
pub mod defaults {
    use std::time::Duration;

    /// Default message size in bytes
    ///
    /// 1KB is chosen as a reasonable balance between:
    /// - Small enough to test low-latency scenarios  
    /// - Large enough to measure meaningful throughput
    /// - Typical of many real-world message payloads
    pub const MESSAGE_SIZE: usize = 1024;

    /// Default number of messages to send
    ///
    /// 10,000 messages provides sufficient statistical significance
    /// while keeping test duration reasonable. This can be overridden
    /// with the `--msg-count` flag for longer or shorter tests.
    pub const MSG_COUNT: usize = 10000;

    /// Default test duration
    ///
    /// When using duration-based testing instead of message-count-based,
    /// 10 seconds provides enough time for performance to stabilize
    /// while keeping total benchmark time manageable.
    pub const DURATION: Duration = Duration::from_secs(10);

    /// Default number of concurrent workers
    ///
    /// Single-threaded operation is the default to avoid complications
    /// with thread synchronization and resource contention. Multi-threaded
    /// scenarios can be enabled with the `--concurrency` flag.
    pub const CONCURRENCY: usize = 1;

    /// Default output file name
    ///
    /// Results are written in JSON format for easy parsing and analysis
    /// by external tools. The filename clearly indicates the content type.
    pub const OUTPUT_FILE: &str = "benchmark_results.json";

    /// Default warmup iterations
    ///
    /// 1,000 warmup iterations help stabilize performance by:
    /// - Allowing JIT compilation to optimize hot paths
    /// - Filling CPU caches with relevant data
    /// - Establishing network connections and OS buffers
    /// - Reducing measurement variance from cold-start effects
    pub const WARMUP_ITERATIONS: usize = 1000;
}
