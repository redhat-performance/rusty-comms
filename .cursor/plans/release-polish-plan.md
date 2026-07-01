# Release Polish Plan — rusty-comms

**Branch:** `chore/release-polish`  
**Base:** `main`  
**Status:** Complete  
**Date:** 2026-06-30  

---

## Objective

Comprehensive code quality review and polish of the rusty-comms IPC benchmark
suite in preparation for initial release. Focus areas: code quality, efficiency,
cleanup, minor bug fixes, and issue creation for larger problems.

---

## Review Groups

The codebase was reviewed in 7 logical groups, each examined by a dedicated
review pass analyzing every file for bugs, performance issues, dead code,
style violations, and documentation accuracy.

| Group | Files | Status |
|-------|-------|--------|
| 1. Core types | `ipc/mod.rs`, `execution_mode.rs`, `cli.rs` | Done |
| 2. Transport implementations | 10 files in `src/ipc/` (~8.6K lines) | Done |
| 3. Benchmark runners | `benchmark.rs`, `benchmark_blocking.rs` | Done |
| 4. Results and metrics | `results.rs`, `results_blocking.rs`, `metrics.rs` | Done |
| 5. Standalone client/server | `standalone_server.rs`, `standalone_client.rs` | Done |
| 6. Main and utilities | `main.rs`, `utils.rs`, `lib.rs`, `logging.rs` | Done |
| 7. Tests and examples | Integration tests, unit test quality | Done |

---

## Inline Fixes Applied

### Bug Fixes

| File | Issue | Fix |
|------|-------|-----|
| `src/cli.rs` | `parse_duration_micros` silently truncated fractional values (e.g. "1.5ms") due to `as u64` cast | Changed to `Duration::from_secs_f64()` with appropriate divisors, matching `parse_duration()` |
| `src/ipc/unix_domain_socket_blocking.rs` | `writev` partial-write fallback used incorrect offset calculation (`written.saturating_sub(4)`) — could corrupt frames when `written < 4` | Rewrote to handle partial length-prefix and partial payload cases separately; added `writev == 0` error handling |
| `src/ipc/tcp_socket.rs` | `panic!` in multi-server accept loop when `try_clone()` fails | Replaced with `warn!` + `continue` to keep server running |
| `src/ipc/shared_memory_direct.rs` | No validation of `payload_len` read from shared memory before `copy_nonoverlapping` — potential OOB read | Added bounds check against `MAX_PAYLOAD_SIZE` with proper mutex cleanup on failure |
| `src/ipc/shared_memory_direct.rs` | `receive_blocking_timed` captured a new timestamp instead of using the already-captured `msg.receive_time_ns` | Changed to return the in-message timestamp for accurate latency measurement |
| `src/ipc/shared_memory_blocking.rs` | `read_data_blocking` did not reject `data_len == 0` (inconsistent with other transports) | Added zero-length rejection to match protocol validation |
| `src/results.rs` | `get_memory_gb()` hardcoded `16.0` — incorrect system metadata in JSON output | Replaced with actual `/proc/meminfo` read on Linux |
| `src/results.rs` | `SystemInfo::default()` hardcoded `rust_version: "1.75.0"` and `memory_gb: 16.0` | Changed to use `get_memory_gb()` and `env!("CARGO_PKG_RUST_VERSION")` |
| `src/results_blocking.rs` | Same hardcoded `get_memory_gb()` returning `16.0` | Same `/proc/meminfo` fix |

### Performance Fixes

| File | Issue | Fix |
|------|-------|-----|
| `src/main.rs` | `.context(format!(...))` eagerly allocates format string on every call | Changed to `.with_context(\|\| format!(...))` (lazy evaluation) |

### Code Cleanup

| File | Issue | Fix |
|------|-------|-----|
| `src/ipc/mod.rs` | `Message::new()` and `Message::new_for_blocking()` were identical implementations | Made `new_for_blocking` delegate to `new()` with `#[inline]` — semantic alias |
| `src/ipc/mod.rs` | Test `test_timestamp_offset_update_in_serialized_buffer` was outside `mod tests {}` block | Moved inside the test module |
| `src/ipc/mod.rs` | Test `test_message_size_analysis` used `println!` | Replaced with proper assertions |
| `src/benchmark_blocking.rs` | `if true { ... }` dead scaffolding wrapping round-trip recording | Removed the dead conditional |
| `src/utils.rs` | Dead function `current_timestamp_ns()` (unused — callers use `MessageLatencyRecord::current_timestamp_ns()`) | Removed function and unused imports |
| `src/main.rs` | Stale dev comment `// === ALL EXISTING MAIN() LOGIC STARTS HERE ===` | Removed |

