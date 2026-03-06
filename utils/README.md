# Utility Scripts

This directory contains Python utility scripts for running comprehensive
benchmarks and processing results.

## Scripts

### comprehensive-rusty-comms-testing.py

A comprehensive benchmark suite runner that automates running all IPC test
configurations across multiple execution modes.

#### Features

- Runs all IPC mechanisms (TCP, UDS, SHM, PMQ)
- Multiple execution modes (all enabled by default):
  - **Standalone** — async, blocking, and shm-direct on the host
  - **Host-to-QM container (H2QM)** — host sender to a QM-partitioned
    container receiver
  - **Host-to-non-QM container (H2NQM)** — host sender to a regular
    (non-QM) container receiver
  - **Container-to-Container (C2C)** — sender and receiver each in
    their own container
  - **QM C2C** — both processes inside a QM partition (opt-in)
- Both iteration-based and duration-based tests
- Automatic binary deployment to containers (bind-mounted from
  `target/release/`)
- Real-time streaming output option
- JSON and CSV output generation
- Path overrides via CLI for binary, output directory, and project
  directory

#### Usage

```bash
# Run all tests (quiet mode) — includes Standalone, H2QM, H2NQM, C2C
python3 comprehensive-rusty-comms-testing.py

# Run all tests with real-time streaming output
python3 comprehensive-rusty-comms-testing.py --stream

# Run only standalone tests
python3 comprehensive-rusty-comms-testing.py --standalone-only

# Run only Host-to-non-QM container tests
python3 comprehensive-rusty-comms-testing.py --h2nqm-only

# Run with streaming for specific message sizes
python3 comprehensive-rusty-comms-testing.py --stream --sizes 1024,4096

# Generate both JSON and CSV output
python3 comprehensive-rusty-comms-testing.py --output both

# Include QM C2C tests (not included by default)
python3 comprehensive-rusty-comms-testing.py --include-qm-c2c

# Override default paths
python3 comprehensive-rusty-comms-testing.py --binary /path/to/ipc-benchmark \
    --output-dir /path/to/results \
    --project-dir /path/to/rusty-comms
```

#### Command-Line Options

**Mode Selection** (which test categories to run):

| Option | Description |
|--------|-------------|
| `--standalone-only` | Only run standalone tests (skip all container tests) |
| `--h2qm-only` | Only run Host-to-QM container tests |
| `--h2nqm-only` | Only run Host-to-non-QM container tests |
| `--c2c-only` | Only run Container-to-Container tests |
| `--qm-c2c-only` | Only run C2C tests inside QM partition |
| `--include-qm-c2c` | Include QM C2C tests (not included by default) |
| `--blocking-only` | Only run blocking (synchronous) mode tests |
| `--async-only` | Only run async mode tests |
| `--mechanisms` | Only run specific mechanisms (e.g., `--mechanisms pmq,tcp`) |
| `--allow-selinux-permissive` | Allow `setenforce 0` for PMQ tests in QM containers |

**Streaming Options** (which tests show real-time output):

| Option | Description |
|--------|-------------|
| `--stream` | Enable real-time streaming output |
| `--sizes` | Stream only specific sizes (e.g., `--sizes 1024,4096`) |
| `--iter-only` | Stream only iteration-based tests |
| `--dur-only` | Stream only duration-based tests |

**Output Format:**

| Option | Description |
|--------|-------------|
| `--output` | Output format: `json` (default), `csv`, or `both` |

**Path Overrides:**

| Option | Description |
|--------|-------------|
| `--binary` | Host binary path |
| `--output-dir` | Results output directory |
| `--project-dir` | Rusty-comms project directory |
| `--qm-binary` | Binary path inside QM container |
| `--qm-container` | QM container name |

#### Configuration

Default paths are set at the top of the script and can be overridden
with CLI options:

```python
BINARY = "/root/hostcont/rusty-comms/target/release/ipc-benchmark"
OUTPUT_DIR = Path("/root/scripts/fullrun/out")
RUSTY_COMMS_DIR = Path("/root/hostcont/rusty-comms")
```

Test parameters:

```python
ITERATIONS = 50000    # Messages per iteration test
DURATION_SECS = 10    # Seconds per duration test
WARMUP = 100          # Warmup iterations
```

Message sizes:

```python
PMQ_SIZES = [512, 1024, 4096, 8100]
GENERAL_SIZES = [64, 512, 1024, 4096, 8192, 65536]
```

#### Container Setup

Container-based tests (H2QM, H2NQM, C2C) do **not** build a custom
container image. Instead, the host-compiled binary at
`target/release/ipc-benchmark` is bind-mounted into a stock
`ubi9-minimal` container at `/app/ipc-benchmark`. This means:

- The container always runs whatever binary is in `target/release/`
- You must run `cargo build --release` before testing if you have
  code changes
- The QM path has an automatic binary freshness check; C2C and H2NQM
  do not

---

### generate_summary_csv.py

Consolidates multiple benchmark summary JSON files into a single CSV
for analysis.

#### Features

- Parses all `*_summary.json` and `*_verify*.json` files in the output
  directory
- Extracts both one-way and round-trip latency metrics
- Sorts results by test type, mode, mechanism, communication method,
  and message size
- Outputs a sorted, consolidated CSV
- Provides a preview of the first 10 rows

#### Usage

```bash
# Generate CSV from default output directory
python3 generate_summary_csv.py

# Generate CSV from a custom directory
python3 generate_summary_csv.py --dir /path/to/json/files
```

#### Command-Line Options

| Option | Description |
|--------|-------------|
| `--dir` | Directory containing benchmark JSON files (default: `/root/scripts/fullrun/out`) |

#### Output Format

The generated CSV (`benchmark_results.csv`) includes:

| Column | Description |
|--------|-------------|
| `test_type` | `iter` (iteration-based) or `dur` (duration-based) |
| `mode` | `standalone`, `host to QM cntr`, `host to non QM cntr`, `cntr to cntr`, `host to cntr`, `QM cntr to cntr` |
| `communication_method` | `async`, `blocking`, or `shm_direct` |
| `mechanism` | `uds`, `tcp`, `shm`, `pmq` |
| `message_size` | Size in bytes |
| `total_messages_sent` | Number of messages sent |
| `average_throughput_mb_s` | Throughput in MB/s |
| `ow_min_ns` | One-way minimum latency (nanoseconds) |
| `ow_max_ns` | One-way maximum latency |
| `ow_mean_ns` | One-way mean latency |
| `ow_p99_ns` | One-way P99 latency |
| `rt_min_ns` | Round-trip minimum latency |
| `rt_max_ns` | Round-trip maximum latency |
| `rt_mean_ns` | Round-trip mean latency |
| `rt_p99_ns` | Round-trip P99 latency |
| `filename` | Source JSON filename |

---

## Dashboard

The `dashboard/` subdirectory contains the performance analysis
dashboard.

See [dashboard/README.md](dashboard/README.md) for dashboard
documentation.

---

## Requirements

These scripts require Python 3.6+ with the following standard library
modules:
- `subprocess`
- `json`
- `csv`
- `pathlib`
- `argparse`

No additional packages are required for the utility scripts. The
dashboard has separate requirements in `dashboard/requirements.txt`.
