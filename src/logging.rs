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
use std::fmt;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;

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
        // Buffer the formatted fields to apply color to the entire line.
        // This is necessary because the format_fields method writes directly.
        let mut buffer = String::new();
        let mut buf_writer = Writer::new(&mut buffer);
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
    }
}