### Style / Documentation Fixes

| File | Issue | Fix |
|------|-------|-----|
| `src/ipc/unix_domain_socket.rs` | `println!` in backpressure test | Changed to `tracing::trace!` |
| `src/ipc/tcp_socket.rs` | `println!` in backpressure test | Changed to `tracing::trace!` |
| `src/ipc/shared_memory.rs` | `println!` in backpressure test | Changed to `tracing::trace!` |
| `src/ipc/posix_message_queue.rs` | `println!` in backpressure test | Changed to `tracing::trace!` |
| `src/ipc/shared_memory_direct.rs` | Module doc said `#[repr(C, packed)]` but struct is `#[repr(C)]` | Fixed to `#[repr(C)]` |
| `src/ipc/shared_memory_direct.rs` | Comment said "Maximum 1MB" but `MAX_PAYLOAD_SIZE` is 8 KB | Updated to "Maximum 8 KB" |
| `src/main.rs` | Doc said "No streaming output support (will be added in Stage 5)" — streaming is implemented | Updated to reflect current state |
| `src/benchmark_blocking.rs` | Same stale "Stage 5" streaming doc | Updated |
| `src/lib.rs` | "Zero-copy serialization" claim (bincode allocates; only SHM-direct is zero-copy) | Changed to "Compact binary serialization via bincode" |
| `src/lib.rs` | "Async I/O throughout using Tokio" ignores blocking mode | Updated to mention both modes |
| `src/lib.rs` | Warmup comment cited "JIT compilation" (not applicable to AOT Rust) | Changed to CPU cache/TLB warming |
| `src/logging.rs` | Module docs described a non-existent `init_logging` function with usage example | Rewrote to describe actual exports (`ColorizedFormatter`) |
| `src/results.rs` | `rust_version` field doc said "compiler version" — it's actually MSRV | Fixed doc and method comment |

---

## GitHub Issues Filed (Architectural / Beyond Polish Scope)

