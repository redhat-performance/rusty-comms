use anyhow::Result;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Generate a unique identifier for test runs
pub fn generate_test_id() -> String {
    Uuid::new_v4().to_string()
}

/// Get current timestamp as nanoseconds since Unix epoch
pub fn current_timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Convert nanoseconds to a human-readable duration string
pub fn format_duration_ns(ns: u64) -> String {
    let duration = Duration::from_nanos(ns);
    format_duration(duration)
}

/// Format a duration in a human-readable way
pub fn format_duration(duration: Duration) -> String {
    let total_ns = duration.as_nanos();

    if total_ns < 1_000 {
        format!("{}ns", total_ns)
    } else if total_ns < 1_000_000 {
        format!("{:.2}μs", total_ns as f64 / 1_000.0)
    } else if total_ns < 1_000_000_000 {
        format!("{:.2}ms", total_ns as f64 / 1_000_000.0)
    } else if total_ns < 60_000_000_000 {
        format!("{:.2}s", total_ns as f64 / 1_000_000_000.0)
    } else {
        let seconds = duration.as_secs();
        let minutes = seconds / 60;
        let remaining_seconds = seconds % 60;

        if minutes < 60 {
            format!("{}m {}s", minutes, remaining_seconds)
        } else {
            let hours = minutes / 60;
            let remaining_minutes = minutes % 60;
            format!("{}h {}m {}s", hours, remaining_minutes, remaining_seconds)
        }
    }
}

/// Format bytes in a human-readable way
pub fn format_bytes(bytes: usize) -> String {
    format_bytes_f64(bytes as f64)
}

/// Format bytes (as f64) in a human-readable way
pub fn format_bytes_f64(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{:.0} B", bytes)
    } else if bytes < 1024.0 * 1024.0 {
        format!("{:.2} KB", bytes / 1024.0)
    } else if bytes < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} MB", bytes / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format a rate (bytes per second) in a human-readable way
pub fn format_rate(bytes_per_second: f64) -> String {
    format!("{}/s", format_bytes_f64(bytes_per_second))
}

/// Format a message rate in a human-readable way
pub fn format_message_rate(messages_per_second: f64) -> String {
    if messages_per_second < 1000.0 {
        format!("{:.0} msg/s", messages_per_second)
    } else if messages_per_second < 1_000_000.0 {
        format!("{:.2}K msg/s", messages_per_second / 1000.0)
    } else {
        format!("{:.2}M msg/s", messages_per_second / 1_000_000.0)
    }
}

/// Calculate statistics from a vector of values
pub fn calculate_stats(values: &[f64]) -> (f64, f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let sum: f64 = values.iter().sum();
    let count = values.len() as f64;
    let mean = sum / count;

    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let variance = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count;
    let std_dev = variance.sqrt();

    (mean, min, max, std_dev)
}

/// Calculate percentiles from a vector of values
pub fn calculate_percentiles(values: &[f64], percentiles: &[f64]) -> Vec<(f64, f64)> {
    if values.is_empty() {
        return percentiles.iter().map(|&p| (p, 0.0)).collect();
    }

    let mut sorted_values = values.to_vec();
    sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

    percentiles
        .iter()
        .map(|&p| {
            let index = (p / 100.0) * (sorted_values.len() - 1) as f64;
            let lower_index = index.floor() as usize;
            let upper_index = index.ceil() as usize;

            if lower_index == upper_index {
                (p, sorted_values[lower_index])
            } else {
                let lower_value = sorted_values[lower_index];
                let upper_value = sorted_values[upper_index];
                let weight = index - lower_index as f64;
                (p, lower_value + weight * (upper_value - lower_value))
            }
        })
        .collect()
}

/// Validate that a port number is in the valid range
pub fn validate_port(port: u16) -> Result<()> {
    if port < 1024 {
        anyhow::bail!("Port number {} is too low (below 1024)", port);
    }
    // Note: No need to check > 65535 since u16 max is 65535
    Ok(())
}

/// Validate that a buffer size is reasonable
pub fn validate_buffer_size(buffer_size: usize) -> Result<()> {
    if buffer_size < 1024 {
        anyhow::bail!(
            "Buffer size {} is too small (minimum 1024 bytes)",
            buffer_size
        );
    }
    if buffer_size > 1024 * 1024 * 1024 {
        anyhow::bail!("Buffer size {} is too large (maximum 1GB)", buffer_size);
    }
    Ok(())
}

/// Validate that a message size is reasonable
pub fn validate_message_size(message_size: usize) -> Result<()> {
    if message_size == 0 {
        anyhow::bail!("Message size cannot be zero");
    }
    if message_size > 16 * 1024 * 1024 {
        anyhow::bail!("Message size {} is too large (maximum 16MB)", message_size);
    }
    Ok(())
}

/// Validate that concurrency level is reasonable
pub fn validate_concurrency(concurrency: usize) -> Result<()> {
    if concurrency == 0 {
        anyhow::bail!("Concurrency cannot be zero");
    }
    if concurrency > 1024 {
        anyhow::bail!("Concurrency {} is too high (maximum 1024)", concurrency);
    }
    Ok(())
}

/// Get the number of CPU cores available
pub fn get_cpu_cores() -> usize {
    num_cpus::get()
}

