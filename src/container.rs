//! Container management for host-to-container IPC benchmarking.
//!
//! This module provides Podman container lifecycle management for running
//! IPC benchmark clients inside containers. All operations use `std::process::Command`
//! to invoke Podman CLI — no shell scripts.
//!
//! # Supported IPC Mechanisms
//!
//! Each mechanism requires specific container configuration:
//!
//! - **UDS**: Mount socket directory (`-v /tmp/rusty-comms:/tmp/rusty-comms`)
//! - **SHM**: Use `--ipc=host` for direct /dev/shm access
//! - **PMQ**: Mount message queue filesystem (`-v /dev/mqueue:/dev/mqueue`)
//!
//! # Example
//!
//! ```rust,no_run
//! use ipc_benchmark::container::{ContainerManager, ContainerConfig};
//! use ipc_benchmark::cli::IpcMechanism;
//!
//! # fn example() -> anyhow::Result<()> {
//! let manager = ContainerManager::new("rusty-comms", "localhost/ipc-benchmark:latest");
//!
//! // Create container configuration for UDS benchmarking
//! let mut config = ContainerConfig::for_mechanism(&IpcMechanism::UnixDomainSocket)?;
//! config.name = manager.container_name(&IpcMechanism::UnixDomainSocket);
//!
//! // Ensure container is running
//! manager.ensure_running(&config)?;
//!
//! // Run benchmark...
//!
//! // Stop container when done
//! manager.stop(&config.name)?;
//! # Ok(())
//! # }
//! ```

use anyhow::{bail, Context, Result};
use std::process::{Command, Output, Stdio};
use tracing::{debug, info, warn};

use crate::cli::IpcMechanism;

/// Directory used for Unix Domain Socket files shared between host and container.
pub const UDS_SOCKET_DIR: &str = "/tmp/rusty-comms";

/// Configuration for a benchmark container.
///
/// Holds all the settings needed to create and run a container
/// for a specific IPC mechanism.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container name (e.g., "rusty-comms-uds").
    /// Set by ContainerManager when creating the container.
    pub name: String,

    /// IPC mechanism this container is configured for.
    pub mechanism: IpcMechanism,

    /// Volume mounts required for IPC.
    /// Format: "host_path:container_path[:options]"
    pub volume_mounts: Vec<String>,

    /// Additional podman run arguments (e.g., "--ipc=host").
    pub extra_args: Vec<String>,

    /// Command arguments to pass to the container entry point.
    pub command_args: Vec<String>,
}

impl ContainerConfig {
    /// Create container configuration for a specific IPC mechanism.
    ///
    /// Sets up appropriate volume mounts and options for the mechanism.
    ///
    /// # Arguments
    ///
    /// * `mechanism` - The IPC mechanism to configure for
    ///
    /// # Returns
    ///
    /// * `Ok(ContainerConfig)` - Configuration for the mechanism
    /// * `Err` - Mechanism not supported for container mode
    ///
    /// # Example
    ///
    /// ```rust
    /// use ipc_benchmark::container::ContainerConfig;
    /// use ipc_benchmark::cli::IpcMechanism;
    ///
    /// let config = ContainerConfig::for_mechanism(&IpcMechanism::SharedMemory).unwrap();
    /// assert!(config.extra_args.iter().any(|a| a == "--ipc=host"));
    /// ```
    pub fn for_mechanism(mechanism: &IpcMechanism) -> Result<Self> {
        match mechanism {
            #[cfg(unix)]
            IpcMechanism::UnixDomainSocket => Ok(Self::uds_config()),
            IpcMechanism::SharedMemory => Ok(Self::shm_config()),
            #[cfg(target_os = "linux")]
            IpcMechanism::PosixMessageQueue => Ok(Self::pmq_config()),
            IpcMechanism::TcpSocket => Ok(Self::tcp_config()),
            IpcMechanism::All => {
                bail!("Cannot create container config for 'All'. Expand mechanisms first.")
            }
            #[allow(unreachable_patterns)]
            _ => bail!("Mechanism {:?} not supported for container mode", mechanism),
        }
    }

