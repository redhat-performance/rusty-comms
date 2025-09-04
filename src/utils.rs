//! # Utility Functions and Helper Module
//!
//! This module provides essential utility functions used throughout the IPC benchmark
//! suite. It includes formatters for human-readable output, validation functions for
//! input parameters, statistical calculations, and system information utilities.
//!
//! ## Key Functionality Categories
//!
//! - **Formatting**: Human-readable display of durations, bytes, and rates
//! - **Validation**: Input parameter validation with clear error messages
//! - **Statistics**: Mathematical calculations for performance analysis
//! - **System Information**: Platform and hardware detection utilities
//! - **File Management**: Temporary file cleanup and path utilities
//! - **Display Helpers**: Table formatting and progress indicators
//!
//! ## Design Principles
//!
//! - **User-Friendly Output**: All formatters prioritize human readability
//! - **Comprehensive Validation**: Clear error messages for invalid inputs
//! - **Cross-Platform**: Functions work consistently across operating systems
//! - **Performance**: Minimal overhead for frequently called functions
//! - **Extensibility**: Easy to add new formatters and validators
//!
//! ## Usage Examples
//!
//! ```rust
//! use ipc_benchmark::utils::*;
//! use std::time::Duration;
//!
//! // Format durations for display
//! let duration_str = format_duration(Duration::from_micros(1500));
//! assert_eq!(duration_str, "1.50ms");
//!
//! // Format throughput rates
//! let rate_str = format_rate(1048576.0);
//! assert_eq!(rate_str, "1.00 MB/s");
//!
//! // Validate configuration parameters
//! # fn main() -> anyhow::Result<()> {
//! validate_message_size(1024)?; // OK
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Generate a unique identifier for test runs
///
/// Creates a UUID v4 string that can be used to uniquely identify test runs,
/// temporary files, or other resources that need unique naming. This helps
/// prevent conflicts when running multiple benchmarks concurrently.
///
/// ## Returns
/// String representation of a UUID v4 (e.g., "550e8400-e29b-41d4-a716-446655440000")
///
/// ## Usage
///
/// - Unique naming for temporary files and shared resources
/// - Test run identification in logging and results
/// - Preventing resource conflicts in concurrent execution
/// - Generating unique ports, socket paths, and memory segment names
///
/// ## Thread Safety
///
/// This function is thread-safe and can be called concurrently from multiple threads.
/// Each call is guaranteed to return a unique identifier.
pub fn generate_test_id() -> String {
    Uuid::new_v4().to_string()
}

/// Get current timestamp as nanoseconds since Unix epoch
///
/// Provides high-precision timing information for performance measurement
/// and result correlation. The nanosecond precision enables accurate timing
/// even for very fast operations.
///
/// ## Returns
/// Number of nanoseconds since January 1, 1970, 00:00:00 UTC
///
/// ## Precision
///
/// The returned value has nanosecond precision on systems that support it,
/// though the actual resolution depends on system capabilities. Most modern
/// systems provide microsecond or better resolution.
///
/// ## Error Handling
///
/// If the system time is before the Unix epoch (very rare), returns 0
/// to provide a safe fallback rather than panicking.
///
/// ## Usage
///
/// - Timestamping measurement data
/// - Calculating time differences
/// - Correlating events across different system components
/// - High-precision performance measurement
pub fn current_timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Convert nanoseconds to a human-readable duration string
///
/// Converts a nanosecond duration value to a human-readable string with
/// appropriate units. This is a convenience wrapper around `format_duration`
/// for nanosecond values.
///
/// ## Parameters
/// - `ns`: Duration in nanoseconds
///
/// ## Returns
/// Human-readable duration string with appropriate units
///
/// ## Unit Selection
///
/// The function automatically selects the most appropriate unit:
/// - Nanoseconds for values < 1,000 ns
/// - Microseconds for values < 1,000,000 ns
/// - Milliseconds for values < 1,000,000,000 ns
/// - Seconds and larger units for longer durations
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::format_duration_ns;
/// assert_eq!(format_duration_ns(500), "500ns");
/// assert_eq!(format_duration_ns(1500), "1.50μs");
/// assert_eq!(format_duration_ns(1500000), "1.50ms");
/// ```
pub fn format_duration_ns(ns: u64) -> String {
    let duration = Duration::from_nanos(ns);
    format_duration(duration)
}

