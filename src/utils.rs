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

use anyhow::{anyhow, Result};
use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::warn;

/// Spawns a Tokio task on a new thread with a specific CPU affinity.
///
/// If `core_id` is `Some`, this function will create a new thread, pin it to the
/// specified CPU core, and then spawn the given future on a single-threaded Tokio
/// runtime on that thread.
///
/// If `core_id` is `None`, it will spawn the future on the default Tokio runtime
/// without any specific affinity.
///
/// # Arguments
///
/// * `future` - The future to execute.
/// * `core_id` - The ID of the CPU core to pin the thread to.
///
/// # Returns
///
/// A `JoinHandle` for the spawned task.
pub async fn spawn_with_affinity<F, T>(future: F, core_id: Option<usize>) -> Result<T>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    match core_id {
        Some(id) => {
            let handle = tokio::task::spawn_blocking(move || {
                let core_ids = core_affinity::get_core_ids().ok_or_else(|| {
                    anyhow!("Failed to get core IDs, is this a supported platform?")
                })?;

                if core_ids.is_empty() {
                    return Err(anyhow!("No available CPU cores found."));
                }

                let target_core = core_ids.get(id).ok_or_else(|| {
                    anyhow!(
                        "Invalid core ID: {}. System has {} available cores (valid IDs are 0 to {}).",
                        id,
                        core_ids.len(),
                        core_ids.len() - 1
                    )
                })?;

                if !core_affinity::set_for_current(*target_core) {
                    warn!("Failed to set affinity for core ID: {}", id);
                }

                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(future)
            });
            handle.await?
        }
        None => future.await,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Test that a future can be spawned with affinity and its result retrieved.
    #[tokio::test]
    async fn test_spawn_with_affinity_retrieves_result() {
        let future = async { Ok("Hello from thread") };
        // Spawn on core 0 if available, otherwise fallback to no affinity
        let core_id = if core_affinity::get_core_ids().is_some() {
            Some(0)
        } else {
            None
        };
        let result = spawn_with_affinity(future, core_id).await.unwrap();
        assert_eq!(result, "Hello from thread");
    }

    /// Test that spawning with a specific core ID runs the future on a different thread.
    #[tokio::test]
    async fn test_spawn_with_affinity_uses_new_thread() {
        let main_thread_id = thread::current().id();
        let future = async move { Ok(thread::current().id()) };

        // Spawn on core 0 if available, otherwise fallback to no affinity.
        // This test is most meaningful when a core is available.
        let core_id = if core_affinity::get_core_ids().is_some() {
            Some(0)
        } else {
            None
        };

        if core_id.is_some() {
            let future_thread_id = spawn_with_affinity(future, core_id).await.unwrap();
            assert_ne!(
                main_thread_id, future_thread_id,
                "Future should have run on a different thread"
            );
        } else {
            // If we can't test with affinity, we can't guarantee a new thread.
            // Tokio's default scheduler might run it on the same thread.
            // We'll just log a warning that the main part of the test was skipped.
            warn!("Could not get core IDs, skipping new-thread-id check in test_spawn_with_affinity_uses_new_thread");
        }
    }

    /// Test that an invalid core ID returns a descriptive error message.
    #[tokio::test]
    async fn test_spawn_with_affinity_invalid_core_id() {
        let future = async { Ok(()) };
        // Use an impossibly high core ID
        let core_id = Some(9999);

        // This test should only run on systems where we can actually get core IDs
        if let Some(cores) = core_affinity::get_core_ids() {
            let result = spawn_with_affinity(future, core_id).await;
            assert!(result.is_err());
            let error_message = result.err().unwrap().to_string();
            let expected_message = format!(
                "Invalid core ID: 9999. System has {} available cores (valid IDs are 0 to {}).",
                cores.len(),
                cores.len() - 1
            );
            assert_eq!(error_message, expected_message);
        } else {
            warn!("Could not get core IDs, skipping invalid-core-id check in test_spawn_with_affinity_invalid_core_id");
        }
    }
}