    /// UDS container configuration.
    ///
    /// Mounts /tmp/rusty-comms directory for socket files.
    #[cfg(unix)]
    fn uds_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::UnixDomainSocket,
            volume_mounts: vec![format!("{}:{}:rw", UDS_SOCKET_DIR, UDS_SOCKET_DIR)],
            extra_args: vec![],
            command_args: vec![],
        }
    }

    /// SHM container configuration.
    ///
    /// Uses --ipc=host to share /dev/shm with host for shared memory access.
    fn shm_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::SharedMemory,
            volume_mounts: vec![],
            extra_args: vec!["--ipc=host".to_string()],
            command_args: vec![],
        }
    }

    /// PMQ container configuration.
    ///
    /// Mounts /dev/mqueue for POSIX message queues.
    /// May require --privileged for mqueue access on some systems.
    #[cfg(target_os = "linux")]
    fn pmq_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::PosixMessageQueue,
            volume_mounts: vec!["/dev/mqueue:/dev/mqueue:rw".to_string()],
            extra_args: vec!["--privileged".to_string()],
            command_args: vec![],
        }
    }

    /// TCP container configuration.
    ///
    /// Uses host network for simplicity (container shares host's network stack).
    fn tcp_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::TcpSocket,
            volume_mounts: vec![],
            extra_args: vec!["--network=host".to_string()],
            command_args: vec![],
        }
    }
}

/// Manages Podman container lifecycle for IPC benchmarking.
///
/// Provides methods to create, start, stop, and query containers
/// using Podman CLI via `std::process::Command`. No shell scripts are used.
///
/// # Example
///
/// ```rust,no_run
/// use ipc_benchmark::container::ContainerManager;
/// use ipc_benchmark::cli::IpcMechanism;
///
/// let manager = ContainerManager::new("rusty-comms", "localhost/ipc-benchmark:latest");
/// let name = manager.container_name(&IpcMechanism::SharedMemory);
/// assert!(name.contains("sharedmemory"));
/// ```
pub struct ContainerManager {
    /// Prefix for container names (e.g., "rusty-comms").
    prefix: String,

    /// Container image to use (e.g., "localhost/ipc-benchmark:latest").
    image: String,
}

