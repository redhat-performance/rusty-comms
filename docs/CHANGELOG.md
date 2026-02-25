# Changelog

All notable changes to the rusty-comms IPC Benchmark Suite will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0]

### Added
- Cross-process testing support with `--run-mode client` and `--run-mode sender`
- Container-safe synchronization with `--cross-container` flag
- Comprehensive documentation structure in `docs/` directory
- Documentation audit report (`docs/DOCUMENTATION_AUDIT.md`)

### Changed
- Simplified container workflow to manual cross-process testing
- Updated CLI examples to use correct flag syntax (`-m` instead of `--mechanism`)

### Removed
- Automatic container management (`--stop-container`, `--list-containers`)
- (Docs-only) references to removed container automation
- `docs/PODMAN_SETUP.md` and `docs/HOST_CONTAINER_USAGE.md`

### Fixed
- Documentation accuracy issues (non-existent flags, incorrect defaults)
- Broken references to deleted files

## [0.1.0] - Initial Release

### Added
- IPC benchmark suite with support for:
  - Unix Domain Sockets (UDS)
  - Shared Memory (SHM) with ring buffer and direct memory modes
  - TCP Sockets
  - POSIX Message Queues (PMQ)
- Async (Tokio) and blocking execution modes
- One-way and round-trip latency measurements
- Statistical analysis with configurable percentiles
- JSON and CSV output formats
- Streaming output for real-time monitoring
- Performance dashboard for result visualization
- CPU affinity controls for reproducible benchmarks
- Comprehensive CLI with extensive configuration options

---

## Version History

| Version | Date | Highlights |
|---------|------|------------|
| 0.2.0 | - | Cross-process testing, breaking: removed container automation |
| 0.1.0 | Initial | Full IPC benchmark suite |
