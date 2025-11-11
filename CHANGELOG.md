# Changelog

All notable changes to the IPC Benchmark Suite will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added - PR #93: Blocking Mode & Direct Memory SHM

#### Major Features
- **Blocking execution mode** - Complete synchronous I/O implementation for all IPC mechanisms
  - Uses pure `std` library (no Tokio in blocking mode)
  - Add `--blocking` flag to enable blocking mode
  - Maintains backward compatibility (async is default)
  - Both modes coexist in same binary
  
- **Direct memory shared memory implementation** - High-performance C-style SHM
  - Add `--shm-direct` flag for blocking SHM mode
  - Zero serialization overhead (direct memcpy with `#[repr(C)]`)
  - pthread mutex + condition variable synchronization
  - 3× faster average latency (7 μs vs 20 μs)
  - 450× better max latency (22 μs vs 10 ms)
  - Matches C benchmark performance

#### Blocking Transport Implementations
- `BlockingUnixDomainSocket` - Blocking UDS transport
- `BlockingTcpSocket` - Blocking TCP transport  
- `BlockingSharedMemory` - Blocking ring buffer SHM transport
- `BlockingSharedMemoryDirect` - Direct memory SHM transport
- `BlockingPosixMessageQueue` - Blocking PMQ transport (Linux only)

#### Infrastructure
- `BlockingTransport` trait - Common interface for blocking transports
- `BlockingTransportFactory` - Factory for creating blocking transports
- `BlockingBenchmarkRunner` - Benchmark orchestration for blocking mode
- `BlockingResultsManager` - Results collection for blocking mode
- `ExecutionMode` enum - Runtime mode selection

#### Documentation
- Added blocking mode section to README.md
- New METHODOLOGY_CHANGE.md - Timestamp methodology details
- New SHM_ANALYSIS.md - Performance analysis
- New TEST_REPORT.md - Test results
- New AGENTS.md - AI agent guidelines
- New examples/blocking_basic.rs - Basic usage example
- New examples/blocking_comparison.rs - Async vs blocking comparison

#### Testing
- 40 passing doctests
- 24+ integration tests for blocking mode
- Full coverage of all blocking transports
- Advanced scenario testing

### Fixed - PR #93

- **Critical: Warmup reporting bug** - JSON output now correctly reports warmup iterations
  - `warmup_iterations` was hardcoded to 0, now uses actual value
  
- **Critical: Cold-start penalty** - Eliminated 308× first-message latency spike
  - Implemented canary message system (ID = u64::MAX)
  - First message excluded from results by default
  - Max latency improved from 9.98 ms to 32 μs for SHM
  - Add `--include-first-message` flag to include first message if needed

- **Critical: Doctest compilation failures** - Fixed 3 doctests with outdated signatures
  - Updated `BlockingTransportFactory::create()` examples
  - Added missing Args struct fields in benchmark.rs

- **MessageType serialization** - Added `From<u32>` trait for proper deserialization

- **Flag validation** - `--shm-direct` now requires `--blocking` with clear error message

- **Trailing whitespace** - Fixed formatting issues in benchmark.rs and benchmark_blocking.rs

### Changed - PR #93

- **Timestamp methodology** - Now matches C benchmark approach
  - Timestamps captured immediately before IPC syscalls
  - Pre-serialize messages to avoid timing serialization overhead
  - Ensures scheduling delays are included in measured latency

- **Performance** - Rust now matches or exceeds C performance
  - PMQ: 1.02× C (essentially equal)
  - SHM Min: 0.41× C (59% faster!)
  - UDS Avg: 0.79× C (21% faster!)
  - UDS Max: 0.63× C (37% faster!)

### Performance Comparison

#### Direct Memory SHM vs Ring Buffer SHM

| Metric | Ring Buffer | Direct Memory | Improvement |
|--------|-------------|---------------|-------------|
| Mean | 22.51 μs | 7.42 μs | **3.0× faster** |
| Min | 5.73 μs | 5.00 μs | 1.15× faster |
| Max | 9,983 μs | 22.18 μs | **450× faster** |

#### Rust vs C Benchmarks (Blocking Mode)

| Test | Metric | C (ns) | Rust (ns) | Rust/C Ratio |
|------|--------|--------|-----------|--------------|
| PMQ NoLoad | Avg | 8,498 | 8,635 | 1.02× (equal) |
| SHM Load | Avg | 95,094 | 95,719 | 1.01× (equal) |
| SHM Load | Min | 5,729 | 2,344 | **0.41× (59% faster)** |
| UDS NoLoad | Avg | 18,445 | 14,502 | **0.79× (21% faster)** |
| UDS NoLoad | Max | 81,042 | 50,781 | **0.63× (37% faster)** |

### Technical Details

#### Code Statistics
- **12,306 lines added** across 38 files
- **5,305 lines** of blocking implementation code
- **1,642 lines** of documentation
- **30+ commits** with clear progression

#### Quality Metrics
- ✅ 68 tests passing (40 doctests + 28 integration tests)
- ✅ Zero clippy warnings
- ✅ Zero compiler warnings
- ✅ All unsafe code documented with SAFETY comments
- ✅ Comprehensive error handling with context
- ✅ Full backward compatibility maintained

## [0.1.0] - Initial Release

### Added
- Initial async implementation with Tokio runtime
- Unix Domain Sockets (UDS) transport
- TCP Sockets transport
- Shared Memory (ring buffer) transport
- POSIX Message Queues transport (Linux only)
- Comprehensive metrics collection with HDR histograms
- JSON and CSV output formats
- Streaming output support
- CPU affinity controls
- Warmup iterations support

---

## Legend

- **Added** - New features
- **Changed** - Changes to existing functionality
- **Deprecated** - Soon-to-be removed features
- **Removed** - Removed features
- **Fixed** - Bug fixes
- **Security** - Vulnerability fixes

[Unreleased]: https://github.com/redhat-performance/rusty-comms/compare/main...feature/shm-direct-memory
[0.1.0]: https://github.com/redhat-performance/rusty-comms/releases/tag/v0.1.0

