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
//! use ipc_benchmark::{BenchmarkRunner, BenchmarkConfig, IpcMechanism, cli::Args};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     #[cfg(unix)]
//!     {
//!         let args = Args::default();
//!         let config = BenchmarkConfig {
//!             mechanism: IpcMechanism::UnixDomainSocket,
//!             message_size: 1024,
//!             msg_count: Some(10000),
//!             duration: None,
//!             concurrency: 1,
//!             one_way: true,
//!             round_trip: false,
//!             warmup_iterations: 100,
//!             percentiles: vec![50.0, 95.0, 99.0],
//!             buffer_size: Some(8192),
//!             host: "127.0.0.1".to_string(),
//!             port: 8080,
//!             send_delay: None,
//!             pmq_priority: 0,
//!             include_first_message: false,
//!             server_affinity: None,
//!             client_affinity: None,
//!         };
//!     
//!         let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket, args);
//!         // let results = runner.run(None).await?;
//!     
//!         // println!("Average latency: {:?}", results.summary.average_latency_ns);
//!     }
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

/// Blocking benchmarking functionality
///
/// Contains the `BlockingBenchmarkRunner` that provides synchronous/blocking
/// execution of performance tests. This module mirrors the async benchmark
/// module but uses standard library blocking I/O instead of Tokio. Features:
/// - Pure standard library implementation (no async/await or Tokio)
/// - Identical measurement methodology to async version for fair comparison
/// - Uses `BlockingTransport` trait for blocking I/O operations
/// - Supports all IPC mechanisms in blocking mode
pub mod benchmark_blocking;

/// Command-line interface and configuration
///
/// Provides argument parsing using clap and converts user-friendly CLI options
/// into internal configuration structures. Includes:
/// - Comprehensive argument validation and type checking
/// - Duration parsing with human-readable formats (e.g., "10s", "5m")
/// - Mechanism selection with "all" expansion capability
/// - Output file and streaming configuration
pub mod cli;

/// Execution mode configuration
///
/// Defines the execution model (async vs blocking) for IPC operations.
/// The benchmark supports both Tokio-based async I/O and standard library
/// blocking I/O, allowing for direct performance comparison between the two
/// approaches. The mode is selected at runtime via CLI flags.
pub mod execution_mode;

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

/// Blocking results management module
///
/// This module provides result collection, aggregation, and output
/// functionality using synchronous/blocking I/O operations. It mirrors
/// the async `results` module but uses standard library blocking I/O.
///
/// ## Key Features
///
/// - JSON and CSV output formats
/// - Real-time streaming results during execution (blocking I/O)
/// - Cross-mechanism comparison and ranking
/// - System information collection for reproducibility
pub mod results_blocking;

/// Container management for host-to-container IPC benchmarking
///
/// This module provides Podman container lifecycle management for running
/// IPC benchmark clients inside containers. All operations use
/// `std::process::Command` to invoke Podman CLI — no shell scripts.
///
/// ## Supported IPC Mechanisms
///
/// - **UDS**: Mount socket directory for Unix Domain Sockets
/// - **SHM**: Use `--ipc=host` for shared memory access
/// - **PMQ**: Mount `/dev/mqueue` for POSIX message queues
///
/// ## Key Types
///
/// - `ContainerManager`: Manages container lifecycle (create, start, stop)
/// - `ContainerConfig`: Mechanism-specific container configuration
pub mod container;

/// Host-container benchmark runner
///
/// This module provides infrastructure for running IPC benchmarks between
/// a host (driver) and a Podman container (responder). The host drives the
/// tests while the container acts as the IPC server/receiver.
///
/// ## Architecture
///
/// - Host: Runs on the bare metal, drives tests, collects results
/// - Container: Runs inside Podman, acts as IPC server (receiver/responder)
///
/// ## Key Types
///
/// - `HostBenchmarkRunner`: Manages container spawn and benchmark execution
pub mod host_container;

pub mod utils;

// Re-export commonly used utilities for convenient access
pub use utils::{get_temp_dir, get_temp_socket_path};

// Re-export key types for convenient library usage
// These are the primary types that library users will interact with

/// Main benchmark execution engine
///
/// Re-exported from the benchmark module for easy access. The `BenchmarkRunner`
/// is the primary interface for executing performance tests in async mode.
/// `BlockingBenchmarkRunner` provides the blocking/synchronous execution mode.
pub use benchmark::{BenchmarkConfig, BenchmarkRunner};
pub use benchmark_blocking::BlockingBenchmarkRunner;

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
/// structure and the `ResultsManager` for output handling (async mode) and
/// `BlockingResultsManager` for blocking mode.
pub use results::{BenchmarkResults, ResultsManager};
pub use results_blocking::BlockingResultsManager;

/// Container management types
///
/// Re-exported from the container module for managing Podman containers
/// for host-to-container IPC benchmarking.
pub use container::{ContainerConfig, ContainerManager};

/// Host-container benchmark runner
///
/// Re-exported for running benchmarks between host and container.
pub use host_container::HostBenchmarkRunner;

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
