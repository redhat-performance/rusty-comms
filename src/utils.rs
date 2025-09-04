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
//!
//! // Validate configuration parameters
//! # fn main() -> anyhow::Result<()> {
//! validate_message_size(1024)?; // OK
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}