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

use std::time::{SystemTime, UNIX_EPOCH};

/// Spawn a future on a dedicated thread, optionally setting CPU affinity
/// before running it. Returns the future's output.
///
/// If `core_id` is `Some(n)`, the spawned thread will attempt to pin itself
/// to CPU core `n` using the `core_affinity` crate. If affinity cannot be set,
/// the future will still run normally.
pub async fn spawn_with_affinity<F, T>(future: F, core_id: Option<usize>) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    // Use a dedicated OS thread instead of Tokio's thread pool to maintain CPU affinity
    let (sender, receiver) = tokio::sync::oneshot::channel();
    
    std::thread::spawn(move || {
        // Run the async future on this dedicated thread using a local runtime
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build runtime: {}", e));
            
        let result = match rt {
            Ok(runtime) => {
                // Set CPU affinity AFTER creating the runtime but BEFORE running the future
                // This ensures the runtime and all its threads inherit the affinity
                if let Some(core_index) = core_id {
                    if let Some(cores) = core_affinity::get_core_ids() {
                        // Fix: Use core_index as array index, not as core ID to match
                        // This makes client affinity consistent with server affinity behavior
                        if let Some(core) = cores.get(core_index) {
                            if core_affinity::set_for_current(*core) {
                                tracing::info!("Successfully set client affinity to CPU core {} after runtime creation", core_index);
                            } else {
                                tracing::warn!("Failed to set client affinity to CPU core {} after runtime creation", core_index);
                            }
                        } else {
                            tracing::warn!("Invalid client core ID: {} (available cores: 0-{})", 
                                         core_index, cores.len().saturating_sub(1));
                        }
                    } else {
                        tracing::warn!("Failed to get core IDs for client affinity");
                    }
                }
                
                runtime.block_on(future)
            },
            Err(e) => Err(e),
        };
        
        // Send the result back to the main async context
        let _ = sender.send(result);
    });
    
    // Wait for the dedicated thread to complete
    receiver
        .await
        .map_err(|e| anyhow::anyhow!("Thread communication error: {}", e))?
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
    use super::spawn_with_affinity;

    /// Smoke test for spawn_with_affinity: ensures the future runs and returns a value.
    #[tokio::test]
    async fn test_spawn_with_affinity_smoke() {
        let fut = async move {
            // simple computation
            Ok::<_, anyhow::Error>(42u32)
        };
        let result: u32 = spawn_with_affinity(fut, None).await.unwrap();
        assert_eq!(result, 42);
    }
}
