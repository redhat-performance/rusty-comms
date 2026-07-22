//! # Logging Configuration Module
//!
//! Centralised logging setup for the IPC benchmark suite.  All
//! subscriber initialization flows through [`init_logging`] (or
//! [`try_init_logging`] when a subscriber may already be set).
//!
//! ## Features
//!
//! - **Dual output**: simultaneous logging to a rolling daily file
//!   (or stderr) *and* colorised stdout for user-facing output.
//! - **Verbosity control**: maps `-v` / `-vv` flags to INFO / DEBUG /
//!   TRACE via [`LogConfig::verbose`].
//! - **Quiet mode**: suppresses the stdout layer entirely.
//! - **Server subprocess mode**: minimal stderr-only logging at DEBUG
//!   level to avoid interfering with stdout pipe signalling.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use ipc_benchmark::logging::{init_logging, LogConfig};
//!
//! fn main() -> anyhow::Result<()> {
//!     let config = LogConfig {
//!         verbose: 1,
//!         quiet: false,
//!         log_file: None,
//!         is_server_subprocess: false,
//!     };
//!     let _guard = init_logging(&config)?;
//!     // ... rest of the application
//!     Ok(())
//! }
//! ```

use anyhow::Result;
use colored::*;
use std::cell::RefCell;
use std::fmt;
use tracing::{Event, Level, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Configuration for the logging subsystem.
///
/// Construct this from CLI arguments and pass to [`init_logging`] or
/// [`try_init_logging`].
///
/// ## Fields
///
/// * `verbose` — verbosity level: 0 = INFO, 1 = DEBUG, 2+ = TRACE
/// * `quiet` — if true, suppress the colorised stdout layer
/// * `log_file` — destination for the detailed log layer:
///   - `None` — daily-rotating file in current directory
///   - `Some("stderr")` — write to stderr
///   - `Some(path)` — daily-rotating file at the given path
/// * `is_server_subprocess` — if true, use a minimal stderr-only
///   subscriber at DEBUG level (ignores other fields)
pub struct LogConfig {
    pub verbose: u8,
    pub quiet: bool,
    pub log_file: Option<String>,
    pub is_server_subprocess: bool,
}

/// Initialize the global tracing subscriber.
///
/// This must be called exactly once per process.  Returns an optional
/// [`WorkerGuard`] that the caller must keep alive for the duration
/// of the program when file logging is active (dropping it flushes
/// and closes the log file).
///
/// # Errors
///
/// Returns an error if the subscriber cannot be initialised (e.g.
/// one is already set).
pub fn init_logging(config: &LogConfig) -> Result<Option<WorkerGuard>> {
    // Server subprocess: minimal stderr, no file, no stdout layer.
    if config.is_server_subprocess {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::DEBUG)
            .init();
        return Ok(None);
    }

    let log_level = verbosity_to_level(config.verbose);

    let (detailed_log_layer, guard) = build_detailed_layer(config.log_file.as_deref(), log_level)?;

    let stdout_log = if !config.quiet {
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .event_format(ColorizedFormatter)
                .with_filter(log_level),
        )
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(detailed_log_layer)
        .with(stdout_log)
        .init();

    Ok(guard)
}

/// Attempt to initialize logging, silently succeeding if a
/// subscriber is already registered.
///
/// Used by standalone server/client paths which may be invoked
/// after the parent process has already set up logging.
///
/// # Returns
///
/// `Ok(())` regardless of whether a new subscriber was installed.
pub fn try_init_logging(config: &LogConfig) -> Result<()> {
    if config.quiet {
        return Ok(());
    }

    let log_level = verbosity_to_level(config.verbose);

    // Standalone paths use stderr + colorised output only.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(log_level)
        .event_format(ColorizedFormatter)
        .try_init();

    Ok(())
}

/// Map the `-v` count to a [`LevelFilter`].
fn verbosity_to_level(verbose: u8) -> LevelFilter {
    match verbose {
        0 => LevelFilter::INFO,
        1 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    }
}

/// Build the detailed (file or stderr) log layer and optional guard.
fn build_detailed_layer(
    log_file: Option<&str>,
    level: LevelFilter,
) -> Result<(
    Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>,
    Option<WorkerGuard>,
)> {
    if let Some("stderr") = log_file {
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(level)
            .boxed();
        return Ok((layer, None));
    }

    let file_appender = match log_file {
        Some(path_str) => {
            let log_path = std::path::Path::new(path_str);
            let log_dir = log_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            let log_filename = log_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("ipc_benchmark.log"));
            tracing_appender::rolling::daily(log_dir, log_filename)
        }
        None => tracing_appender::rolling::daily(".", "ipc_benchmark.log"),
    };

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_filter(level)
        .boxed();

    Ok((layer, Some(guard)))
}

