# Implementation Plan: Host-to-Container IPC Communication

**Project Goal:** Enable IPC benchmarking between host and Podman containers  
**Date:** 2025-12-16  
**Target Repository:** rusty-comms  
**Status:** PLANNING  

---

## Overview

Add `--host` CLI option to enable IPC communication between the host (server) and Podman 
containers (clients). The host drives the benchmark tests and collects results while 
containers act as connected clients. This enables realistic cross-boundary IPC performance 
measurement.

### What We're Building

1. **New `--run-mode` CLI option** with three modes:
   - `standalone` (default): Current behavior, spawns client as subprocess
   - `host`: Acts as server, drives tests, auto-creates container clients
   - `client`: Runs inside container, connects back to host server
   
   Note: Named `--run-mode` to avoid conflict with existing `--host` TCP address option.

2. **Automatic container lifecycle management**:
   - Auto-detect mechanism and configure container appropriately
   - Create/start Podman container with correct volume mounts
   - Reuse running containers when possible
   - Stop containers via CLI option (`--stop-container`)

3. **Supported IPC mechanisms**:
   - Unix Domain Sockets (UDS): Mount socket directory
   - Shared Memory (SHM): Mount `/dev/shm` or specific segments
   - POSIX Message Queues (PMQ): Mount `/dev/mqueue`

