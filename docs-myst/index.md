# Rusty-Comms Documentation

Welcome to the comprehensive documentation for **rusty-comms**, an IPC benchmark suite for measuring interprocess communication performance.

## Quick Start

```bash
# Build and run a simple benchmark
cargo build --release
./target/release/ipc-benchmark -m uds -i 10000
```

## Documentation Contents

```{toctree}
:maxdepth: 2
:caption: User Guide

../docs/user-guide/getting-started
../docs/user-guide/basic-usage
../docs/user-guide/advanced-usage
../docs/user-guide/ipc-mechanisms
../docs/user-guide/troubleshooting
```

```{toctree}
:maxdepth: 2
:caption: Reference

../docs/reference/cli-reference
```

```{toctree}
:maxdepth: 1
:caption: Project

../README
../CONTRIBUTING
../docs/CHANGELOG
```

## Overview

Rusty-comms is a Rust-based IPC benchmark suite that measures:

- **Latency**: One-way and round-trip message timing
- **Throughput**: Messages per second and bytes per second
- **Statistical metrics**: Percentiles, min/max, standard deviation

### Supported IPC Mechanisms

| Mechanism | Description | Platform |
|-----------|-------------|----------|
| UDS | Unix Domain Sockets | Unix |
| SHM | Shared Memory | All |
| TCP | TCP Sockets | All |
| PMQ | POSIX Message Queues | Linux |

## Getting Help

- [Troubleshooting Guide](../docs/user-guide/troubleshooting.md)
- [GitHub Issues](https://github.com/redhat-performance/rusty-comms/issues)