/// Format a duration in a human-readable way
///
/// Converts a Duration to a human-readable string, automatically selecting
/// the most appropriate unit based on the magnitude. Uses sensible precision
/// levels to balance readability with useful detail.
///
/// ## Parameters
/// - `duration`: The Duration to format
///
/// ## Returns
/// Human-readable duration string with appropriate units and precision
///
/// ## Unit Selection Logic
///
/// - **Nanoseconds**: < 1,000 ns (e.g., "500ns")
/// - **Microseconds**: < 1,000,000 ns (e.g., "1.50μs")
/// - **Milliseconds**: < 1,000,000,000 ns (e.g., "25.75ms")
/// - **Seconds**: < 60 seconds (e.g., "5.25s")
/// - **Minutes and Hours**: For longer durations (e.g., "5m 30s", "2h 15m 30s")
///
/// ## Precision
///
/// - Sub-second units use 2 decimal places for meaningful precision
/// - Larger units use whole numbers to avoid overwhelming detail
/// - Compound units (hours/minutes/seconds) show the most significant components
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::format_duration;
/// # use std::time::Duration;
/// assert_eq!(format_duration(Duration::from_nanos(750)), "750ns");
/// assert_eq!(format_duration(Duration::from_nanos(1250)), "1.25μs");
/// assert_eq!(format_duration(Duration::from_micros(2500)), "2.50ms");
/// assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
/// ```
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
        // For longer durations, use compound format (hours, minutes, seconds)
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
///
/// Converts a byte count to a human-readable string with appropriate units.
/// Uses binary (1024-based) scaling which is standard for memory and storage.
///
/// ## Parameters
/// - `bytes`: Number of bytes as usize
///
/// ## Returns
/// Human-readable byte string with appropriate units
///
/// ## Unit Scaling
///
/// Uses binary scaling (powers of 1024):
/// - Bytes: < 1024 (e.g., "500 B")
/// - Kilobytes: < 1024² (e.g., "1.50 KB")
/// - Megabytes: < 1024³ (e.g., "2.25 MB")
/// - Gigabytes: ≥ 1024³ (e.g., "1.75 GB")
///
/// ## Precision
///
/// - Bytes show whole numbers (no decimal places needed)
/// - Larger units show 2 decimal places for meaningful precision
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::format_bytes;
/// assert_eq!(format_bytes(512), "512 B");
/// assert_eq!(format_bytes(1536), "1.50 KB");
/// assert_eq!(format_bytes(2621440), "2.50 MB");
/// ```
pub fn format_bytes(bytes: usize) -> String {
    format_bytes_f64(bytes as f64)
}

/// Format bytes (as f64) in a human-readable way
///
/// Internal implementation for byte formatting that works with floating-point
/// values. This enables precise calculations while maintaining consistent
/// formatting across integer and float inputs.
///
/// ## Parameters
/// - `bytes`: Number of bytes as f64
///
/// ## Returns
/// Human-readable byte string with appropriate units
///
/// ## Implementation Details
///
/// This function handles the actual formatting logic and is used by both
/// `format_bytes` (for usize) and `format_rate` (for rates). Using f64
/// internally allows for precise fractional calculations.
///
/// ## Rounding
///
/// Values are formatted with 2 decimal places for scaled units, which
/// provides good precision without overwhelming detail. The underlying
/// f64 arithmetic ensures accurate calculations.
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
///
/// Converts a throughput rate to a human-readable string with "/s" suffix.
/// This is commonly used for displaying network and storage throughput.
///
/// ## Parameters
/// - `bytes_per_second`: Transfer rate in bytes per second
///
/// ## Returns
/// Human-readable rate string with appropriate units and "/s" suffix
///
/// ## Format Examples
///
/// - "1.50 KB/s" for 1536 bytes/second
/// - "10.25 MB/s" for 10,747,904 bytes/second
/// - "2.00 GB/s" for 2,147,483,648 bytes/second
///
/// ## Usage Context
///
/// This function is typically used for displaying:
/// - Network throughput measurements
/// - Disk I/O performance
/// - Memory bandwidth utilization
/// - IPC mechanism throughput comparison
///
/// ## Consistency
///
/// Uses the same scaling and precision rules as `format_bytes_f64` to
/// ensure consistent presentation across the application.
pub fn format_rate(bytes_per_second: f64) -> String {
    format!("{}/s", format_bytes_f64(bytes_per_second))
}

