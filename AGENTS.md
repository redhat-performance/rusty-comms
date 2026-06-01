# AGENTS.md

IPC benchmark suite in Rust. Measures latency and throughput across
multiple transport mechanisms on embedded Linux platforms (ARM/aarch64).

## Constraints

- MSRV is defined by `rust-version` in Cargo.toml. That is the single
  source of truth — do not hardcode the version elsewhere.
- Pin transitive dependencies that pull in editions or features above MSRV.
- Do not add `.cargo/config.toml` with `target-cpu=native` — binaries
  must be portable across ARM variants. Apply CPU tuning at build time.
- No customer or partner names in code, comments, issues, or PR descriptions.

## Pre-commit hooks

Pre-commit hooks run cargo check, clippy, fmt, tests, and MSRV validation.
Never use `--no-verify` without stating the reason and getting user
confirmation. If a required tool is missing, install it rather than
skipping the check.

## Measurement accuracy

This is a benchmarking tool — results must be comparable to equivalent
C programs making the same syscalls. Overhead in the measurement path
directly affects results. Avoid allocations in send/receive hot loops.
Prefer direct libc calls over wrapper crates for timing-critical
syscalls (e.g., `clock_gettime`). Minimize abstraction layers between
the application and the kernel interface. Timestamp capture points
must be documented and placed as close to the actual IPC operation
as possible.

## Code style

- Minimize comments. Don't explain what code does — explain why when
  non-obvious.
- Don't add features or abstractions beyond what the task requires.
- Prefer editing existing files over creating new ones.

## Testing

- AI-assisted code requires measurable test coverage. Library code
  should be testable by tarpaulin — avoid putting testable logic in
  the binary crate (`main.rs`).
- Verify changes on target hardware when performance claims are made.

## Commits

- Include an `AI-Assisted-By:` trailer in commit messages identifying
  the model and tool used.

## PRs

- Reference related issues. Provide evidence (before/after data) for
  performance or correctness claims.
- Keep scope focused. Don't bundle unrelated fixes.
- Rebase on main before requesting review.
