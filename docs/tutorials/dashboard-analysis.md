# Tutorial: Dashboard Analysis

This tutorial shows how to use the performance dashboard to analyze benchmark results.

## Objective

By the end of this tutorial, you will:
- Set up and run the dashboard
- Navigate the dashboard interface
- Perform common analysis tasks

## Prerequisites

- Python 3.8+
- Benchmark result files (JSON/CSV)
- Web browser

## Step 1: Install Dependencies

```bash
cd utils/dashboard
pip install -r requirements.txt
```

Required packages:
- dash
- plotly
- pandas
- numpy

## Step 2: Generate Dashboard-Compatible Data

Run benchmarks with the required output flags:

```bash
mkdir -p ./results
./target/release/ipc-benchmark -m all \
  -i 50000 \
  -o ./results/summary.json \
  --streaming-output-json ./results/streaming.json \
  --continue-on-error
```

This creates both summary and streaming files needed by the dashboard.

## Step 3: Start the Dashboard

```bash
python3 dashboard.py --dir ../../results --host 0.0.0.0 --port 8050
```

**Options:**
- `--dir`: Path to result files
- `--host`: Bind address (use 0.0.0.0 for network access)
- `--port`: Port number (default: 8050)

## Step 4: Access the Dashboard

Open your browser to:

```
http://localhost:8050
```

## Dashboard Overview

### Summary Tab

The Summary tab provides:

- **Performance Cards**: Quick stats for each mechanism
- **Comparison Charts**: Bar charts comparing mechanisms
- **Statistical Tables**: Detailed metrics

### Time Series Tab

The Time Series tab shows:

- **Latency Over Time**: Per-message latency plots
- **Moving Averages**: Smoothed latency trends
- **Anomaly Detection**: Identifies latency spikes

### Filters

Use the sidebar to filter:

- **Mechanism**: Select specific IPC types
- **Message Size**: Filter by payload size
- **Test Type**: One-way vs round-trip

## Common Analysis Tasks

### Compare Mechanism Performance

1. Go to **Summary** tab
2. View the comparison bar chart
3. Note which mechanism has lowest latency

### Identify Latency Spikes

1. Go to **Time Series** tab
2. Look for sudden increases in the latency plot
3. Hover over spikes to see exact values

### Analyze Percentile Distribution

1. Go to **Summary** tab
2. Find the percentile table
3. Compare P50, P95, P99, P99.9 across mechanisms

### Export Data

1. Use the **Export** button
2. Choose format (CSV or PNG)
3. Save for reports

## Understanding the Visualizations

### Latency Distribution

Shows the spread of latency values:
- Narrow distribution = consistent performance
- Wide distribution = variable performance
- Long tail = occasional high latency

### Throughput Comparison

Bar chart showing messages/second:
- Higher is better
- Affected by message size

### Time Series Plot

Shows latency for each message:
- Flat line = stable performance
- Increasing trend = possible resource exhaustion
- Spikes = system interference

## Troubleshooting

### "No Data Available"

**Cause:** Dashboard can't find result files

**Solution:**
1. Check the `--dir` path is correct
2. Verify files exist with correct names
3. Ensure both summary and streaming files present

### Missing Time Series

**Cause:** No streaming output files

**Solution:**
Re-run benchmarks with `--streaming-output-json`

### Slow Performance

**Cause:** Too many data points

**Solution:**
1. Use filters to reduce data
2. Close other browser tabs
3. Use shorter test durations

## Best Practices

1. **Generate complete data**: Use both `-o` and `--streaming-output-json`
2. **Organize results**: Use consistent naming in output directories
3. **Filter appropriately**: Don't try to display everything at once
4. **Export findings**: Save charts for documentation

## Example Workflow

```bash
# 1. Run comprehensive benchmarks
mkdir -p ./analysis
./target/release/ipc-benchmark -m all \
  -i 50000 \
  -o ./analysis/summary.json \
  --streaming-output-json ./analysis/streaming.json

# 2. Start dashboard
cd utils/dashboard
python3 dashboard.py --dir ../../analysis

# 3. Open browser and analyze
# http://localhost:8050

# 4. Export results as needed
```

## See Also

- [utils/dashboard/README.md](../../utils/dashboard/README.md) - Detailed dashboard docs
- [Output Formats](../user-guide/output-formats.md) - Understanding result files
- [Comparing IPC Mechanisms](comparing-ipc-mechanisms.md) - Generating comparison data
