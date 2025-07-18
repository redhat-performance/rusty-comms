use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// IPC Benchmark Suite - A comprehensive tool for measuring IPC performance
#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
pub struct Args {
    /// IPC mechanisms to benchmark (space-separated: uds, shm, tcp, or all)
    #[clap(short = 'm', value_enum, default_values_t = vec![IpcMechanism::UnixDomainSocket], help_heading = "Core Options", num_args = 1..)]
    pub mechanisms: Vec<IpcMechanism>,

    /// Message size in bytes
    #[clap(short = 's', long, default_value_t = crate::defaults::MESSAGE_SIZE)]
    pub message_size: usize,

    /// Number of iterations to run (ignored if duration is specified)
    #[clap(short = 'i', long, default_value_t = crate::defaults::ITERATIONS)]
    pub iterations: usize,

    /// Duration to run the benchmark (takes precedence over iterations)
    #[clap(short = 'd', long, value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// Number of concurrent processes/threads
    #[clap(short = 'c', long, default_value_t = crate::defaults::CONCURRENCY)]
    pub concurrency: usize,

    /// Output file for results (JSON format)
    #[clap(short = 'o', long, default_value = crate::defaults::OUTPUT_FILE)]
    pub output_file: PathBuf,

    /// Include one-way latency measurements
    #[clap(long, default_value_t = true)]
    pub one_way: bool,

    /// Include round-trip latency measurements
    #[clap(long, default_value_t = true)]
    pub round_trip: bool,

    /// Number of warmup iterations
    #[clap(short = 'w', long, default_value_t = crate::defaults::WARMUP_ITERATIONS)]
    pub warmup_iterations: usize,

    /// Continue running other benchmarks even if one fails
    #[clap(long, default_value_t = false)]
    pub continue_on_error: bool,

    /// Verbose output
    #[clap(short = 'v', long, default_value_t = false)]
    pub verbose: bool,

    /// JSON output file for streaming results during execution
    #[clap(long)]
    pub streaming_output: Option<PathBuf>,

    /// Percentiles to calculate for latency metrics
    #[clap(long, default_values_t = vec![50.0, 95.0, 99.0, 99.9])]
    pub percentiles: Vec<f64>,

    /// Buffer size for message queues and shared memory
    #[clap(long, default_value_t = 8192)]
    pub buffer_size: usize,

    /// Host address for TCP sockets
    #[clap(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Port for TCP sockets
    #[clap(long, default_value_t = 8080)]
    pub port: u16,
}

/// Available IPC mechanisms for benchmarking
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
pub enum IpcMechanism {
    /// Unix Domain Sockets
    #[clap(name = "uds")]
    UnixDomainSocket,

    /// Shared Memory
    #[clap(name = "shm")]
    SharedMemory,

    /// TCP Sockets
    #[clap(name = "tcp")]
    TcpSocket,

    /// POSIX Message Queues
    #[clap(name = "pmq")]
    PosixMessageQueue,

    /// All available mechanisms
    #[clap(name = "all")]
    All,
}

impl std::fmt::Display for IpcMechanism {
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
    pub fn expand_all(mechanisms: Vec<IpcMechanism>) -> Vec<IpcMechanism> {
        if mechanisms.contains(&IpcMechanism::All) {
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkConfiguration {
    pub mechanisms: Vec<IpcMechanism>,
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

impl From<&Args> for BenchmarkConfiguration {
    fn from(args: &Args) -> Self {
        Self {
            mechanisms: IpcMechanism::expand_all(args.mechanisms.clone()),
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
        }
    }
}

/// Parse duration from string (e.g., "10s", "5m", "1h")
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();

    if s.is_empty() {
        return Err("Duration cannot be empty".to_string());
    }

    let (num_str, unit) = if let Some(stripped) = s.strip_suffix("ms") {
        (stripped, "ms")
    } else if let Some(stripped) = s.strip_suffix('s') {
        (stripped, "s")
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, "m")
    } else if let Some(stripped) = s.strip_suffix('h') {
        (stripped, "h")
    } else {
        (s, "s") // Default to seconds
    };

    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number in duration: {}", num_str))?;

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

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("10").unwrap(), Duration::from_secs(10));

        assert!(parse_duration("").is_err());
        assert!(parse_duration("invalid").is_err());
    }

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

    #[test]
    fn test_ipc_mechanism_expand_all() {
        let all_mechanisms = vec![
            IpcMechanism::UnixDomainSocket,
            IpcMechanism::SharedMemory,
            IpcMechanism::TcpSocket,
            IpcMechanism::PosixMessageQueue,
        ];
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::All]),
            all_mechanisms
        );
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::UnixDomainSocket]),
            vec![IpcMechanism::UnixDomainSocket]
        );
        assert_eq!(
            IpcMechanism::expand_all(vec![IpcMechanism::UnixDomainSocket, IpcMechanism::All]),
            all_mechanisms
        );
    }
}
