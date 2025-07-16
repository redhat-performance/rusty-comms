//! # IPC Benchmark Suite
//!
//! A comprehensive interprocess communication benchmark suite implemented in Rust.
//! This library provides tools for measuring latency and throughput of various IPC mechanisms.

pub mod benchmark;
pub mod cli;
pub mod ipc;
pub mod metrics;
pub mod results;
pub mod utils;

pub use benchmark::{BenchmarkConfig, BenchmarkRunner};
pub use cli::{Args, IpcMechanism};
pub use ipc::{IpcTransport, Message};
pub use metrics::{LatencyMetrics, ThroughputMetrics};
pub use results::{BenchmarkResults, ResultsManager};

/// The current version of the IPC benchmark suite
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default configuration values
pub mod defaults {
    use std::time::Duration;

    /// Default message size in bytes
    pub const MESSAGE_SIZE: usize = 1024;

    /// Default number of iterations
    pub const ITERATIONS: usize = 10000;

    /// Default test duration
    pub const DURATION: Duration = Duration::from_secs(10);

    /// Default number of concurrent workers
    pub const CONCURRENCY: usize = 1;

    /// Default output file name
    pub const OUTPUT_FILE: &str = "benchmark_results.json";

    /// Default warmup iterations
    pub const WARMUP_ITERATIONS: usize = 1000;
}
