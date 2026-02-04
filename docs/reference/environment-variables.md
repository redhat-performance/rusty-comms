# Environment Variables

This document lists environment variables that affect ipc-benchmark behavior.

## Logging

### RUST_LOG

Controls the logging level filter.

**Values:** `trace`, `debug`, `info`, `warn`, `error`

```bash
# Show debug messages
RUST_LOG=debug ipc-benchmark -m uds

# Show only errors
RUST_LOG=error ipc-benchmark -m uds

# Module-specific logging
RUST_LOG=rusty_comms::ipc=debug ipc-benchmark -m uds
```

## Binary Resolution

These variables help the spawner find the benchmark binary in test environments.

### CARGO_BIN_EXE_ipc-benchmark

Path hint for the test runner to locate the server binary.

```bash
export CARGO_BIN_EXE_ipc-benchmark=/path/to/ipc-benchmark
cargo test
```

### CARGO_BIN_EXE_ipc_benchmark

Alternative variable name (with underscore instead of hyphen).

```bash
export CARGO_BIN_EXE_ipc_benchmark=/path/to/ipc-benchmark
```

## Paths

### IPC_BENCHMARK_TEMP_DIR

Override the temporary directory for IPC files (sockets, shared memory).

**Default:** `/tmp`

```bash
IPC_BENCHMARK_TEMP_DIR=/var/tmp ipc-benchmark -m uds
```

## System Variables

These standard system variables also affect behavior:

### HOME

Used for resolving paths.

### TMPDIR

Alternative temporary directory on some systems.

### PATH

Used when resolving the binary for server spawning.

## Usage Examples

### Development Testing

```bash
# Maximum verbosity for debugging
RUST_LOG=trace ipc-benchmark -m uds -i 100 -vvv
```

### CI Environment

```bash
# Quiet logging, output to specific file
RUST_LOG=warn ipc-benchmark -m all -i 10000 -o /artifacts/results.json
```

### Cross-Process Testing

```bash
# Server process
RUST_LOG=info ipc-benchmark -m tcp --run-mode client --blocking

# Client process (different terminal)
RUST_LOG=debug ipc-benchmark -m tcp --run-mode sender --blocking -i 1000
```

## See Also

- [CLI Reference](cli-reference.md) - Command-line options
- [Troubleshooting](../user-guide/troubleshooting.md) - Debugging with logging