/// Get recommended concurrency based on available CPU cores
pub fn get_recommended_concurrency() -> usize {
    let cores = get_cpu_cores();
    // Use number of cores, but cap at 8 for reasonable defaults
    cores.min(8)
}

/// Check if we're running in a container environment
pub fn is_container_environment() -> bool {
    std::fs::read_to_string("/proc/1/cgroup")
        .map(|contents| contents.contains("docker") || contents.contains("lxc"))
        .unwrap_or(false)
}

/// Get available memory in bytes (simplified estimation)
pub fn get_available_memory_bytes() -> usize {
    // This is a very simplified estimation
    // In a real implementation, you'd want to use proper system info
    if is_container_environment() {
        2 * 1024 * 1024 * 1024 // 2GB default for containers
    } else {
        16 * 1024 * 1024 * 1024 // 16GB default for native
    }
}

/// Clean up temporary files matching a pattern
pub fn cleanup_temp_files(pattern: &str) -> Result<()> {
    let temp_dir = std::env::temp_dir();

    if let Ok(entries) = std::fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.contains(pattern) {
                    if let Err(e) = std::fs::remove_file(entry.path()) {
                        tracing::warn!("Failed to remove temp file {:?}: {}", entry.path(), e);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Print a formatted table row
pub fn print_table_row(columns: &[&str], widths: &[usize]) {
    print!("|");
    for (i, column) in columns.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(10);
        print!(" {:width$} |", column, width = width);
    }
    println!();
}

/// Print a table separator
pub fn print_table_separator(widths: &[usize]) {
    print!("+");
    for &width in widths {
        print!("{}", "-".repeat(width + 2));
        print!("+");
    }
    println!();
}

/// Create a progress bar-like indicator
pub fn create_progress_indicator(current: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return "█".repeat(width);
    }

    let progress = (current as f64 / total as f64).min(1.0);
    let filled = ((progress * width as f64) as usize).min(width);
    let empty = width - filled;

    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_ns() {
        assert_eq!(format_duration_ns(500), "500ns");
        assert_eq!(format_duration_ns(1500), "1.50μs");
        assert_eq!(format_duration_ns(1_500_000), "1.50ms");
        assert_eq!(format_duration_ns(1_500_000_000), "1.50s");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1572864), "1.50 MB");
        assert_eq!(format_bytes(1610612736), "1.50 GB");
    }

    #[test]
    fn test_format_rate() {
        assert_eq!(format_rate(1024.0), "1.00 KB/s");
        assert_eq!(format_rate(1048576.0), "1.00 MB/s");
    }

    #[test]
    fn test_format_message_rate() {
        assert_eq!(format_message_rate(500.0), "500 msg/s");
        assert_eq!(format_message_rate(1500.0), "1.50K msg/s");
        assert_eq!(format_message_rate(1500000.0), "1.50M msg/s");
    }

    #[test]
    fn test_calculate_stats() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (mean, min, max, std_dev) = calculate_stats(&values);

        assert_eq!(mean, 3.0);
        assert_eq!(min, 1.0);
        assert_eq!(max, 5.0);
        assert!((std_dev - 1.4142135623730951).abs() < 0.001);
    }

    #[test]
    fn test_calculate_percentiles() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let percentiles = calculate_percentiles(&values, &[50.0, 95.0]);

        assert_eq!(percentiles.len(), 2);
        assert_eq!(percentiles[0].0, 50.0);
        assert_eq!(percentiles[0].1, 3.0);
        assert_eq!(percentiles[1].0, 95.0);
        assert_eq!(percentiles[1].1, 4.8);
    }

    #[test]
    fn test_validate_port() {
        assert!(validate_port(1024).is_ok());
        assert!(validate_port(8080).is_ok());
        assert!(validate_port(65535).is_ok());
        assert!(validate_port(1023).is_err());
        assert!(validate_port(65536).is_err());
    }

    #[test]
    fn test_validate_buffer_size() {
        assert!(validate_buffer_size(1024).is_ok());
        assert!(validate_buffer_size(8192).is_ok());
        assert!(validate_buffer_size(1023).is_err());
        assert!(validate_buffer_size(1024 * 1024 * 1024 + 1).is_err());
    }

    #[test]
    fn test_validate_message_size() {
        assert!(validate_message_size(1).is_ok());
        assert!(validate_message_size(1024).is_ok());
        assert!(validate_message_size(0).is_err());
        assert!(validate_message_size(16 * 1024 * 1024 + 1).is_err());
    }

    #[test]
    fn test_validate_concurrency() {
        assert!(validate_concurrency(1).is_ok());
        assert!(validate_concurrency(8).is_ok());
        assert!(validate_concurrency(0).is_err());
        assert!(validate_concurrency(1025).is_err());
    }

    #[test]
    fn test_get_cpu_cores() {
        assert!(get_cpu_cores() > 0);
    }

    #[test]
    fn test_get_recommended_concurrency() {
        let concurrency = get_recommended_concurrency();
        assert!(concurrency > 0);
        assert!(concurrency <= 8);
    }

    #[test]
    fn test_create_progress_indicator() {
        assert_eq!(create_progress_indicator(0, 100, 10), "░░░░░░░░░░");
        assert_eq!(create_progress_indicator(50, 100, 10), "█████░░░░░");
        assert_eq!(create_progress_indicator(100, 100, 10), "██████████");
    }
}
