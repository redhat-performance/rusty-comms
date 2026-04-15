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

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the system-appropriate temporary directory for IPC files.
///
/// This function provides cross-platform support for determining the temporary
/// directory location, supporting Linux, macOS, and Windows.
///
/// ## Resolution Order
///
/// 1. **Environment Variable**: If `IPC_BENCHMARK_TEMP_DIR` is set, uses that value
/// 2. **System Default**: Falls back to `std::env::temp_dir()` which returns:
///    - **Linux**: `/tmp` or value of `TMPDIR` environment variable
///    - **macOS**: `/var/folders/.../T/` or value of `TMPDIR` environment variable
///    - **Windows**: `C:\Users\<user>\AppData\Local\Temp\` or value of `TEMP`/`TMP`
///
/// ## Examples
///
/// ```rust
/// use ipc_benchmark::utils::get_temp_dir;
///
/// let temp_dir = get_temp_dir();
/// let socket_path = temp_dir.join("my_socket.sock");
/// ```
///
/// ## Environment Override
///
/// ```bash
/// # Use custom temporary directory
/// IPC_BENCHMARK_TEMP_DIR=/var/run/ipc ipc-benchmark -m uds
/// ```
///
/// ## Platform Notes
///
/// - On Unix systems, socket paths have a maximum length (typically 104-108 bytes)
/// - The returned path is guaranteed to be a valid directory on the system
/// - On Windows, Unix domain sockets are only available on Windows 10 build 17063+
pub fn get_temp_dir() -> PathBuf {
    // First, check for custom temp directory via environment variable
    if let Ok(custom_dir) = std::env::var("IPC_BENCHMARK_TEMP_DIR") {
        let path = PathBuf::from(custom_dir);
        if path.is_dir() {
            return path;
        }
        // Log warning if custom dir doesn't exist but fall through to default
        tracing::warn!(
            "IPC_BENCHMARK_TEMP_DIR is set to '{}' but directory does not exist, using system default",
            path.display()
        );
    }

    // Use system-appropriate temp directory
    // This handles all platforms correctly:
    // - Linux: /tmp or TMPDIR
    // - macOS: /var/folders/.../T/ or TMPDIR
    // - Windows: %TEMP% or %TMP% or C:\Users\<user>\AppData\Local\Temp
    std::env::temp_dir()
}

/// Returns a path for a socket file in the system temporary directory.
///
/// This is a convenience function that combines `get_temp_dir()` with a
/// socket filename to create a full path suitable for Unix domain sockets.
///
/// ## Arguments
///
/// * `filename` - The name of the socket file (e.g., "ipc_benchmark.sock")
///
/// ## Returns
///
/// Full path to the socket file in the appropriate temp directory
///
/// ## Example
///
/// ```rust
/// use ipc_benchmark::utils::get_temp_socket_path;
///
/// let path = get_temp_socket_path("my_app.sock");
/// // On Linux: "/tmp/my_app.sock"
/// // On macOS: "/var/folders/.../T/my_app.sock"
/// // On Windows: "C:\\Users\\...\\AppData\\Local\\Temp\\my_app.sock"
/// ```
pub fn get_temp_socket_path(filename: &str) -> String {
    get_temp_dir().join(filename).to_string_lossy().into_owned()
}

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
                            tracing::warn!(
                                "Invalid client core ID: {} (available cores: 0-{})",
                                core_index,
                                cores.len().saturating_sub(1)
                            );
                        }
                    } else {
                        tracing::warn!("Failed to get core IDs for client affinity");
                    }
                }

                runtime.block_on(future)
            }
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