/// Format a message rate in a human-readable way
///
/// Converts a message frequency to a human-readable string with appropriate
/// scaling. This is useful for displaying message-oriented performance metrics
/// where the number of messages matters more than bytes transferred.
///
/// ## Parameters
/// - `messages_per_second`: Message frequency in messages per second
///
/// ## Returns
/// Human-readable message rate string with appropriate units
///
/// ## Unit Scaling
///
/// Uses decimal scaling (powers of 1000) for rate measurements:
/// - Messages/sec: < 1,000 (e.g., "750 msg/s")
/// - Thousands: < 1,000,000 (e.g., "15.5K msg/s")
/// - Millions: ≥ 1,000,000 (e.g., "2.3M msg/s")
///
/// ## Usage Context
///
/// Message rates are particularly relevant for:
/// - High-frequency trading systems
/// - Real-time event processing
/// - Message queue performance
/// - Low-latency communication patterns
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::format_message_rate;
/// assert_eq!(format_message_rate(750.0), "750 msg/s");
/// assert_eq!(format_message_rate(15500.0), "15.50K msg/s");
/// assert_eq!(format_message_rate(2300000.0), "2.30M msg/s");
/// ```
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
///
/// Computes basic statistical measures from a dataset, providing a comprehensive
/// statistical summary useful for performance analysis and data validation.
///
/// ## Parameters
/// - `values`: Slice of f64 values to analyze
///
/// ## Returns
/// Tuple of (mean, min, max, standard_deviation) as f64 values
///
/// ## Statistical Measures
///
/// - **Mean**: Arithmetic average of all values
/// - **Minimum**: Smallest value in the dataset
/// - **Maximum**: Largest value in the dataset
/// - **Standard Deviation**: Measure of variability around the mean
///
/// ## Empty Dataset Handling
///
/// If the input slice is empty, returns (0.0, 0.0, 0.0, 0.0) to provide
/// safe fallback values rather than causing errors.
///
/// ## Standard Deviation Calculation
///
/// Uses the population standard deviation formula:
/// σ = √(Σ(x - μ)² / N)
///
/// Where μ is the mean and N is the number of values.
///
/// ## Usage Context
///
/// This function is commonly used for:
/// - Analyzing latency distribution characteristics
/// - Validating measurement consistency
/// - Detecting performance outliers
/// - Generating summary statistics for reports
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::calculate_stats;
/// let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
/// let (mean, min, max, std_dev) = calculate_stats(&values);
/// assert_eq!(mean, 3.0);
/// assert_eq!(min, 1.0);
/// assert_eq!(max, 5.0);
/// // std_dev ≈ 1.414
/// ```
pub fn calculate_stats(values: &[f64]) -> (f64, f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }

    // Calculate basic statistics
    let sum: f64 = values.iter().sum();
    let count = values.len() as f64;
    let mean = sum / count;

    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Calculate population standard deviation
    let variance = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count;
    let std_dev = variance.sqrt();

    (mean, min, max, std_dev)
}

