# Utility Scripts

This directory contains Python utility scripts for running comprehensive benchmarks and processing results.

## Scripts

### fullrun.py

A comprehensive benchmark suite runner that automates running all test configurations.

#### Features

- Runs all IPC mechanisms (TCP, UDS, SHM, PMQ)
- Multiple execution modes:
  - Standalone (async/blocking/shm-direct)
  - Host-to-QM (H2QM) - tests from host to a container named "qm"
  - Container-to-Container (C2C)
- Both iteration-based and duration-based tests
- Automatic binary deployment to containers
- Real-time streaming output option
- JSON and CSV output generation

#### Usage

```bash
# Run all tests (quiet mode)
python3 fullrun.py

# Run all tests with real-time streaming output
python3 fullrun.py --stream

# Run only standalone tests
python3 fullrun.py --standalone-only

# Run with streaming for specific message sizes
python3 fullrun.py --stream --sizes 1024,4096

# Generate both JSON and CSV output
python3 fullrun.py --output both
```

#### Command-Line Options

| Option | Description |
|--------|-------------|
| `--standalone-only` | Only run standalone tests (skip container tests) |
| `--h2qm-only` | Only run Host-to-QM container tests |
| `--c2c-only` | Only run Container-to-Container tests |
| `--blocking-only` | Only run blocking mode tests |
| `--async-only` | Only run async mode tests |
| `--stream` | Enable real-time streaming output |
| `--sizes` | Stream only specific sizes (e.g., `--sizes 1024,4096`) |
| `--iter-only` | Stream only iteration-based tests |
| `--dur-only` | Stream only duration-based tests |
| `--output` | Output format: `json`, `csv`, or `both` |

#### Configuration

Edit the script to modify default paths:

```python
BINARY = "/path/to/ipc-benchmark"
OUTPUT_DIR = Path("/path/to/output")
```

Test parameters:

```python
ITERATIONS = 50000    # Messages per iteration test
DURATION_SECS = 10    # Seconds per duration test
WARMUP = 100          # Warmup iterations
```

Message sizes:

```python
PMQ_SIZES = [512, 1024, 4096, 8100]           # PMQ-specific sizes
GENERAL_SIZES = [64, 512, 1024, 4096, 8192, 65536]  # Other mechanisms
```

---

### generate_summary_csv.py

Consolidates multiple benchmark summary JSON files into a single CSV for analysis.

#### Features

- Parses all `*_summary.json` files in the output directory
- Extracts both one-way and round-trip latency metrics
- Outputs a sorted, consolidated CSV
- Provides a preview of results

#### Usage

```bash
# Generate CSV from default output directory
python3 generate_summary_csv.py

# The script reads from OUTPUT_DIR and writes benchmark_results.csv
```

#### Configuration

Edit the script to modify paths:

```python
OUTPUT_DIR = Path("/path/to/json/files")
CSV_OUTPUT = OUTPUT_DIR / "benchmark_results.csv"
```

#### Output Format

The generated CSV includes:

| Column | Description |
|--------|-------------|
| `test_type` | `iter` or `dur` |
| `mode` | `standalone`, `h2qm`, `c2c` |
| `variant` | `async`, `blocking`, `shm_direct` |
| `mechanism` | `uds`, `tcp`, `shm`, `pmq` |
| `message_size` | Size in bytes |
| `total_messages_sent` | Number of messages |
| `average_throughput_mbps` | Throughput in MB/s |
| `ow_min_ns` | One-way minimum latency |
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

The `dashboard/` subdirectory contains the performance analysis dashboard.

See [dashboard/README.md](dashboard/README.md) for dashboard documentation.

---

## Requirements

These scripts require Python 3.6+ with the following standard library modules:
- `subprocess`
- `json`
- `csv`
- `pathlib`

No additional packages are required for the utility scripts. The dashboard has separate requirements in `dashboard/requirements.txt`.
