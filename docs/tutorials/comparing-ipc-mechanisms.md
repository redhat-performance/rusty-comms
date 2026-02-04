# Tutorial: Comparing IPC Mechanisms

This tutorial walks through running a comprehensive comparison of all IPC mechanisms.

## Objective

By the end of this tutorial, you will:
- Run benchmarks for all IPC mechanisms
- Generate comparison data
- Analyze the results to understand trade-offs

## Prerequisites

- Built `ipc-benchmark` binary
- Linux system with all mechanisms available
- Basic command-line familiarity

## Step 1: Prepare the Environment

First, ensure you have a clean environment:

```bash
# Build release binary
cargo build --release

# Verify the binary works
./target/release/ipc-benchmark --version

# Clean up any stale resources
rm -f /tmp/ipc_benchmark_*
```

## Step 2: Run a Quick Comparison

Start with a quick test to verify everything works:

```bash
./target/release/ipc-benchmark -m all -i 1000 -v
```

This runs all mechanisms with 1,000 messages each.

## Step 3: Run Full Comparison

Now run a comprehensive comparison:

```bash
./target/release/ipc-benchmark -m all \
  -i 50000 \
  -w 5000 \
  --percentiles 50 95 99 99.9 \
  -o comparison.json \
  --streaming-output-json streaming.json
```

**Parameters explained:**
- `-m all`: Test all mechanisms (UDS, SHM, TCP, PMQ)
- `-i 50000`: 50,000 messages per test
- `-w 5000`: 5,000 warmup messages
- `--percentiles`: Calculate multiple percentile values
- `-o`: Save summary to JSON
- `--streaming-output-json`: Save per-message data

## Step 4: Review Console Output

The console output shows a summary like:

```
Benchmark Results:
-----------------------------------------------------------------
Mechanism: Unix Domain Socket
  One-Way Latency:
      Mean: 3.15 us, P95: 5.21 us, P99: 8.43 us
-----------------------------------------------------------------
Mechanism: Shared Memory
  One-Way Latency:
      Mean: 2.10 us, P95: 3.50 us, P99: 5.20 us
-----------------------------------------------------------------
Mechanism: TCP Socket
  One-Way Latency:
      Mean: 8.50 us, P95: 12.30 us, P99: 18.40 us
-----------------------------------------------------------------
Mechanism: POSIX Message Queue
  One-Way Latency:
      Mean: 6.20 us, P95: 9.10 us, P99: 14.50 us
-----------------------------------------------------------------
```

## Step 5: Analyze JSON Results

Extract specific metrics using `jq`:

```bash
# Compare mean latencies
cat comparison.json | jq -r '.results[] | "\(.mechanism): \(.one_way_results.latency.mean_ns / 1000 | floor) µs"'

# Get P99 latencies
cat comparison.json | jq -r '.results[] | "\(.mechanism): P99 = \(.one_way_results.latency.percentiles[] | select(.percentile == 99.0) | .value_ns / 1000 | floor) µs"'

# Compare throughput
cat comparison.json | jq -r '.results[] | "\(.mechanism): \(.summary.average_throughput_mbps | floor) MB/s"'
```

## Step 6: Compare Message Sizes

Run comparisons with different message sizes:

```bash
for size in 64 256 1024 4096 16384; do
  echo "Testing message size: $size bytes"
  ./target/release/ipc-benchmark -m all \
    -s $size \
    -i 10000 \
    -o "comparison_${size}.json"
done
```

## Step 7: Create a Summary Table

Use the `generate_summary_csv.py` script:

```bash
python3 utils/generate_summary_csv.py
```

Or create your own with Python:

```python
import json
import os

results = []
for f in os.listdir('.'):
    if f.startswith('comparison_') and f.endswith('.json'):
        with open(f) as file:
            data = json.load(file)
            for result in data['results']:
                results.append({
                    'size': data['results'][0]['test_config']['message_size'],
                    'mechanism': result['mechanism'],
                    'mean_us': result['one_way_results']['latency']['mean_ns'] / 1000
                })

for r in sorted(results, key=lambda x: (x['size'], x['mean_us'])):
    print(f"{r['size']:>6} bytes | {r['mechanism']:>20} | {r['mean_us']:>8.2f} µs")
```

## Step 8: Visualize with Dashboard

For interactive analysis, use the dashboard:

```bash
cd utils/dashboard
pip install -r requirements.txt
python3 dashboard.py --dir ../../
```

Open http://localhost:8050 in your browser.

## Understanding the Results

### Typical Performance Ranking

1. **Shared Memory (Direct)** - Fastest (~5-10 µs)
2. **Shared Memory (Ring Buffer)** - Fast (~15-25 µs)
3. **Unix Domain Sockets** - Low latency (~3-10 µs)
4. **POSIX Message Queues** - Moderate (~5-15 µs)
5. **TCP Sockets** - Higher latency (~5-20 µs)

### Factors Affecting Results

- **Message size**: Larger messages favor SHM
- **System load**: Background activity increases variance
- **CPU frequency**: Turbo boost causes inconsistency
- **Kernel version**: Newer kernels often perform better

## Next Steps

- [Cross-Process Testing](cross-process-testing.md) - Test across containers
- [Dashboard Analysis](dashboard-analysis.md) - Deep-dive visualization
- [Performance Tuning](../user-guide/performance-tuning.md) - Optimize results