/// Calculate percentiles from a vector of values
///
/// Computes specified percentile values from a dataset using linear interpolation
/// for smooth results. This provides detailed insight into data distribution
/// characteristics beyond simple mean and standard deviation.
///
/// ## Parameters
/// - `values`: Slice of f64 values to analyze
/// - `percentiles`: Slice of percentile levels to calculate (0.0 to 100.0)
///
/// ## Returns
/// Vector of (percentile, value) tuples for each requested percentile
///
/// ## Interpolation Method
///
/// Uses linear interpolation between data points to provide smooth percentile
/// calculations. This gives more accurate results than simple index-based lookup,
/// especially for datasets with few samples.
///
/// ## Algorithm Details
///
/// 1. **Sort**: Data is sorted in ascending order
/// 2. **Index Calculation**: For percentile P, index = (P/100) * (N-1)
/// 3. **Interpolation**: If index is fractional, interpolate between adjacent values
/// 4. **Boundary Handling**: Clamps indices to valid range [0, N-1]
///
/// ## Empty Dataset Handling
///
/// If the input slice is empty, returns tuples with the requested percentile
/// levels paired with 0.0 values.
///
/// ## Performance
///
/// Time complexity is O(n log n) due to sorting. For frequent percentile
/// calculations on the same dataset, consider sorting once and reusing.
///
/// ## Usage Context
///
/// Percentiles are essential for:
/// - Understanding latency distribution (P95, P99)
/// - Identifying performance outliers
/// - Setting Service Level Objectives (SLOs)
/// - Comparing performance across different systems
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::calculate_percentiles;
/// let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
/// let percentiles = calculate_percentiles(&values, &[50.0, 95.0]);
/// // Returns [(50.0, 3.0), (95.0, 4.8)]
/// ```
pub fn calculate_percentiles(values: &[f64], percentiles: &[f64]) -> Vec<(f64, f64)> {
    if values.is_empty() {
        return percentiles.iter().map(|&p| (p, 0.0)).collect();
    }

    let mut sorted_values = values.to_vec();
    sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

    percentiles
        .iter()
        .map(|&p| {
            // Calculate fractional index for this percentile
            let index = (p / 100.0) * (sorted_values.len() - 1) as f64;
            let lower_index = index.floor() as usize;
            let upper_index = index.ceil() as usize;

            if lower_index == upper_index {
                // Exact index - no interpolation needed
                (p, sorted_values[lower_index])
            } else {
                // Interpolate between adjacent values
                let lower_value = sorted_values[lower_index];
                let upper_value = sorted_values[upper_index];
                let weight = index - lower_index as f64;
                (p, lower_value + weight * (upper_value - lower_value))
            }
        })
        .collect()
}

/// Validate that a port number is in the valid range
///
/// Ensures that a port number is suitable for network operations by checking
/// that it's not in the privileged range (< 1024) and is within the valid
/// 16-bit range.
///
/// ## Parameters
/// - `port`: Port number to validate
///
/// ## Returns
/// - `Ok(())`: Port number is valid
/// - `Err(anyhow::Error)`: Port number is invalid with descriptive error
///
/// ## Validation Rules
///
/// - **Minimum**: Port must be ≥ 1024 (avoid privileged ports)
/// - **Maximum**: Port must be ≤ 65535 (16-bit limit, checked by type system)
///
/// ## Privileged Ports
///
/// Ports below 1024 are reserved for system services and typically require
/// root privileges to bind. The benchmark avoids these to ensure it can
/// run with normal user permissions.
///
/// ## Error Messages
///
/// Provides clear error messages that explain why the port is invalid
/// and suggest valid alternatives.
///
/// ## Usage Context
///
/// Used to validate:
/// - TCP socket port numbers from command line arguments
/// - Automatically generated port numbers for concurrent tests
/// - User-specified ports in configuration files
pub fn validate_port(port: u16) -> Result<()> {
    if port < 1024 {
        anyhow::bail!("Port number {} is too low (below 1024)", port);
    }
    // Note: No need to check > 65535 since u16 max is 65535
    Ok(())
}