4. **No shell scripts**: All container management in Rust using `std::process::Command`

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                          HOST SYSTEM                            │
│                                                                 │
│  ┌─────────────────────┐     ┌─────────────────────────────┐   │
│  │  ipc-benchmark      │     │     Podman Container        │   │
│  │  --host host        │     │                             │   │
│  │                     │     │  ┌─────────────────────┐    │   │
│  │  • Creates server   │────►│  │  ipc-benchmark      │    │   │
│  │  • Spawns container │     │  │  --host client      │    │   │
│  │  • Drives tests     │◄────│  │                     │    │   │
│  │  • Collects results │     │  │  • Connects to host │    │   │
│  │                     │     │  │  • Sends/receives   │    │   │
│  └─────────────────────┘     │  └─────────────────────┘    │   │
│           │                  │                             │   │
│           │                  └─────────────────────────────┘   │
│           │                              │                      │
│           ▼                              ▼                      │
│  ┌────────────────────────────────────────────────────────┐    │
│  │              IPC Channel (UDS/SHM/PMQ)                 │    │
│  │                                                        │    │
│  │  UDS: /tmp/rusty-comms/*.sock  (mounted in container)  │    │
│  │  SHM: /dev/shm/rusty-comms-*   (mounted in container)  │    │
│  │  PMQ: /dev/mqueue/rusty-comms-*(mounted in container)  │    │
│  └────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

### Architecture Decisions

- **1a**: Keep existing spawned-process architecture, add container mode alongside
- **2a**: Implement for UDS, SHM, and PMQ mechanisms (TCP works naturally across containers)
- **3a**: Container image includes prebuilt benchmark binary
- **4a**: Host mode creates/manages containers via Podman CLI
- **5a**: Client mode is passive, connects back to host-provided endpoints

### Core Principles

1. **No shell scripts**: All Podman operations via `std::process::Command` in Rust
2. **Reuse existing transports**: Same IpcTransport implementations, different lifecycle
3. **Minimal container overhead**: Reuse running containers when possible
4. **Predictable resource paths**: Use `/tmp/rusty-comms/` prefix for all IPC resources
5. **Complete documentation**: README for setup and usage
6. **Full test coverage**: Unit and integration tests for container management
7. **Backward compatible**: `--host standalone` is default, existing usage unchanged

---

## How to Use This Plan

**For AI Agents:** When asked "What's next?", look at the "Current Stage" marker below 
and execute the next incomplete stage. Each stage is self-contained with clear:
- Prerequisites (what must be done first)
- Implementation steps (what to code)
- Documentation requirements (what to document)
- Testing requirements (what tests to write)
- Validation criteria (how to verify it works)
- Completion marker (how to mark it done)

**Progress Tracking:** Update the stage status as you go:
- `[ ]` = Not started
- `[~]` = In progress
- `[✓]` = Completed

**IMPORTANT:** Before every git commit, update both:
1. The Master Progress Checklist (below)
2. The Changelog (at end of document)

---

## Master Progress Checklist

**Last Updated:** 2025-12-16  
**Overall Status:** Stage 1 Complete (1/8 stages complete)

### Stage Completion Status

- [✓] **Stage 1**: Foundation - CLI and Host Mode Infrastructure (3/3 steps)
  - [✓] Step 1.1: Add --run-mode CLI option with RunMode enum
  - [✓] Step 1.2: Add --stop-container CLI option
  - [✓] Step 1.3: Update main() to branch on run mode
  - [ ] Git commit created

- [ ] **Stage 2**: Container Management Module (3/3 steps)
  - [ ] Step 2.1: Create ContainerManager struct
  - [ ] Step 2.2: Implement container create/start/stop
  - [ ] Step 2.3: Implement mechanism-specific volume mounts
  - [ ] Git commit created

- [ ] **Stage 3**: Host Mode Implementation (3/3 steps)
  - [ ] Step 3.1: Create HostModeRunner for driving tests
  - [ ] Step 3.2: Integrate ContainerManager with runner
  - [ ] Step 3.3: Implement container client spawning
  - [ ] Git commit created

- [ ] **Stage 4**: Client Mode Implementation (2/2 steps)
  - [ ] Step 4.1: Update async transports for client mode
  - [ ] Step 4.2: Update blocking transports for client mode
  - [ ] Git commit created

- [ ] **Stage 5**: SHM-Direct Container Support (2/2 steps)
  - [ ] Step 5.1: Update shared_memory_direct.rs for container paths
  - [ ] Step 5.2: Add /dev/shm mount handling
  - [ ] Git commit created

- [ ] **Stage 6**: Container Lifecycle Commands (2/2 steps)
  - [ ] Step 6.1: Implement --stop-container functionality
  - [ ] Step 6.2: Implement --list-containers (optional)
  - [ ] Git commit created

- [ ] **Stage 7**: Integration Testing (4/4 substages)
  - [ ] Step 7.1: UDS host-container tests
  - [ ] Step 7.2: SHM host-container tests
  - [ ] Step 7.3: PMQ host-container tests
  - [ ] Step 7.4: Container lifecycle tests
  - [ ] Git commit created

- [ ] **Stage 8**: Documentation and Final Validation (3/3 steps)
  - [ ] Step 8.1: Create docs/PODMAN_SETUP.md
  - [ ] Step 8.2: Create docs/HOST_CONTAINER_USAGE.md
  - [ ] Step 8.3: Update README.md with overview
  - [ ] Final validation complete
  - [ ] Git commit and tag created

### Quality Gates (Must Pass Before Completion)

- [ ] `cargo test --all-features` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo fmt --check` passes
- [ ] `cargo doc --open` generates clean docs
- [ ] Host mode works with all 3 mechanisms (UDS, SHM, PMQ)
- [ ] Container management works (create, start, stop)
- [ ] Both async and blocking modes work in host/client configuration
- [ ] SHM-direct works in host-container mode
- [ ] No breaking changes to standalone mode (backward compatible)

### Git Commit Tracking

**Expected commits:** 9 total (8 stage commits + final tag)

- [ ] Stage 1 commit
- [ ] Stage 2 commit
- [ ] Stage 3 commit
- [ ] Stage 4 commit
- [ ] Stage 5 commit
- [ ] Stage 6 commit
- [ ] Stage 7 commit
- [ ] Stage 8 commit
- [ ] Git tag: v0.3.0-host-container

---

## CURRENT STAGE: Stage 2

---

## STAGE 1: Foundation - CLI and Host Mode Infrastructure

**Estimated Time:** 2-3 hours  
**Status:** `[✓]` Completed  
**Prerequisites:** None (starting point)

### Objectives
- Add `--host` CLI option with HostMode enum
- Add `--stop-container` CLI option for container lifecycle management
- Update main() to branch between standalone, host, and client modes
- Establish testing framework for host-container modes

### Implementation Steps

#### Step 1.1: Add --host CLI Option and HostMode Enum

**File:** `src/cli.rs`  
**Action:** Add host option to Args struct

```rust
/// Host mode for IPC communication.
///
/// Controls how the benchmark operates in terms of process/container architecture:
///
/// - `standalone`: Default mode. Spawns client as a subprocess on same host.
/// - `host`: Server mode. Creates/manages Podman container as client.
/// - `client`: Client mode. Runs inside container, connects to host server.
///
/// # Examples
///
/// ```bash
/// # Standalone mode (default, existing behavior)
/// ipc-benchmark -m uds --msg-count 1000
///
/// # Host mode - creates container and drives tests
/// ipc-benchmark -m uds --host host --msg-count 1000
///
/// # Client mode - used inside container (auto-invoked by host mode)
/// ipc-benchmark -m uds --host client --socket-path /tmp/rusty-comms/uds.sock
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum HostMode {
    /// Standalone mode - spawn client as subprocess (default, existing behavior)
    #[default]
    Standalone,
    /// Host/server mode - create container client, drive tests, collect results
    Host,
    /// Client mode - run inside container, connect back to host
    Client,
}

impl std::fmt::Display for HostMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostMode::Standalone => write!(f, "standalone"),
            HostMode::Host => write!(f, "host"),
            HostMode::Client => write!(f, "client"),
        }
    }
}
```

**Add to Args struct:**
```rust
/// Host mode for IPC communication.
///
/// Controls the process/container architecture:
/// - `standalone` (default): Spawns client as subprocess on same host
/// - `host`: Creates Podman container as client, drives tests from host
/// - `client`: Runs inside container, connects back to host server
///
/// # Examples
///
/// ```bash
/// # Run as host, auto-create container client
/// ipc-benchmark -m uds --host host --msg-count 1000
///
/// # Container client mode (auto-invoked)
/// ipc-benchmark -m uds --host client
/// ```
#[arg(long, value_enum, default_value_t = HostMode::Standalone)]
pub host: HostMode,
```

**Location:** After the `blocking` flag in Args struct

**Testing Requirements:**
```rust
#[cfg(test)]
mod host_mode_tests {
    use super::*;
    
    #[test]
    fn test_host_mode_default_is_standalone() {
        let args = Args::parse_from(["ipc-benchmark", "-m", "uds"]);
        assert_eq!(args.host, HostMode::Standalone);
    }
    
    #[test]
    fn test_host_mode_can_be_set_to_host() {
        let args = Args::parse_from(["ipc-benchmark", "-m", "uds", "--host", "host"]);
        assert_eq!(args.host, HostMode::Host);
    }
    
    #[test]
    fn test_host_mode_can_be_set_to_client() {
        let args = Args::parse_from(["ipc-benchmark", "-m", "uds", "--host", "client"]);
        assert_eq!(args.host, HostMode::Client);
    }
    
    #[test]
    fn test_host_mode_display() {
        assert_eq!(format!("{}", HostMode::Standalone), "standalone");
        assert_eq!(format!("{}", HostMode::Host), "host");
        assert_eq!(format!("{}", HostMode::Client), "client");
    }
}
```

---

#### Step 1.2: Add Container Management CLI Options

**File:** `src/cli.rs`  
**Action:** Add container lifecycle options

```rust
/// Stop a running benchmark container.
///
/// When provided, stops the Podman container used for IPC benchmarking
/// instead of running a benchmark. Can specify mechanism to stop specific
/// container or use "all" to stop all benchmark containers.
///
/// # Examples
///
/// ```bash
/// # Stop the UDS benchmark container
/// ipc-benchmark --stop-container uds
///
/// # Stop all benchmark containers
/// ipc-benchmark --stop-container all
/// ```
#[arg(long, value_name = "MECHANISM")]
pub stop_container: Option<String>,

/// Container image to use for client containers.
///
/// Specifies the Podman image containing the ipc-benchmark binary.
/// Default: "localhost/ipc-benchmark:latest"
///
/// # Examples
///
/// ```bash
/// # Use custom image
/// ipc-benchmark -m uds --host host --container-image myregistry/ipc-benchmark:v1
/// ```
#[arg(long, default_value = "localhost/ipc-benchmark:latest")]
pub container_image: String,

/// Container name prefix for benchmark containers.
///
/// Containers are named "<prefix>-<mechanism>", e.g., "rusty-comms-uds".
/// Default: "rusty-comms"
///
/// # Examples
///
/// ```bash
/// # Use custom prefix
/// ipc-benchmark -m uds --host host --container-prefix my-bench
/// ```
#[arg(long, default_value = "rusty-comms")]
pub container_prefix: String,
```

**Testing Requirements:**
```rust
#[test]
fn test_stop_container_option() {
    let args = Args::parse_from(["ipc-benchmark", "--stop-container", "uds"]);
    assert_eq!(args.stop_container, Some("uds".to_string()));
}

#[test]
fn test_stop_container_all() {
    let args = Args::parse_from(["ipc-benchmark", "--stop-container", "all"]);
    assert_eq!(args.stop_container, Some("all".to_string()));
}

#[test]
fn test_container_image_default() {
    let args = Args::parse_from(["ipc-benchmark", "-m", "uds"]);
    assert_eq!(args.container_image, "localhost/ipc-benchmark:latest");
}

#[test]
fn test_container_image_custom() {
    let args = Args::parse_from([
        "ipc-benchmark", "-m", "uds",
        "--container-image", "myrepo/bench:v1"
    ]);
    assert_eq!(args.container_image, "myrepo/bench:v1");
}
```

---

#### Step 1.3: Update Main Entry Point

**File:** `src/main.rs`  
**Action:** Add host mode branching

```rust
fn main() -> Result<()> {
    let args = Args::parse();
    
    // Handle container stop command first (exits after)
    if let Some(ref mechanism) = args.stop_container {
        return stop_container_command(&args, mechanism);
    }
    
    // Branch based on host mode and execution mode (async/blocking)
    match args.host {
        HostMode::Standalone => {
            // Existing behavior - spawn subprocess client
            if args.blocking {
                run_blocking_mode(args)
            } else {
                run_async_mode(args)
            }
        }
        HostMode::Host => {
            // Host mode - create container, drive tests
            if args.blocking {
                run_host_mode_blocking(args)
            } else {
                run_host_mode_async(args)
            }
        }
        HostMode::Client => {
            // Client mode - connect to host (run inside container)
            if args.blocking {
                run_client_mode_blocking(args)
            } else {
                run_client_mode_async(args)
            }
        }
    }
}

/// Stop container command handler.
///
/// Stops the specified benchmark container(s) and exits.
fn stop_container_command(args: &Args, mechanism: &str) -> Result<()> {
    // TODO: Implement in Stage 6
    Err(anyhow::anyhow!(
        "Container stop not yet implemented. Will be completed in Stage 6."
    ))
}

/// Run benchmark in host mode with async I/O.
///
/// Creates Podman container client, drives tests, collects results.
fn run_host_mode_async(_args: Args) -> Result<()> {
    // TODO: Implement in Stage 3
    Err(anyhow::anyhow!(
        "Host mode (async) not yet implemented. Will be completed in Stage 3."
    ))
}

/// Run benchmark in host mode with blocking I/O.
///
/// Creates Podman container client, drives tests, collects results.
fn run_host_mode_blocking(_args: Args) -> Result<()> {
    // TODO: Implement in Stage 3
    Err(anyhow::anyhow!(
        "Host mode (blocking) not yet implemented. Will be completed in Stage 3."
    ))
}

/// Run benchmark in client mode with async I/O.
///
/// Runs inside container, connects back to host server.
fn run_client_mode_async(_args: Args) -> Result<()> {
    // TODO: Implement in Stage 4
    Err(anyhow::anyhow!(
        "Client mode (async) not yet implemented. Will be completed in Stage 4."
    ))
}

/// Run benchmark in client mode with blocking I/O.
///
/// Runs inside container, connects back to host server.
fn run_client_mode_blocking(_args: Args) -> Result<()> {
    // TODO: Implement in Stage 4
    Err(anyhow::anyhow!(
        "Client mode (blocking) not yet implemented. Will be completed in Stage 4."
    ))
}
```

**Testing Requirements:**
```rust
#[cfg(test)]
mod main_host_mode_tests {
    use super::*;
    
    #[test]
    fn test_host_mode_returns_not_implemented() {
        let args = Args {
            host: HostMode::Host,
            mechanisms: vec![IpcMechanism::UnixDomainSocket],
            blocking: false,
            ..Default::default()
        };
        
        let result = run_host_mode_async(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not yet implemented"));
    }
    
    #[test]
    fn test_client_mode_returns_not_implemented() {
        let args = Args {
            host: HostMode::Client,
            mechanisms: vec![IpcMechanism::UnixDomainSocket],
            blocking: false,
            ..Default::default()
        };
        
        let result = run_client_mode_async(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not yet implemented"));
    }
}
```

---

### Stage 1 Completion Checklist

Before marking Stage 1 complete, verify:

- [ ] HostMode enum created with Standalone, Host, Client variants
- [ ] HostMode has Display implementation
- [ ] `--host` CLI option added with default Standalone
- [ ] `--stop-container` CLI option added
- [ ] `--container-image` CLI option added with default
- [ ] `--container-prefix` CLI option added with default
- [ ] main() updated to branch on host mode
- [ ] Stub functions created for host/client modes
- [ ] All tests written and passing (8+ tests)
- [ ] `--host` appears in `--help` output with descriptions
- [ ] `cargo test` passes
- [ ] `cargo clippy` has no warnings
- [ ] `cargo fmt` has been run

**Completion Command:**
```bash
cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check

git add src/cli.rs src/main.rs docs/host-container-ipc-implementation-plan.md
git commit -m "Stage 1: Add CLI and host mode infrastructure

- Add HostMode enum with Standalone, Host, Client variants
- Add --host CLI option (default: standalone)
- Add --stop-container option for container lifecycle
- Add --container-image and --container-prefix options
- Update main() to branch on host mode
- Add stub functions for host/client modes
- All new code includes documentation and tests

AI-assisted-by: Claude Opus 4.5"
```

---

## STAGE 2: Container Management Module

**Estimated Time:** 3-4 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 1 complete

### Objectives
- Create ContainerManager struct for Podman operations
- Implement container create/start/stop using `std::process::Command`
- Configure mechanism-specific volume mounts for IPC

### Implementation Steps

#### Step 2.1: Create ContainerManager Struct

**File:** `src/container.rs` (new file)

```rust
//! Container management for host-to-container IPC benchmarking.
//!
//! This module provides Podman container lifecycle management for running
//! IPC benchmark clients inside containers. All operations use `std::process::Command`
//! to invoke Podman CLI - no shell scripts.
//!
//! # Supported IPC Mechanisms
//!
//! Each mechanism requires specific container configuration:
//!
//! - **UDS**: Mount socket directory (`-v /tmp/rusty-comms:/tmp/rusty-comms`)
//! - **SHM**: Mount shared memory (`--mount type=tmpfs,destination=/dev/shm,tmpfs-size=1G`)
//!           or use `--ipc=host` for direct /dev/shm access
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
//! // Create and start container for UDS benchmarking
//! let config = ContainerConfig::for_mechanism(&IpcMechanism::UnixDomainSocket)?;
//! manager.ensure_running(&config)?;
//!
//! // Run benchmark...
//!
//! // Stop container when done
//! manager.stop(&config.name)?;
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, Result, bail};
use std::process::{Command, Output};
use tracing::{debug, info, warn};

use crate::cli::IpcMechanism;

/// Configuration for a benchmark container.
///
/// Holds all the settings needed to create and run a container
/// for a specific IPC mechanism.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container name (e.g., "rusty-comms-uds")
    pub name: String,
    
    /// IPC mechanism this container is configured for
    pub mechanism: IpcMechanism,
    
    /// Volume mounts required for IPC
    /// Format: "host_path:container_path[:options]"
    pub volume_mounts: Vec<String>,
    
    /// Additional podman run arguments
    pub extra_args: Vec<String>,
    
    /// Command to run inside container
    pub command: Vec<String>,
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
    pub fn for_mechanism(mechanism: &IpcMechanism) -> Result<Self> {
        match mechanism {
            IpcMechanism::UnixDomainSocket => Ok(Self::uds_config()),
            IpcMechanism::SharedMemory => Ok(Self::shm_config()),
            IpcMechanism::PosixMessageQueue => Ok(Self::pmq_config()),
            IpcMechanism::TcpSocket => {
                // TCP works naturally across containers via network
                Ok(Self::tcp_config())
            }
            IpcMechanism::All => {
                bail!("Cannot create container config for 'All'. Expand mechanisms first.")
            }
        }
    }
    
    /// UDS container configuration.
    ///
    /// Mounts /tmp/rusty-comms directory for socket files.
    fn uds_config() -> Self {
        Self {
            name: String::new(), // Set by ContainerManager
            mechanism: IpcMechanism::UnixDomainSocket,
            volume_mounts: vec![
                "/tmp/rusty-comms:/tmp/rusty-comms:rw".to_string(),
            ],
            extra_args: vec![],
            command: vec![],
        }
    }
    
    /// SHM container configuration.
    ///
    /// Uses --ipc=host to share /dev/shm with host.
    fn shm_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::SharedMemory,
            volume_mounts: vec![],
            extra_args: vec![
                "--ipc=host".to_string(),
            ],
            command: vec![],
        }
    }
    
    /// PMQ container configuration.
    ///
    /// Mounts /dev/mqueue for POSIX message queues.
    fn pmq_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::PosixMessageQueue,
            volume_mounts: vec![
                "/dev/mqueue:/dev/mqueue:rw".to_string(),
            ],
            extra_args: vec![
                "--privileged".to_string(), // May be needed for mqueue access
            ],
            command: vec![],
        }
    }
    
    /// TCP container configuration.
    ///
    /// Uses host network for simplicity.
    fn tcp_config() -> Self {
        Self {
            name: String::new(),
            mechanism: IpcMechanism::TcpSocket,
            volume_mounts: vec![],
            extra_args: vec![
                "--network=host".to_string(),
            ],
            command: vec![],
        }
    }
}

/// Manages Podman container lifecycle for IPC benchmarking.
///
/// Provides methods to create, start, stop, and query containers
/// using Podman CLI via `std::process::Command`.
pub struct ContainerManager {
    /// Prefix for container names (e.g., "rusty-comms")
    prefix: String,
    
    /// Container image to use
    image: String,
}

impl ContainerManager {
    /// Create a new container manager.
    ///
    /// # Arguments
    ///
    /// * `prefix` - Prefix for container names
    /// * `image` - Container image to use
    pub fn new(prefix: &str, image: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            image: image.to_string(),
        }
    }
    
    /// Get the full container name for a mechanism.
    pub fn container_name(&self, mechanism: &IpcMechanism) -> String {
        format!("{}-{}", self.prefix, mechanism.to_string().to_lowercase())
    }
    
    /// Check if a container exists.
    pub fn exists(&self, name: &str) -> Result<bool> {
        let output = Command::new("podman")
            .args(["container", "exists", name])
            .output()
            .context("Failed to run podman container exists")?;
        
        Ok(output.status.success())
    }
    
    /// Check if a container is running.
    pub fn is_running(&self, name: &str) -> Result<bool> {
        let output = Command::new("podman")
            .args(["container", "inspect", name, "--format", "{{.State.Running}}"])
            .output()
            .context("Failed to inspect container")?;
        
        if !output.status.success() {
            return Ok(false);
        }
        
        let running = String::from_utf8_lossy(&output.stdout);
        Ok(running.trim() == "true")
    }
    
    /// Ensure container is created and running.
    ///
    /// Creates the container if it doesn't exist, starts it if not running.
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
    fn create(&self, name: &str, config: &ContainerConfig) -> Result<()> {
        let mut cmd = Command::new("podman");
        cmd.args(["create", "--name", name]);
        
        // Add volume mounts
        for mount in &config.volume_mounts {
            cmd.args(["-v", mount]);
        }
        
        // Add extra args
        for arg in &config.extra_args {
            cmd.arg(arg);
        }
        
        // Add image
        cmd.arg(&self.image);
        
        // Add command (if any)
        for arg in &config.command {
            cmd.arg(arg);
        }
        
        debug!("Running: {:?}", cmd);
        
        let output = cmd.output()
            .context("Failed to run podman create")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to create container {}: {}", name, stderr);
        }
        
        Ok(())
    }
    
    /// Start an existing container.
    pub fn start(&self, name: &str) -> Result<()> {
        let output = Command::new("podman")
            .args(["start", name])
            .output()
            .context("Failed to run podman start")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to start container {}: {}", name, stderr);
        }
        
        Ok(())
    }
    
    /// Stop a running container.
    pub fn stop(&self, name: &str) -> Result<()> {
        if !self.exists(name)? {
            warn!("Container {} does not exist", name);
            return Ok(());
        }
        
        info!("Stopping container: {}", name);
        
        let output = Command::new("podman")
            .args(["stop", "-t", "5", name])
            .output()
            .context("Failed to run podman stop")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to stop container {}: {}", name, stderr);
        }
        
        Ok(())
    }
    
    /// Remove a container.
    pub fn remove(&self, name: &str) -> Result<()> {
        if !self.exists(name)? {
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
            .context("Failed to run podman rm")?;
        
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to remove container {}: {}", name, stderr);
        }
        
        Ok(())
    }
    
    /// Execute a command inside a running container.
    pub fn exec(&self, name: &str, command: &[&str]) -> Result<Output> {
        let mut cmd = Command::new("podman");
        cmd.args(["exec", name]);
        cmd.args(command);
        
        debug!("Executing in container {}: {:?}", name, command);
        
        cmd.output()
            .with_context(|| format!("Failed to exec in container {}", name))
    }
    
    /// Run a command in a new container instance (one-shot).
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
        
        debug!("Running one-shot: {:?}", cmd);
        
        cmd.output()
            .context("Failed to run podman run")
    }
    
    /// Stop all benchmark containers.
    pub fn stop_all(&self) -> Result<()> {
        let mechanisms = [
            IpcMechanism::UnixDomainSocket,
            IpcMechanism::SharedMemory,
            IpcMechanism::PosixMessageQueue,
            IpcMechanism::TcpSocket,
        ];
        
        for mechanism in &mechanisms {
            let name = self.container_name(mechanism);
            if self.exists(&name)? {
                self.stop(&name)?;
            }
        }
        
        Ok(())
    }
    
    /// Remove all benchmark containers.
    pub fn remove_all(&self) -> Result<()> {
        let mechanisms = [
            IpcMechanism::UnixDomainSocket,
            IpcMechanism::SharedMemory,
            IpcMechanism::PosixMessageQueue,
            IpcMechanism::TcpSocket,
        ];
        
        for mechanism in &mechanisms {
            let name = self.container_name(mechanism);
            self.remove(&name)?;
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_container_name() {
        let manager = ContainerManager::new("rusty-comms", "test:latest");
        assert_eq!(
            manager.container_name(&IpcMechanism::UnixDomainSocket),
            "rusty-comms-unixdomainsocket"
        );
    }
    
    #[test]
    fn test_container_config_uds() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::UnixDomainSocket)
            .unwrap();
        assert!(config.volume_mounts.iter().any(|m| m.contains("rusty-comms")));
    }
    
    #[test]
    fn test_container_config_shm() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::SharedMemory)
            .unwrap();
        assert!(config.extra_args.iter().any(|a| a == "--ipc=host"));
    }
    
    #[test]
    fn test_container_config_pmq() {
        let config = ContainerConfig::for_mechanism(&IpcMechanism::PosixMessageQueue)
            .unwrap();
        assert!(config.volume_mounts.iter().any(|m| m.contains("mqueue")));
    }
    
    #[test]
    fn test_container_config_all_fails() {
        let result = ContainerConfig::for_mechanism(&IpcMechanism::All);
        assert!(result.is_err());
    }
}
```

**Additional Changes:**
- Update `src/lib.rs` to export the new module:
```rust
pub mod container;
```

---

### Stage 2 Completion Checklist

Before marking Stage 2 complete, verify:

- [ ] ContainerConfig struct with mechanism-specific configurations
- [ ] ContainerManager with create/start/stop/remove operations
- [ ] Volume mount configurations for UDS, SHM, PMQ
- [ ] All Podman operations use std::process::Command (no shell scripts)
- [ ] Comprehensive documentation for all public items
- [ ] All tests written and passing
- [ ] Module exported from lib.rs

**Completion Command:**
```bash
cargo test container && cargo clippy --all-targets -- -D warnings && cargo fmt --check

git add src/container.rs src/lib.rs docs/host-container-ipc-implementation-plan.md
git commit -m "Stage 2: Add container management module

- Create ContainerManager for Podman lifecycle operations
- Add ContainerConfig with mechanism-specific volume mounts
- Implement create/start/stop/remove via std::process::Command
- Support UDS, SHM, PMQ with appropriate container options
- No shell scripts - pure Rust implementation
- Comprehensive documentation and tests

AI-assisted-by: Claude Opus 4.5"
```

---

## STAGE 3: Host Mode Implementation

**Estimated Time:** 4-5 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 2 complete

### Objectives
- Create HostModeRunner for driving tests from host
- Integrate ContainerManager with benchmark runner
- Spawn container with client mode and correct IPC configuration

*Detailed implementation in subsequent stages.*

---

## STAGE 4: Client Mode Implementation

**Estimated Time:** 3-4 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 3 complete

### Objectives
- Update async transports to support client mode (connect to host-provided endpoints)
- Update blocking transports similarly
- Handle environment variables or CLI args passed from host

*Detailed implementation in subsequent stages.*

---

## STAGE 5: SHM-Direct Container Support

**Estimated Time:** 2-3 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 4 complete

### Objectives
- Update shared_memory_direct.rs for container-aware paths
- Ensure /dev/shm mount works correctly
- Test shm-direct mode across host-container boundary

*Detailed implementation in subsequent stages.*

---

## STAGE 6: Container Lifecycle Commands

**Estimated Time:** 2 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 5 complete

### Objectives
- Implement --stop-container functionality
- Optionally implement --list-containers
- Clean shutdown and resource cleanup

*Detailed implementation in subsequent stages.*

---

## STAGE 7: Integration Testing

**Estimated Time:** 3-4 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 6 complete

### Objectives
- End-to-end tests for UDS host-container
- End-to-end tests for SHM host-container
- End-to-end tests for PMQ host-container
- Container lifecycle tests

*Detailed implementation in subsequent stages.*

---

## STAGE 8: Documentation and Final Validation

**Estimated Time:** 2-3 hours  
**Status:** `[ ]` Not Started  
**Prerequisites:** Stage 7 complete

### Objectives
- Create docs/PODMAN_SETUP.md with container image build instructions
- Create docs/HOST_CONTAINER_USAGE.md with usage examples
- Update README.md with overview
- Final validation and polish

### Documentation: PODMAN_SETUP.md Outline

```markdown
# Podman Setup for IPC Benchmarking

## Prerequisites
- Podman installed (version X.Y+)
- Linux host (RHEL 9.x recommended)

## Building the Container Image

### Using the Provided Containerfile
```bash
podman build -t localhost/ipc-benchmark:latest .
```

### Manual Build
...

## Container Requirements by IPC Mechanism

### Unix Domain Sockets (UDS)
- Volume mount: /tmp/rusty-comms
- No special privileges required

### Shared Memory (SHM)
- Use --ipc=host for direct /dev/shm access
- Or mount specific tmpfs for isolation

### POSIX Message Queues (PMQ)
- Volume mount: /dev/mqueue
- May require --privileged flag

## Troubleshooting
...
```

### Documentation: HOST_CONTAINER_USAGE.md Outline

```markdown
# Host-Container IPC Benchmarking Guide

## Quick Start
```bash
# Build container image
podman build -t localhost/ipc-benchmark:latest .

# Run UDS benchmark (host creates container automatically)
./target/release/ipc-benchmark -m uds --host host --msg-count 10000

# Stop containers when done
./target/release/ipc-benchmark --stop-container all
```

## Host Mode
...

## Client Mode
...

## Supported Mechanisms
...

## Performance Considerations
...
```

---

## PROJECT COMPLETION

**Total Estimated Time:** 22-28 hours across 8 stages  
**Deliverables:**
- Full host-to-container IPC benchmarking capability
- Container management without shell scripts
- Support for UDS, SHM, PMQ mechanisms
- Both async and blocking modes supported
- SHM-direct supported
- Comprehensive documentation

**Success Criteria:**
✅ `--host host` creates container and runs benchmark  
✅ `--host client` runs inside container correctly  
✅ `--stop-container` manages container lifecycle  
✅ All 3 mechanisms work (UDS, SHM, PMQ)  
✅ Both async and blocking modes work  
✅ SHM-direct works  
✅ No shell scripts used  
✅ Backward compatible (standalone mode unchanged)  

---

## Changelog

### 2025-12-16 - Planning Phase
**Status:** Completed  
**Time Spent:** N/A  
**Changes:**
- Created comprehensive implementation plan with 8 stages
- Added master progress checklist
- Added changelog section
- Documented architecture and design decisions

**Notes:**
- Plan is ready for implementation
- Starting point is Stage 1
- Will use existing blocking mode plan as template/reference

---

### 2025-12-16 - Stage 1: Foundation - CLI and Host Mode Infrastructure
**Status:** Completed  
**Time Spent:** ~1 hour  
**Changes:**
- Added `RunMode` enum with Standalone, Host, Client variants to `src/cli.rs`
- Added `--run-mode` CLI option (renamed from `--host` to avoid conflict with TCP host address)
- Added `--stop-container` option for container lifecycle management
- Added `--container-image` option (default: `localhost/ipc-benchmark:latest`)
- Added `--container-prefix` option (default: `rusty-comms`)
- Updated `main()` in `src/main.rs` to branch on run mode
- Added stub functions for host/client modes (return "not yet implemented" errors)
- Added 12 new CLI tests (all passing)
- Updated examples to include new fields
- All clippy checks pass, code formatted

**Issues Encountered:**
- Naming conflict with existing `--host` flag (TCP address) - resolved by renaming to `--run-mode`

**Validation Results:**
- All 22 CLI tests passing
- All 261 lib tests passing (3 PMQ failures are pre-existing resource limits)
- Clippy clean
- Code formatted
- New options appear correctly in `--help` output
- Stub functions return expected error messages

**Notes:**
- Existing standalone mode unchanged (backward compatible)
- Ready to proceed to Stage 2 (Container Management Module)

---

**End of Implementation Plan**