impl ContainerManager {
    /// Create a new container manager.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Prefix for container names (e.g., "rusty-comms")
    /// * `image` - Container image to use (e.g., "localhost/ipc-benchmark:latest")
    ///
    /// # Example
    ///
    /// ```rust
    /// use ipc_benchmark::container::ContainerManager;
    ///
    /// let manager = ContainerManager::new("my-bench", "myimage:v1");
    /// ```
    pub fn new(prefix: &str, image: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            image: image.to_string(),
        }
    }

    /// Get the full container name for a mechanism.
    ///
    /// Container names follow the pattern: `<prefix>-<mechanism>`.
    ///
    /// # Arguments
    ///
    /// * `mechanism` - The IPC mechanism
    ///
    /// # Returns
    ///
    /// Container name string (e.g., "rusty-comms-sharedmemory")
    pub fn container_name(&self, mechanism: &IpcMechanism) -> String {
        let mech_name = match mechanism {
            #[cfg(unix)]
            IpcMechanism::UnixDomainSocket => "uds",
            IpcMechanism::SharedMemory => "shm",
            IpcMechanism::TcpSocket => "tcp",
            #[cfg(target_os = "linux")]
            IpcMechanism::PosixMessageQueue => "pmq",
            IpcMechanism::All => "all",
            #[allow(unreachable_patterns)]
            _ => "unknown",
        };
        format!("{}-{}", self.prefix, mech_name)
    }

    /// Check if a container exists (running or stopped).
    ///
    /// # Arguments
    ///
    /// * `name` - Container name to check
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Container exists
    /// * `Ok(false)` - Container does not exist
    /// * `Err` - Failed to query container status
    pub fn exists(&self, name: &str) -> Result<bool> {
        let output = Command::new("podman")
            .args(["container", "exists", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run 'podman container exists'")?;

        Ok(output.success())
    }

    /// Check if a container is currently running.
    ///
    /// # Arguments
    ///
    /// * `name` - Container name to check
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Container is running
    /// * `Ok(false)` - Container is not running or does not exist
    /// * `Err` - Failed to inspect container
    pub fn is_running(&self, name: &str) -> Result<bool> {
        let output = Command::new("podman")
            .args([
                "container",
                "inspect",
                name,
                "--format",
                "{{.State.Running}}",
            ])
            .output()
            .context("Failed to run 'podman container inspect'")?;

        if !output.status.success() {
            return Ok(false);
        }

        let running = String::from_utf8_lossy(&output.stdout);
        Ok(running.trim() == "true")
    }

    /// Ensure container is created and running.
    ///
    /// Creates the container if it doesn't exist, starts it if not running.
    /// This is the primary method for preparing a container for benchmarking.
    ///
    /// # Arguments
    ///
    /// * `config` - Container configuration with name, mounts, and options
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Container is running and ready
    /// * `Err` - Failed to create or start container
    pub fn ensure_running(&self, config: &ContainerConfig) -> Result<()> {
        let name = if config.name.is_empty() {
            self.container_name(&config.mechanism)
        } else {
            config.name.clone()
        };

        if !self.exists(&name)? {
            info!("Creating container: {}", name);
            self.create(&name, config)?;
        }

        if !self.is_running(&name)? {
            info!("Starting container: {}", name);
            self.start(&name)?;
        } else {
            debug!("Container already running: {}", name);
        }

        Ok(())
    }

    /// Create a new container (does not start it).
    ///
    /// The container is created with the specified configuration but left
    /// in stopped state. Use `start()` to run it.
    ///
    /// # Arguments
    ///
    /// * `name` - Container name
    /// * `config` - Container configuration
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Container created successfully
    /// * `Err` - Failed to create container
    fn create(&self, name: &str, config: &ContainerConfig) -> Result<()> {
        let mut cmd = Command::new("podman");
        cmd.args(["create", "--name", name]);

        // Add volume mounts
        for mount in &config.volume_mounts {
            cmd.args(["-v", mount]);
        }

        // Add extra args (e.g., --ipc=host, --privileged)
        for arg in &config.extra_args {
            cmd.arg(arg);
        }

        // Add image
        cmd.arg(&self.image);

        // Add command args
        for arg in &config.command_args {
            cmd.arg(arg);
        }

        debug!("Running: podman {:?}", cmd.get_args().collect::<Vec<_>>());

        let output = cmd.output().context("Failed to run 'podman create'")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create container '{}': {}", name, stderr.trim());
        }

        info!("Container '{}' created successfully", name);
        Ok(())
    }

    /// Start an existing container.
    ///
    /// # Arguments
    ///
    /// * `name` - Container name to start
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Container started
    /// * `Err` - Failed to start container
    pub fn start(&self, name: &str) -> Result<()> {
        let output = Command::new("podman")
            .args(["start", name])
            .output()
            .context("Failed to run 'podman start'")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to start container '{}': {}", name, stderr.trim());
        }

        info!("Container '{}' started", name);
        Ok(())
    }

    /// Stop a running container.
    ///
    /// Stops the container with a 5 second timeout. If the container
    /// does not exist, this is a no-op.
    ///
    /// # Arguments
    ///
    /// * `name` - Container name to stop
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Container stopped or did not exist
    /// * `Err` - Failed to stop container
    pub fn stop(&self, name: &str) -> Result<()> {
        if !self.exists(name)? {
            debug!("Container '{}' does not exist, nothing to stop", name);
            return Ok(());
        }

        if !self.is_running(name)? {
            debug!("Container '{}' is not running", name);
            return Ok(());
        }

        info!("Stopping container: {}", name);

        let output = Command::new("podman")
            .args(["stop", "-t", "5", name])
            .output()
            .context("Failed to run 'podman stop'")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to stop container '{}': {}", name, stderr.trim());
        } else {
            info!("Container '{}' stopped", name);
        }

        Ok(())
    }

    /// Remove a container (stops it first if running).
    ///
    /// # Arguments
    ///
    /// * `name` - Container name to remove
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Container removed or did not exist
    /// * `Err` - Failed to remove container
    pub fn remove(&self, name: &str) -> Result<()> {
        if !self.exists(name)? {
            debug!("Container '{}' does not exist, nothing to remove", name);
            return Ok(());
        }

        // Stop first if running
        if self.is_running(name)? {
            self.stop(name)?;
        }

        info!("Removing container: {}", name);

        let output = Command::new("podman")
            .args(["rm", "-f", name])
            .output()
            .context("Failed to run 'podman rm'")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to remove container '{}': {}", name, stderr.trim());
        } else {
            info!("Container '{}' removed", name);
        }

        Ok(())
    }

    /// Execute a command inside a running container.
    ///
    /// # Arguments
    ///
    /// * `name` - Container name
    /// * `command` - Command and arguments to execute
    ///
    /// # Returns
    ///
    /// * `Ok(Output)` - Command output (may have failed, check status)
    /// * `Err` - Failed to exec command
    pub fn exec(&self, name: &str, command: &[&str]) -> Result<Output> {
        let mut cmd = Command::new("podman");
        cmd.args(["exec", name]);
        cmd.args(command);

        debug!("Executing in container '{}': {:?}", name, command);

        cmd.output()
            .with_context(|| format!("Failed to exec in container '{}'", name))
    }

    /// Run a command in a new container instance (one-shot).
    ///
    /// Creates a temporary container, runs the command, and removes
    /// the container when done.
    ///
    /// # Arguments
    ///
    /// * `config` - Container configuration
    /// * `command` - Command and arguments to run
    ///
    /// # Returns
    ///
    /// * `Ok(Output)` - Command output
    /// * `Err` - Failed to run container
    pub fn run_oneshot(&self, config: &ContainerConfig, command: &[&str]) -> Result<Output> {
        let mut cmd = Command::new("podman");
        cmd.args(["run", "--rm"]);

        // Add volume mounts
        for mount in &config.volume_mounts {
            cmd.args(["-v", mount]);
        }

        // Add extra args
        for arg in &config.extra_args {
            cmd.arg(arg);
        }

        cmd.arg(&self.image);
        cmd.args(command);

        debug!(
            "Running one-shot: podman {:?}",
            cmd.get_args().collect::<Vec<_>>()
        );

        cmd.output().context("Failed to run 'podman run'")
    }

    /// Stop all benchmark containers managed by this manager.
    ///
    /// Iterates through all supported mechanisms and stops their containers.
    pub fn stop_all(&self) -> Result<()> {
        info!(
            "Stopping all benchmark containers with prefix '{}'",
            self.prefix
        );

        let mechanisms = Self::all_mechanisms();
        for mechanism in &mechanisms {
            let name = self.container_name(mechanism);
            if self.exists(&name)? {
                self.stop(&name)?;
            }
        }

        Ok(())
    }

    /// Remove all benchmark containers managed by this manager.
    ///
    /// Iterates through all supported mechanisms and removes their containers.
    pub fn remove_all(&self) -> Result<()> {
        info!(
            "Removing all benchmark containers with prefix '{}'",
            self.prefix
        );

        let mechanisms = Self::all_mechanisms();
        for mechanism in &mechanisms {
            let name = self.container_name(mechanism);
            self.remove(&name)?;
        }

        Ok(())
    }

    /// Get list of all supported mechanisms for container mode.
    fn all_mechanisms() -> Vec<IpcMechanism> {
        let mut mechanisms = vec![IpcMechanism::SharedMemory, IpcMechanism::TcpSocket];

        #[cfg(unix)]
        mechanisms.push(IpcMechanism::UnixDomainSocket);

        #[cfg(target_os = "linux")]
        mechanisms.push(IpcMechanism::PosixMessageQueue);

        mechanisms
    }

    /// Ensure the socket directory exists on the host.
    ///
    /// Creates /tmp/rusty-comms if it doesn't exist. Required for UDS.
    pub fn ensure_socket_dir() -> Result<()> {
        let path = std::path::Path::new(UDS_SOCKET_DIR);
        if !path.exists() {
            std::fs::create_dir_all(path).with_context(|| {
                format!("Failed to create socket directory: {}", UDS_SOCKET_DIR)
            })?;
            info!("Created socket directory: {}", UDS_SOCKET_DIR);
        }
        Ok(())
    }

    /// Check if Podman is available on the system.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Podman is installed and accessible
    /// * `Ok(false)` - Podman not found
    /// * `Err` - Error checking for Podman
    pub fn is_podman_available() -> Result<bool> {
        let output = Command::new("podman")
            .args(["--version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match output {
            Ok(status) => Ok(status.success()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("Failed to check for Podman"),
        }
    }

    /// Check if the container image exists locally.
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Image exists locally
    /// * `Ok(false)` - Image not found
    /// * `Err` - Error checking for image
    pub fn image_exists(&self) -> Result<bool> {
        let output = Command::new("podman")
            .args(["image", "exists", &self.image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to run 'podman image exists'")?;

        Ok(output.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_name_generation() {
        let manager = ContainerManager::new("test-prefix", "test:latest");

        #[cfg(unix)]
        assert_eq!(
            manager.container_name(&IpcMechanism::UnixDomainSocket),
            "test-prefix-uds"
        );

        assert_eq!(
            manager.container_name(&IpcMechanism::SharedMemory),
            "test-prefix-shm"
        );

        assert_eq!(
            manager.container_name(&IpcMechanism::TcpSocket),
            "test-prefix-tcp"
        );

        #[cfg(target_os = "linux")]
        assert_eq!(
            manager.container_name(&IpcMechanism::PosixMessageQueue),
            "test-prefix-pmq"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_container_config_uds() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::UnixDomainSocket).unwrap();
        assert_eq!(config.mechanism, IpcMechanism::UnixDomainSocket);
        assert!(config
            .volume_mounts
            .iter()
            .any(|m| m.contains("rusty-comms")));
    }

    #[test]
    fn test_container_config_shm() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::SharedMemory).unwrap();
        assert_eq!(config.mechanism, IpcMechanism::SharedMemory);
        assert!(config.extra_args.iter().any(|a| a == "--ipc=host"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_container_config_pmq() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::PosixMessageQueue).unwrap();
        assert_eq!(config.mechanism, IpcMechanism::PosixMessageQueue);
        assert!(config.volume_mounts.iter().any(|m| m.contains("mqueue")));
        assert!(config.extra_args.iter().any(|a| a == "--privileged"));
    }

    #[test]
    fn test_container_config_tcp() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::TcpSocket).unwrap();
        assert_eq!(config.mechanism, IpcMechanism::TcpSocket);
        assert!(config.extra_args.iter().any(|a| a == "--network=host"));
    }

    #[test]
    fn test_container_config_all_fails() {
        let result = ContainerConfig::for_mechanism(&IpcMechanism::All);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("All"));
    }

    #[test]
    fn test_manager_new() {
        let manager = ContainerManager::new("my-prefix", "my-image:v1");
        assert_eq!(manager.prefix, "my-prefix");
        assert_eq!(manager.image, "my-image:v1");
    }

    #[test]
    fn test_all_mechanisms_includes_expected() {
        let mechanisms = ContainerManager::all_mechanisms();

        // SHM and TCP should always be present
        assert!(mechanisms.contains(&IpcMechanism::SharedMemory));
        assert!(mechanisms.contains(&IpcMechanism::TcpSocket));

        // UDS on Unix
        #[cfg(unix)]
        assert!(mechanisms.contains(&IpcMechanism::UnixDomainSocket));

        // PMQ on Linux
        #[cfg(target_os = "linux")]
        assert!(mechanisms.contains(&IpcMechanism::PosixMessageQueue));
    }
}