// A thread-local buffer for formatting log messages to avoid allocations on every event.
// A generous capacity is chosen to prevent reallocations for most log messages.
thread_local! {
    static BUF: RefCell<String> = RefCell::new(String::with_capacity(1024));
}

/// A custom tracing event formatter for colorizing log output based on level.
///
/// This formatter is designed to provide clean, user-facing output where the
/// entire log line is colored according to its severity level, without any
/// extra metadata like timestamps or log levels printed.
pub struct ColorizedFormatter;

impl<S, N> FormatEvent<S, N> for ColorizedFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        BUF.with(|buf| {
            let mut buffer = buf.borrow_mut();
            // Clear the buffer to reuse it for the current event.
            buffer.clear();

            let mut buf_writer = Writer::new(&mut *buffer);
            ctx.format_fields(buf_writer.by_ref(), event)?;

            // Apply color based on the event's log level.
            let colored_output = match *event.metadata().level() {
                Level::INFO => buffer.white(),
                Level::WARN => buffer.yellow(),
                Level::ERROR => buffer.red(),
                Level::DEBUG => buffer.blue(),
                Level::TRACE => buffer.purple(),
            };

            // Write the colored line to the actual output.
            writeln!(writer, "{}", colored_output)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tracing_subscriber::fmt::MakeWriter;

    /// A simple writer that captures output to a shared buffer for testing.
    #[derive(Clone)]
    struct TestWriter {
        buffer: std::sync::Arc<Mutex<Vec<u8>>>,
    }

    impl TestWriter {
        fn new() -> Self {
            Self {
                buffer: std::sync::Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_output(&self) -> String {
            let buf = self.buffer.lock().unwrap();
            String::from_utf8_lossy(&buf).to_string()
        }
    }

    impl std::io::Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.buffer.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for TestWriter {
        type Writer = TestWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn test_colorized_formatter_formats_messages() {
        let writer = TestWriter::new();
        let writer_clone = writer.clone();

        // Create a subscriber with our colorized formatter
        let subscriber = tracing_subscriber::fmt()
            .event_format(ColorizedFormatter)
            .with_writer(writer_clone)
            .with_max_level(Level::TRACE)
            .finish();

        // Use the subscriber for this test only
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("test info message");
            tracing::warn!("test warning message");
            tracing::error!("test error message");
            tracing::debug!("test debug message");
            tracing::trace!("test trace message");
        });

        let output = writer.get_output();

        // Verify messages were formatted
        assert!(
            output.contains("test info message"),
            "Should contain info message, got: {}",
            output
        );
        assert!(
            output.contains("test warning message"),
            "Should contain warning message"
        );
        assert!(
            output.contains("test error message"),
            "Should contain error message"
        );
        assert!(
            output.contains("test debug message"),
            "Should contain debug message"
        );
        assert!(
            output.contains("test trace message"),
            "Should contain trace message"
        );
    }

    #[test]
    fn test_colorized_formatter_struct_exists() {
        // Simple test to verify the struct can be instantiated
        let _formatter = ColorizedFormatter;
    }

    #[test]
    fn test_thread_local_buffer_reuse() {
        // Test that the thread-local buffer can be accessed
        BUF.with(|buf| {
            let mut buffer = buf.borrow_mut();
            buffer.clear();
            buffer.push_str("test");
            assert_eq!(&*buffer, "test");
            buffer.clear();
            assert!(buffer.is_empty());
        });
    }

    #[test]
    fn test_verbosity_to_level() {
        assert_eq!(verbosity_to_level(0), LevelFilter::INFO);
        assert_eq!(verbosity_to_level(1), LevelFilter::DEBUG);
        assert_eq!(verbosity_to_level(2), LevelFilter::TRACE);
    }

    #[test]
    fn test_build_detailed_layer_stderr() {
        let result = build_detailed_layer(Some("stderr"), LevelFilter::INFO);
        assert!(result.is_ok());
        let (_layer, guard) = result.unwrap();
        assert!(guard.is_none(), "stderr path should not produce a guard");
    }

    #[test]
    fn test_try_init_logging_quiet_noop() {
        let config = LogConfig {
            quiet: true,
            verbose: 0,
            log_file: None,
            is_server_subprocess: false,
        };
        assert!(try_init_logging(&config).is_ok());
    }
}