/// Sets the CPU affinity for the current thread to the specified core.
///
/// This function takes a core ID as input and attempts to pin the current
/// thread to that CPU core. This can help improve performance by reducing
/// cache misses and context switching.
///
/// ## Parameters
///
/// - `core_id`: The ID of the CPU core to pin the thread to.
///
/// ## Returns
///
/// - `Ok(())` if the affinity was set successfully.
/// - `Err(anyhow::Error)` if the specified core ID is not available or the
///   affinity could not be set.
pub fn set_affinity(core_id: usize) -> anyhow::Result<()> {
    use anyhow::Context;

    let core_ids = core_affinity::get_core_ids().context("Failed to get core IDs")?;
    tracing::info!(
        "Available cores: {} total, requesting core {}",
        core_ids.len(),
        core_id
    );

    let core = core_ids
        .get(core_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid core ID: {}", core_id))?;
    tracing::info!("Attempting to set affinity to core {:?}", core);

    if core_affinity::set_for_current(*core) {
        tracing::info!("Successfully set affinity to core {}", core_id);
        Ok(())
    } else {
        tracing::error!("Failed to set affinity to core {}", core_id);
        Err(anyhow::anyhow!(
            "Failed to set affinity for core ID: {}",
            core_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{get_temp_dir, get_temp_socket_path, spawn_with_affinity};

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

    /// Test that get_temp_dir returns a valid directory
    #[test]
    fn test_get_temp_dir_returns_valid_directory() {
        let temp_dir = get_temp_dir();
        assert!(
            temp_dir.is_dir(),
            "get_temp_dir() should return an existing directory"
        );
    }

    /// Test that get_temp_socket_path creates proper path
    #[test]
    fn test_get_temp_socket_path_creates_valid_path() {
        let socket_path = get_temp_socket_path("test_socket.sock");
        assert!(socket_path.ends_with("test_socket.sock"));
        // Should contain a path separator
        assert!(socket_path.contains(std::path::MAIN_SEPARATOR) || socket_path.contains('/'));
    }

    /// Test that custom temp dir via environment variable works
    #[test]
    fn test_custom_temp_dir_via_env() {
        // Get current temp dir first
        let system_temp = std::env::temp_dir();

        // Set custom env var to the system temp dir (we know it exists)
        std::env::set_var("IPC_BENCHMARK_TEMP_DIR", &system_temp);

        let temp_dir = get_temp_dir();
        assert_eq!(temp_dir, system_temp);

        // Clean up
        std::env::remove_var("IPC_BENCHMARK_TEMP_DIR");
    }

    /// Test spawn_with_affinity with a specific core
    #[tokio::test]
    async fn test_spawn_with_affinity_with_core() {
        // Use core 0 which should always exist
        let fut = async move { Ok::<_, anyhow::Error>(123u32) };
        let result = spawn_with_affinity(fut, Some(0)).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 123);
    }

    /// Test spawn_with_affinity with invalid core still runs
    #[tokio::test]
    async fn test_spawn_with_affinity_invalid_core() {
        // Use an impossibly high core number
        let fut = async move { Ok::<_, anyhow::Error>(456u32) };
        let result = spawn_with_affinity(fut, Some(99999)).await;
        // Should still succeed, just without affinity
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 456);
    }

    /// Test spawn_with_affinity propagates errors
    #[tokio::test]
    async fn test_spawn_with_affinity_error_propagation() {
        let fut = async move { Err::<u32, _>(anyhow::anyhow!("Test error")) };
        let result = spawn_with_affinity(fut, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Test error"));
    }

    /// Test get_temp_socket_path with various filenames
    #[test]
    fn test_get_temp_socket_path_various_names() {
        let path1 = get_temp_socket_path("simple.sock");
        assert!(path1.ends_with("simple.sock"));

        let path2 = get_temp_socket_path("with-dashes.sock");
        assert!(path2.ends_with("with-dashes.sock"));

        let path3 = get_temp_socket_path("with_underscores.sock");
        assert!(path3.ends_with("with_underscores.sock"));
    }

    /// Test that invalid env var path falls back to system temp
    #[test]
    fn test_invalid_env_var_falls_back() {
        // Set env var to non-existent path
        std::env::set_var("IPC_BENCHMARK_TEMP_DIR", "/nonexistent/path/12345");

        let temp_dir = get_temp_dir();
        // Should fall back to system temp dir
        assert!(temp_dir.is_dir());

        // Clean up
        std::env::remove_var("IPC_BENCHMARK_TEMP_DIR");
    }
}