| # | Title | Type |
|---|-------|------|
| [#120](https://github.com/redhat-performance/rusty-comms/issues/120) | Async multi-threaded benchmark aggregation produces incorrect metrics | bug |
| [#121](https://github.com/redhat-performance/rusty-comms/issues/121) | Blocking benchmark runner does not forward duration to spawned server | bug |
| [#122](https://github.com/redhat-performance/rusty-comms/issues/122) | Blocking round-trip ignores include_first_message flag | bug |
| [#123](https://github.com/redhat-performance/rusty-comms/issues/123) | Incorrect pooled standard deviation in multi-worker metrics aggregation | bug |
| [#124](https://github.com/redhat-performance/rusty-comms/issues/124) | Async server spawn missing CLI flag forwarding (buffer-size, send-delay, etc.) | bug |
| [#125](https://github.com/redhat-performance/rusty-comms/issues/125) | Async SHM multi-server path is non-functional | bug |
| [#126](https://github.com/redhat-performance/rusty-comms/issues/126) | Mutex held across await in async multi-client send_to_connection | enhancement |
| [#127](https://github.com/redhat-performance/rusty-comms/issues/127) | Standalone async one-way latency measurement includes executor scheduling delay | bug |
| [#128](https://github.com/redhat-performance/rusty-comms/issues/128) | Transport config parity: async vs blocking runners use different defaults | enhancement |
| [#129](https://github.com/redhat-performance/rusty-comms/issues/129) | Server process exit status not checked after wait() | bug |
| [#130](https://github.com/redhat-performance/rusty-comms/issues/130) | Deduplicate logging setup and implement init_logging | enhancement |
| [#131](https://github.com/redhat-performance/rusty-comms/issues/131) | pthread_mutex_init/condattr_init return values unchecked in SHM-direct | bug |

---

## Validation

```
cargo fmt       ✓  (no changes)
cargo clippy    ✓  (0 warnings with -D warnings)
cargo test      ✓  (42 passed, 0 failed)
cargo build     ✓
```

---

## Files Modified (17 files)

```
Cargo.toml                              |  8 ----
README.md                               |  6 +--
src/benchmark_blocking.rs               | 33 ++++++------
src/cli.rs                              |  7 +--
src/ipc/mod.rs                          | 96 ++++++++++++--------------------
src/ipc/posix_message_queue.rs          |  4 +-
src/ipc/shared_memory.rs                |  4 +-
src/ipc/shared_memory_blocking.rs       |  6 +--
src/ipc/shared_memory_direct.rs         | 43 +++++++++------
src/ipc/tcp_socket.rs                   | 53 ++++++++++++-------
src/ipc/unix_domain_socket.rs           |  4 +-
src/ipc/unix_domain_socket_blocking.rs  | 33 +++++++++---
src/lib.rs                              |  8 +--
src/logging.rs                          | 37 +++----------
src/main.rs                             |  6 +--
src/results.rs                          | 50 ++++++++++--------
src/results_blocking.rs                 | 44 +++++++++-------
src/utils.rs                            | 34 ------------
```

---

## Documentation Review (Additional Pass)

### Inline Fixes Applied

| File | Issue | Fix |
|------|-------|-----|
| `README.md` | Clock source said "via the nix crate" — uses `libc::clock_gettime` directly | Fixed to "via direct `libc::clock_gettime` call" |
| `README.md` | Windows readiness signal incorrectly described as `echo R` | Fixed to "single byte `0x01` on all platforms" |
| `README.md` | Broken markdown fence at line ~548 (unclosed code block) | Added closing ` ``` ` |
| `README.md` | Referenced non-existent `--no-one-way` flag | Changed to `--round-trip` (positive selection) |

### GitHub Issue Filed

| # | Title | Type |
|---|-------|------|
| [#132](https://github.com/redhat-performance/rusty-comms/issues/132) | Documentation audit: README.md and CONFIG.md have significant drift from implementation | documentation |

### Additional Inline Fixes (Second Pass)

| File | Issue | Fix |
|------|-------|-----|
| `docs/archive/blocking-mode-implementation-plan.md` | Header said "Status: PLANNING" despite all 9 stages being complete | Added historical banner and changed status to "COMPLETE (archived)" |
| `utils/dashboard/README.md` | Example used non-existent `--mechanism SharedMemory` and `-o` as directory | Rewrote with correct flags (`-m shm`, explicit file paths) and added note about `-o` semantics |

### Additional GitHub Issues Filed (Second Pass)

| # | Title | Type |
|---|-------|------|
| [#133](https://github.com/redhat-performance/rusty-comms/issues/133) | CONTRIBUTING.md: wrong test layout, fictional TransportManager, stale hook/CI paths | documentation |
| [#134](https://github.com/redhat-performance/rusty-comms/issues/134) | SHM_COMPARISON.md omits async ring buffer, has wrong round-trip scope | documentation |

### Summary of Documentation Drift Found

**CONFIG.md:** Extensive sections describe non-existent features (`--config`,
`--dry-run`, config file loading). Dashboard integration sections describe
incorrect `-o` semantics (directory vs file). Standalone client/server mode
undocumented. Multiple CLI flags missing from reference tables. Requires
ground-up rewrite tracked in issue #132.

**CONTRIBUTING.md:** Wrong test directory layout, fictional `TransportManager`
type, stale hook/CI paths, no mention of blocking transport guidance. Tracked
in issue #133.

**SHM_COMPARISON.md:** Omits async ring buffer entirely, incorrectly claims
round-trip is unsupported globally, stale line counts. Tracked in issue #134.

**CLAUDE.md / GEMINI.md:** Missing blocking/standalone/shm-direct context for
AI agents. Low priority — these mirror AGENTS.md which does cover blocking mode.

**Dashboard README:** Fixed inline (wrong CLI flags, `-o` as directory).

**Archived blocking plan:** Fixed inline (stale PLANNING status).

---

## Cargo.toml and LICENSE Review

### LICENSE File

Standard Apache License 2.0 full text (including APPENDIX). Valid and consistent
with `license = "Apache-2.0"` in Cargo.toml. No issues.

### Cargo.toml Fixes Applied

| Issue | Fix |
|-------|-----|
| `repository` URL was placeholder (`your-org/ipc-benchmark`) | Changed to `https://github.com/redhat-performance/rusty-comms` |
| `documentation` URL pointed to dead `docs.rs/ipc-benchmark` (crate not published) | Removed field entirely |
| `statistics = "0.4"` — not imported anywhere in `src/` or `tests/` | Removed |
| `crossbeam = "0.8"` — not imported anywhere in `src/` or `tests/` | Removed |
| `rand = "0.8"` — not imported anywhere in `src/` or `tests/` | Removed |
| `nix` listed unconditionally with unused `time` feature (code uses `libc::clock_gettime` directly) | Removed unconditional entry; Linux-only `mqueue` entry retained |

### Cargo.toml Notes (No Action Taken)

- **Transitive MSRV pins** (`rayon`, `zmij`, `half`, `parking_lot_core`, etc.) — all correctly
  documented and necessary for Rust 1.70 compatibility. Left as-is.
- **`authors = ["IPC Benchmark Contributors"]`** — generic but acceptable for an org project.
- **`parking_lot`** — IS used directly (`src/ipc/shared_memory.rs`, `shared_memory_blocking.rs`).