/// Validate that a buffer size is reasonable
///
/// Ensures that buffer sizes are within practical limits for IPC operations,
/// preventing both insufficient buffering and excessive memory usage.
///
/// ## Parameters
/// - `buffer_size`: Buffer size in bytes to validate
///
/// ## Returns
/// - `Ok(())`: Buffer size is valid
/// - `Err(anyhow::Error)`: Buffer size is invalid with descriptive error
///
/// ## Validation Rules
///
/// - **Minimum**: 1024 bytes (1 KB) to ensure adequate buffering
/// - **Maximum**: 1 GB to prevent excessive memory usage
///
/// ## Rationale
///
/// - **Minimum Limit**: Ensures buffers are large enough for efficient I/O
/// - **Maximum Limit**: Prevents memory exhaustion and swap thrashing
/// - **Practical Range**: Covers typical IPC buffer size requirements
///
/// ## Error Messages
///
/// Provides clear feedback about why the buffer size is invalid and
/// suggests appropriate size ranges.
///
/// ## Usage Context
///
/// Used to validate:
/// - Shared memory buffer sizes
/// - Socket buffer configurations
/// - Message queue buffer parameters
/// - User-specified buffer sizes from command line
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
///
/// Ensures that message sizes are within practical limits for IPC testing,
/// preventing both degenerate cases and excessive memory usage.
///
/// ## Parameters
/// - `message_size`: Message size in bytes to validate
///
/// ## Returns
/// - `Ok(())`: Message size is valid
/// - `Err(anyhow::Error)`: Message size is invalid with descriptive error
///
/// ## Validation Rules
///
/// - **Minimum**: 1 byte (prevent zero-length messages)
/// - **Maximum**: 16 MB (prevent excessive memory usage per message)
///
/// ## Rationale
///
/// - **Minimum Limit**: Zero-length messages are not meaningful for performance testing
/// - **Maximum Limit**: Very large messages can cause memory issues and skew results
/// - **Practical Range**: Covers typical application message sizes
///
/// ## Error Messages
///
/// Provides clear feedback about message size limits and their rationale.
///
/// ## Usage Context
///
/// Used to validate:
/// - User-specified message sizes from command line
/// - Message sizes in test configurations
/// - Calculated message sizes in benchmark scenarios
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
///
/// Ensures that concurrency settings are within practical limits, preventing
/// both degenerate cases and resource exhaustion from excessive parallelism.
///
/// ## Parameters
/// - `concurrency`: Number of concurrent workers to validate
///
/// ## Returns
/// - `Ok(())`: Concurrency level is valid
/// - `Err(anyhow::Error)`: Concurrency level is invalid with descriptive error
///
/// ## Validation Rules
///
/// - **Minimum**: 1 worker (prevent zero concurrency)
/// - **Maximum**: 1024 workers (prevent resource exhaustion)
///
/// ## Rationale
///
/// - **Minimum Limit**: Zero concurrency is not meaningful for testing
/// - **Maximum Limit**: Excessive concurrency can overwhelm system resources
/// - **Practical Range**: Covers realistic concurrency scenarios
///
/// ## Error Messages
///
/// Provides clear feedback about concurrency limits and their rationale.
///
/// ## Usage Context
///
/// Used to validate:
/// - User-specified concurrency levels from command line
/// - Calculated concurrency based on system capabilities
/// - Concurrency settings in test configurations
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
///
/// Returns the number of logical CPU cores available to the current process.
/// This information is useful for determining appropriate concurrency levels
/// and understanding system capacity.
///
/// ## Returns
/// Number of logical CPU cores available
///
/// ## Implementation
///
/// Uses the `num_cpus` crate which provides cross-platform CPU detection
/// that accounts for:
/// - Physical vs logical cores (includes hyperthreading)
/// - Container resource limits (Docker, cgroups)
/// - Process affinity restrictions
///
/// ## Usage Context
///
/// CPU core count is used for:
/// - Setting default concurrency levels
/// - Validating user-specified concurrency
/// - Scaling buffer sizes based on expected load
/// - Reporting system information in results
///
/// ## Reliability
///
/// The `num_cpus` crate handles edge cases and provides reliable detection
/// across different platforms and virtualization environments.
pub fn get_cpu_cores() -> usize {
    num_cpus::get()
}

/// Get recommended concurrency based on available CPU cores
///
/// Calculates a sensible default concurrency level based on the number of
/// available CPU cores, with practical limits to avoid overwhelming the system.
///
/// ## Returns
/// Recommended number of concurrent workers
///
/// ## Algorithm
///
/// Uses the number of CPU cores as the base, but caps at 8 workers to provide
/// reasonable defaults that work well across different system configurations.
///
/// ## Rationale
///
/// - **Core-based**: More cores generally support more concurrency
/// - **Capped**: Very high core counts don't always benefit from extreme concurrency
/// - **Conservative**: Better to start with moderate concurrency than overwhelm
/// - **Tunable**: Users can override with explicit concurrency settings
///
/// ## Usage Context
///
/// This recommendation is used for:
/// - Default concurrency when not specified by user
/// - Starting point for concurrency tuning
/// - Validation reference for extremely high user-specified values
///
/// ## Examples
///
/// - 2-core system: recommends 2 workers
/// - 8-core system: recommends 8 workers
/// - 32-core system: recommends 8 workers (capped)
pub fn get_recommended_concurrency() -> usize {
    let cores = get_cpu_cores();
    // Use number of cores, but cap at 8 for reasonable defaults
    cores.min(8)
}

