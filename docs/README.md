# Rusty-Comms Documentation

Welcome to the comprehensive documentation for **rusty-comms**, an IPC benchmark suite for measuring interprocess communication performance.

## Quick Start

```bash
# Build
cargo build --release

# Run your first benchmark
./target/release/ipc-benchmark -m uds -i 10000
```

## Documentation Structure

### [User Guide](user-guide/README.md)

Step-by-step guides for using the benchmark tool:

| Guide | Description |
|-------|-------------|
| [Getting Started](user-guide/getting-started.md) | Installation and first benchmark |
| [Basic Usage](user-guide/basic-usage.md) | Common command patterns |
| [Advanced Usage](user-guide/advanced-usage.md) | Cross-process testing, CPU affinity |
| [IPC Mechanisms](user-guide/ipc-mechanisms.md) | UDS, SHM, TCP, PMQ details |
| [Output Formats](user-guide/output-formats.md) | JSON, CSV, console output |
| [Performance Tuning](user-guide/performance-tuning.md) | System configuration |
| [Troubleshooting](user-guide/troubleshooting.md) | Common issues and solutions |

### [Reference](reference/README.md)

Complete technical reference:

| Reference | Description |
|-----------|-------------|
| [CLI Reference](reference/cli-reference.md) | All command-line options |
| [JSON Schema](reference/json-schema.md) | Output file format specification |
| [Environment Variables](reference/environment-variables.md) | Environment configuration |

### [Tutorials](tutorials/README.md)

Hands-on tutorials for specific use cases:

| Tutorial | Description |
|----------|-------------|
| [Comparing IPC Mechanisms](tutorials/comparing-ipc-mechanisms.md) | Run comparative benchmarks |
| [Cross-Process Testing](tutorials/cross-process-testing.md) | Test across containers |
| [Dashboard Analysis](tutorials/dashboard-analysis.md) | Visualize results |

### [Concepts](concepts/README.md)

Background information and architectural decisions:

| Concept | Description |
|---------|-------------|
| [Latency Measurement](concepts/latency-measurement.md) | Timing methodology |
| [Async vs Blocking](concepts/async-vs-blocking.md) | Execution mode comparison |
| [Shared Memory Options](concepts/shared-memory-options.md) | Ring buffer vs direct |

## Quick Links

- [Quick Reference](QUICK_REFERENCE.md) - Command examples at a glance
- [User Guide (Single File)](USER_GUIDE.md) - Complete guide in one document
- [Main README](../README.md) - Project overview
- [Changelog](CHANGELOG.md) - Version history
- [Contributing Guide](../CONTRIBUTING.md) - How to contribute
- [Dashboard](../utils/dashboard/README.md) - Performance dashboard

## Multi-Format Documentation

This documentation is available in multiple formats:

| Format | Location | Build Command |
|--------|----------|---------------|
| Markdown | `docs/` | (source) |
| AsciiDoc | `docs-asciidoc/` | `asciidoctor *.adoc` |
| Sphinx/MyST | `docs-myst/` | `make html` |
| Docusaurus | `docs-site/` | `npm run build` |

## Version

This documentation is for rusty-comms on the `container-to-container-ipc` branch.
