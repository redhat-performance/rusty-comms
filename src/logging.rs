//! # Logging Configuration Module
//!
//! This module provides centralized logging setup and management for the
//! benchmark suite. It configures the `tracing` framework to provide
//! flexible, structured, and performant logging to both the console and
//! optional log files.
//!
//! ## Key Features
//!
//! - **Dual Output**: Supports simultaneous logging to both the console
//!   (stdout) and a dedicated log file.
//! - **Level Control**: Allows independent configuration of log levels for
//!   console and file outputs.
//! - **Dynamic Filtering**: Uses `tracing_subscriber` to allow log levels
//!   to be set via environment variables (e.g., `RUST_LOG`).
//! - **Human-Readable Format**: Configures a clean, readable format for
//!   console output to improve developer experience.
//!
//! ## Usage
//!
//! The primary function, `init_logging`, should be called once at the
//! beginning of the application's `main` function to set up the global
//! logger.
//!
//! ```rust,ignore
//! // In main.rs
//! use ipc_benchmark::logging;
//!
//! fn main() -> anyhow::Result<()> {
//!     let log_file = Some("benchmark.log".to_string());
//!     logging::init_logging(log_level, &log_file)?;
//!     // ... rest of the application
//!     Ok(())
//! }
//! ```

use colored::*;
use std::cell::RefCell;
use std::fmt;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;

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
}
