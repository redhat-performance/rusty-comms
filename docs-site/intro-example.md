---
sidebar_position: 1
slug: /
---

# Introduction

Welcome to the **Rusty-Comms** documentation! Rusty-comms is a comprehensive IPC (Inter-Process Communication) benchmark suite written in Rust.

## What is Rusty-Comms?

Rusty-comms measures the performance of different IPC mechanisms:

- **Unix Domain Sockets (UDS)** - Low-latency local communication
- **Shared Memory (SHM)** - Maximum throughput for large data
- **TCP Sockets** - Network-capable communication
- **POSIX Message Queues (PMQ)** - Kernel-managed messaging

## Quick Start

```bash
# Build the project
cargo build --release

# Run your first benchmark
./target/release/ipc-benchmark -m uds -i 10000
```

## Key Features

- **Dual Execution Modes**: Async (Tokio) and blocking (std) I/O
- **Comprehensive Metrics**: Latency percentiles, throughput, statistics
- **Multiple Output Formats**: JSON, CSV, streaming data
- **Performance Dashboard**: Interactive visualization

## Documentation Structure

| Section | Description |
|---------|-------------|
| [User Guide](user-guide/getting-started) | Step-by-step instructions |
| [Reference](reference/cli-reference) | Complete CLI and API documentation |
| [Tutorials](tutorials/comparing-ipc-mechanisms) | Hands-on guides |
| [Concepts](concepts/latency-measurement) | Background information |

## System Requirements

- **Rust**: 1.70.0 or later
- **OS**: Linux (tested on RHEL 9.6)
- **Platform**: x86_64

## Get Help

- [Troubleshooting Guide](user-guide/troubleshooting)
- [GitHub Issues](https://github.com/redhat-performance/rusty-comms/issues)