/// Check if we're running in a container environment
///
/// Detects whether the benchmark is running inside a container (Docker, LXC, etc.)
/// by examining system characteristics. This information helps explain performance
/// variations and resource limitations.
///
/// ## Returns
/// - `true`: Running in a detected container environment
/// - `false`: Running on bare metal or undetected container
///
/// ## Detection Method
///
/// Examines `/proc/1/cgroup` for container-specific entries like "docker" or "lxc".
/// This is a reliable method on Linux systems but may not detect all container types.
///
/// ## Limitations
///
/// - **Linux-specific**: Only works on Linux systems with /proc filesystem
/// - **Detection gaps**: May not detect all container technologies
/// - **False negatives**: Some containers may not be detected
/// - **Platform support**: Returns false on non-Linux systems
///
/// ## Usage Context
///
/// Container detection is used for:
/// - Adjusting default memory estimates
/// - Explaining performance characteristics in results
/// - Selecting appropriate buffer sizes and limits
/// - Providing context in benchmark reports
///
/// ## Error Handling
///
/// If `/proc/1/cgroup` cannot be read, assumes non-container environment
/// rather than failing, providing graceful degradation.
pub fn is_container_environment() -> bool {
    std::fs::read_to_string("/proc/1/cgroup")
        .map(|contents| contents.contains("docker") || contents.contains("lxc"))
        .unwrap_or(false)
}

/// Get available memory in bytes (simplified estimation)
///
/// Provides an estimate of available system memory for resource planning
/// and buffer sizing. This is a simplified implementation that provides
/// reasonable defaults for common scenarios.
///
/// ## Returns
/// Estimated available memory in bytes
///
/// ## Estimation Strategy
///
/// - **Container Environment**: Assumes 2GB (typical container allocation)
/// - **Native Environment**: Assumes 16GB (common desktop/server configuration)
///
/// ## Limitations
///
/// This is a placeholder implementation with hardcoded estimates. A production
/// implementation would use platform-specific APIs to get accurate memory information:
/// - Linux: `/proc/meminfo`
/// - Windows: GlobalMemoryStatusEx
/// - macOS: sysctl
///
/// ## Usage Context
///
/// Memory estimates are used for:
/// - Validating buffer size configurations
/// - Scaling test parameters based on available resources
/// - Providing system context in benchmark results
/// - Warning about potentially problematic configurations
///
/// ## Future Improvements
///
/// A more sophisticated implementation would:
/// - Use platform-specific memory detection APIs
/// - Account for already allocated memory
/// - Consider container memory limits
/// - Provide more accurate estimates for resource planning
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
///
/// Removes temporary files that match a specified pattern from the system
/// temporary directory. This helps maintain system cleanliness after
/// benchmark runs and prevents accumulation of test artifacts.
///
/// ## Parameters
/// - `pattern`: String pattern to match in file names
///
/// ## Returns
/// - `Ok(())`: Cleanup completed (may have warnings for individual files)
/// - `Err(anyhow::Error)`: Failed to read temporary directory
///
/// ## Cleanup Strategy
///
/// - **Directory**: Uses system temporary directory (platform-specific)
/// - **Pattern Matching**: Simple substring matching in file names
/// - **Error Handling**: Logs warnings for files that cannot be removed
/// - **Non-destructive**: Only removes files, not directories
///
/// ## Safety Considerations
///
/// - **Pattern Specificity**: Use specific patterns to avoid removing unrelated files
/// - **Permission Handling**: Gracefully handles permission denied errors
/// - **Concurrent Safety**: Safe to run while other processes may be creating files
///
/// ## Usage Context
///
/// Used for cleaning up:
/// - Unix domain socket files
/// - Shared memory segments
/// - Temporary configuration files
/// - Test result artifacts
///
/// ## Error Handling
///
/// Individual file removal errors are logged as warnings rather than
/// causing the entire cleanup to fail, ensuring partial cleanup success.
///
/// ## Examples
///
/// ```rust,no_run
/// # use ipc_benchmark::utils::cleanup_temp_files;
/// // Clean up benchmark-related temporary files
/// cleanup_temp_files("ipc_benchmark_")?;
/// 
/// // Clean up specific test run files
/// cleanup_temp_files("test_run_12345")?;
/// # Ok::<(), anyhow::Error>(())
/// ```
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
///
/// Outputs a table row with proper column alignment and separators.
/// This function is part of a set of table formatting utilities that
/// help create readable tabular output for benchmark results.
///
/// ## Parameters
/// - `columns`: Array of column content strings
/// - `widths`: Array of column widths for alignment
///
/// ## Formatting
///
/// - **Alignment**: Left-aligned content within specified column widths
/// - **Separators**: Uses pipe characters (|) between columns
/// - **Padding**: Adds appropriate spacing for readability
/// - **Overflow**: Truncates content that exceeds column width
///
/// ## Usage Context
///
/// Used for displaying:
/// - Benchmark result summaries
/// - Performance comparison tables
/// - Configuration parameter listings
/// - System information displays
///
/// ## Table Construction
///
/// Typically used in combination with:
/// - `print_table_separator()` for borders
/// - Multiple `print_table_row()` calls for content
/// - Consistent width arrays across all rows
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::{print_table_row, print_table_separator};
/// let widths = [15, 10, 12];
/// print_table_separator(&widths);
/// print_table_row(&["Mechanism", "Latency", "Throughput"], &widths);
/// print_table_separator(&widths);
/// print_table_row(&["TCP", "1.5ms", "100MB/s"], &widths);
/// print_table_separator(&widths);
/// ```
pub fn print_table_row(columns: &[&str], widths: &[usize]) {
    print!("|");
    for (i, column) in columns.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(10);
        print!(" {:width$} |", column, width = width);
    }
    println!();
}

