// filepath: /home/dblack/git/redhat-performance/rusty-comms/src/logging.rs
use colored::*;
use std::fmt;
use tracing::{Event, Level, Subscriber};
// Correct the import paths for tracing_subscriber items.
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