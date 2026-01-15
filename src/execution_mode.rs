//! Execution mode configuration for IPC benchmark
//!
//! This module defines the execution model used for IPC operations.
//! The benchmark supports two distinct execution modes:
//!
//! - **Async Mode**: Uses Tokio runtime with async/await for
//!   non-blocking I/O
//! - **Blocking Mode**: Uses standard library with blocking I/O
//!   operations
//!
//! Both modes use the same measurement methodology to ensure fair
//! performance comparison. The async mode is the default to maintain
//! backward compatibility.
//!
//! # Examples
//!
//! ```rust
//! use ipc_benchmark::execution_mode::ExecutionMode;
//!
//! // Determine mode from CLI flag
//! let mode = ExecutionMode::from_blocking_flag(false);
//! assert_eq!(mode, ExecutionMode::Async);
//!
//! let mode = ExecutionMode::from_blocking_flag(true);
//! assert_eq!(mode, ExecutionMode::Blocking);
//! ```

use std::fmt;

/// Defines the execution model for IPC operations.
///
/// This enum controls whether the benchmark uses asynchronous or
/// synchronous I/O operations. The choice affects the entire execution
/// path, from transport initialization through measurement collection to
/// result output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionMode {
    /// Asynchronous I/O using Tokio runtime.
    ///
    /// This mode uses async/await syntax with Tokio for non-blocking I/O.
    /// It's the default mode and provides the best performance for
    /// high-concurrency scenarios.
    Async,

    /// Synchronous blocking I/O using standard library.
    ///
    /// This mode uses traditional blocking I/O operations from std::net,
    /// std::fs, and std::os::unix::net. It provides a baseline for
    /// comparing async overhead and is useful for understanding pure
    /// blocking performance characteristics.
    Blocking,
}

impl ExecutionMode {
    /// Create ExecutionMode from the CLI blocking flag.
    ///
    /// # Arguments
    ///
    /// * `blocking` - If true, returns Blocking mode; if false, returns
    ///   Async mode
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::execution_mode::ExecutionMode;
    ///
    /// let async_mode = ExecutionMode::from_blocking_flag(false);
    /// assert_eq!(async_mode, ExecutionMode::Async);
    ///
    /// let blocking_mode = ExecutionMode::from_blocking_flag(true);
    /// assert_eq!(blocking_mode, ExecutionMode::Blocking);
    /// ```
    pub fn from_blocking_flag(blocking: bool) -> Self {
        if blocking {
            Self::Blocking
        } else {
            Self::Async
        }
    }

    /// Returns true if this is Async mode.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::execution_mode::ExecutionMode;
    ///
    /// assert!(ExecutionMode::Async.is_async());
    /// assert!(!ExecutionMode::Blocking.is_async());
    /// ```
    pub fn is_async(&self) -> bool {
        matches!(self, Self::Async)
    }

    /// Returns true if this is Blocking mode.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ipc_benchmark::execution_mode::ExecutionMode;
    ///
    /// assert!(ExecutionMode::Blocking.is_blocking());
    /// assert!(!ExecutionMode::Async.is_blocking());
    /// ```
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::Blocking)
    }
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Async => write!(f, "Async"),
            Self::Blocking => write!(f, "Blocking"),
        }
    }
}

impl Default for ExecutionMode {
    /// Default execution mode is Async for backward compatibility.
    fn default() -> Self {
        Self::Async
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_blocking_flag_false_gives_async() {
        let mode = ExecutionMode::from_blocking_flag(false);
        assert_eq!(mode, ExecutionMode::Async);
    }

    #[test]
    fn test_from_blocking_flag_true_gives_blocking() {
        let mode = ExecutionMode::from_blocking_flag(true);
        assert_eq!(mode, ExecutionMode::Blocking);
    }

    #[test]
    fn test_is_async() {
        assert!(ExecutionMode::Async.is_async());
        assert!(!ExecutionMode::Blocking.is_async());
    }

    #[test]
    fn test_is_blocking() {
        assert!(ExecutionMode::Blocking.is_blocking());
        assert!(!ExecutionMode::Async.is_blocking());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", ExecutionMode::Async), "Async");
        assert_eq!(format!("{}", ExecutionMode::Blocking), "Blocking");
    }

    #[test]
    fn test_default_is_async() {
        let mode = ExecutionMode::default();
        assert_eq!(mode, ExecutionMode::Async);
    }

    #[test]
    fn test_debug_format() {
        let async_mode = ExecutionMode::Async;
        let blocking_mode = ExecutionMode::Blocking;
        assert!(format!("{:?}", async_mode).contains("Async"));
        assert!(format!("{:?}", blocking_mode).contains("Blocking"));
    }
}