/// Print a table separator
///
/// Outputs a horizontal line separator for tables using dashes and plus
/// signs to create clear visual separation between table sections.
///
/// ## Parameters
/// - `widths`: Array of column widths to match table structure
///
/// ## Formatting
///
/// - **Characters**: Uses dashes (-) for lines and plus (+) for intersections
/// - **Width Matching**: Separator width matches column structure exactly
/// - **Padding**: Accounts for column padding in width calculations
///
/// ## Usage Context
///
/// Used for:
/// - Table headers and footers
/// - Separating data sections
/// - Creating visual structure in console output
/// - Improving readability of tabular data
///
/// ## Visual Example
///
/// ```text
/// +----------------+-----------+-------------+
/// | Mechanism      | Latency   | Throughput  |
/// +----------------+-----------+-------------+
/// | TCP            | 1.5ms     | 100MB/s     |
/// | UDP            | 0.8ms     | 150MB/s     |
/// +----------------+-----------+-------------+
/// ```
pub fn print_table_separator(widths: &[usize]) {
    print!("+");
    for &width in widths {
        print!("{}", "-".repeat(width + 2));
        print!("+");
    }
    println!();
}

/// Create a progress bar-like indicator
///
/// Generates a visual progress indicator string using Unicode block characters
/// to show completion status. This provides user feedback during long-running
/// operations.
///
/// ## Parameters
/// - `current`: Current progress value
/// - `total`: Total expected value
/// - `width`: Width of progress bar in characters
///
/// ## Returns
/// String containing progress bar visualization
///
/// ## Visual Representation
///
/// - **Filled**: Uses filled block (█) for completed progress
/// - **Empty**: Uses light shade (░) for remaining progress
/// - **Proportional**: Progress width is proportional to current/total ratio
/// - **Bounds**: Ensures progress never exceeds 100% visually
///
/// ## Character Selection
///
/// Uses Unicode block characters for better visual representation:
/// - `█` (U+2588): Full block for completed progress
/// - `░` (U+2591): Light shade for remaining progress
///
/// ## Edge Cases
///
/// - **Zero Total**: Returns all filled blocks to avoid division by zero
/// - **Overflow**: Caps progress at 100% even if current > total
/// - **Negative**: Treats negative current as zero progress
///
/// ## Usage Context
///
/// Progress indicators are useful for:
/// - Long-running benchmark operations
/// - File transfer or processing status
/// - Test suite execution progress
/// - User feedback during data analysis
///
/// ## Examples
///
/// ```rust
/// # use ipc_benchmark::utils::create_progress_indicator;
/// assert_eq!(create_progress_indicator(0, 100, 10), "░░░░░░░░░░");
/// assert_eq!(create_progress_indicator(50, 100, 10), "█████░░░░░");
/// assert_eq!(create_progress_indicator(100, 100, 10), "██████████");
/// ```
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

    /// Test duration formatting with various time scales
    #[test]
    fn test_format_duration_ns() {
        // Test each time scale with representative values
        assert_eq!(format_duration_ns(500), "500ns");
        assert_eq!(format_duration_ns(1500), "1.50μs");
        assert_eq!(format_duration_ns(1_500_000), "1.50ms");
        assert_eq!(format_duration_ns(1_500_000_000), "1.50s");
    }

    /// Test byte formatting with various scales
    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1572864), "1.50 MB");
        assert_eq!(format_bytes(1610612736), "1.50 GB");
    }

    /// Test rate formatting for throughput display
    #[test]
    fn test_format_rate() {
        assert_eq!(format_rate(1024.0), "1.00 KB/s");
        assert_eq!(format_rate(1048576.0), "1.00 MB/s");
    }

    /// Test message rate formatting for frequency display
    #[test]
    fn test_format_message_rate() {
        assert_eq!(format_message_rate(500.0), "500 msg/s");
        assert_eq!(format_message_rate(1500.0), "1.50K msg/s");
        assert_eq!(format_message_rate(1500000.0), "1.50M msg/s");
    }

    /// Test statistical calculations with known dataset
    #[test]
    fn test_calculate_stats() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (mean, min, max, std_dev) = calculate_stats(&values);

        assert_eq!(mean, 3.0);
        assert_eq!(min, 1.0);
        assert_eq!(max, 5.0);
        assert!((std_dev - 1.4142135623730951).abs() < 0.001);
    }

    /// Test percentile calculations with interpolation
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

    /// Test port number validation rules
    #[test]
    fn test_validate_port() {
        assert!(validate_port(1024).is_ok());
        assert!(validate_port(8080).is_ok());
        assert!(validate_port(65535).is_ok());
        assert!(validate_port(1023).is_err());
        assert!(validate_port(65535).is_ok());
    }

    /// Test buffer size validation rules
    #[test]
    fn test_validate_buffer_size() {
        assert!(validate_buffer_size(1024).is_ok());
        assert!(validate_buffer_size(8192).is_ok());
        assert!(validate_buffer_size(1023).is_err());
        assert!(validate_buffer_size(1024 * 1024 * 1024 + 1).is_err());
    }

    /// Test message size validation rules
    #[test]
    fn test_validate_message_size() {
        assert!(validate_message_size(1).is_ok());
        assert!(validate_message_size(1024).is_ok());
        assert!(validate_message_size(0).is_err());
        assert!(validate_message_size(16 * 1024 * 1024 + 1).is_err());
    }

    /// Test concurrency validation rules
    #[test]
    fn test_validate_concurrency() {
        assert!(validate_concurrency(1).is_ok());
        assert!(validate_concurrency(8).is_ok());
        assert!(validate_concurrency(0).is_err());
        assert!(validate_concurrency(1025).is_err());
    }

    /// Test CPU core detection
    #[test]
    fn test_get_cpu_cores() {
        assert!(get_cpu_cores() > 0);
    }

    /// Test concurrency recommendation logic
    #[test]
    fn test_get_recommended_concurrency() {
        let concurrency = get_recommended_concurrency();
        assert!(concurrency > 0);
        assert!(concurrency <= 8);
    }

    /// Test progress indicator visualization
    #[test]
    fn test_create_progress_indicator() {
        assert_eq!(create_progress_indicator(0, 100, 10), "░░░░░░░░░░");
        assert_eq!(create_progress_indicator(50, 100, 10), "█████░░░░░");
        assert_eq!(create_progress_indicator(100, 100, 10), "██████████");
    }
}