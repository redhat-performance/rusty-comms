#!/usr/bin/env python3
"""
Generate consolidated CSV from benchmark summary JSON files.

Parses all *_summary.json files in the output directory and creates
a single CSV with all metrics including both one-way and round-trip latencies.
"""

import json
import csv
import sys
from pathlib import Path
from typing import Dict, Any, List, Optional

OUTPUT_DIR = Path("/root/scripts/fullrun/out")
CSV_OUTPUT = OUTPUT_DIR / "benchmark_results.csv"


def parse_filename(filename: str) -> Dict[str, str]:
    """Parse test configuration from filename."""
    result = {
        "filename": filename,
        "mode": "unknown",
        "variant": "unknown", 
        "mechanism": "unknown",
        "size": "0",
        "test_type": "unknown",
    }
    
    if filename.startswith("standalone_async_"):
        result["mode"] = "standalone"
        result["variant"] = "async"
        remaining = filename.replace("standalone_async_", "").replace("_summary.json", "")
    elif filename.startswith("standalone_blocking_"):
        result["mode"] = "standalone"
        result["variant"] = "blocking"
        remaining = filename.replace("standalone_blocking_", "").replace("_summary.json", "")
    elif filename.startswith("standalone_shm_direct_"):
        result["mode"] = "standalone"
        result["variant"] = "shm_direct"
        remaining = filename.replace("standalone_shm_direct_", "").replace("_summary.json", "")
    elif filename.startswith("h2c_blocking_"):
        result["mode"] = "h2c"
        result["variant"] = "blocking"
        remaining = filename.replace("h2c_blocking_", "").replace("_summary.json", "")
    elif filename.startswith("h2c_shm_direct_"):
        result["mode"] = "h2c"
        result["variant"] = "shm_direct"
        remaining = filename.replace("h2c_shm_direct_", "").replace("_summary.json", "")
    elif filename.startswith("c2c_blocking_"):
        result["mode"] = "c2c"
        result["variant"] = "blocking"
        remaining = filename.replace("c2c_blocking_", "").replace("_summary.json", "")
    elif filename.startswith("c2c_shm_direct_"):
        result["mode"] = "c2c"
        result["variant"] = "shm_direct"
        remaining = filename.replace("c2c_shm_direct_", "").replace("_summary.json", "")
    # Handle c2c_<mechanism>_<size>_<type> format (without blocking/shm_direct)
    elif filename.startswith("c2c_"):
        result["mode"] = "c2c"
        result["variant"] = "blocking"  # Default to blocking for c2c
        remaining = filename.replace("c2c_", "").replace("_summary.json", "")
    # Host-to-QM container tests
    elif filename.startswith("h2qm_async_"):
        result["mode"] = "h2qm"
        result["variant"] = "async"
        remaining = filename.replace("h2qm_async_", "").replace("_summary.json", "")
    elif filename.startswith("h2qm_blocking_"):
        result["mode"] = "h2qm"
        result["variant"] = "blocking"
        remaining = filename.replace("h2qm_blocking_", "").replace("_summary.json", "")
    elif filename.startswith("h2qm_shm_direct_"):
        result["mode"] = "h2qm"
        result["variant"] = "shm_direct"
        remaining = filename.replace("h2qm_shm_direct_", "").replace("_summary.json", "")
    # Host-to-non-QM container tests
    elif filename.startswith("h2nqm_async_"):
        result["mode"] = "h2nqm"
        result["variant"] = "async"
        remaining = filename.replace("h2nqm_async_", "").replace("_summary.json", "")
    elif filename.startswith("h2nqm_blocking_"):
        result["mode"] = "h2nqm"
        result["variant"] = "blocking"
        remaining = filename.replace("h2nqm_blocking_", "").replace("_summary.json", "")
    elif filename.startswith("h2nqm_shm_direct_"):
        result["mode"] = "h2nqm"
        result["variant"] = "shm_direct"
        remaining = filename.replace("h2nqm_shm_direct_", "").replace("_summary.json", "")
    # Container-to-Container tests (new format)
    elif filename.startswith("c2c_async_"):
        result["mode"] = "c2c"
        result["variant"] = "async"
        remaining = filename.replace("c2c_async_", "").replace("_summary.json", "")
    # QM C2C tests (both processes inside QM partition)
    elif filename.startswith("qm_c2c_blocking_"):
        result["mode"] = "qm_c2c"
        result["variant"] = "blocking"
        remaining = filename.replace("qm_c2c_blocking_", "").replace("_summary.json", "")
    else:
        return result
    
    # remaining should be: mechanism_size_testtype
    parts = remaining.split("_")
    if len(parts) >= 3:
        result["mechanism"] = parts[0]
        result["size"] = parts[1]
        result["test_type"] = parts[2]
    
    return result


def extract_latency_metrics(latency_data: Dict[str, Any], prefix: str) -> Dict[str, Any]:
    """Extract latency metrics with given prefix (ow_ or rt_)."""
    metrics = {
        f"{prefix}min_ns": "",
        f"{prefix}max_ns": "",
        f"{prefix}mean_ns": "",
        f"{prefix}p99_ns": "",
    }
    
    if not latency_data:
        return metrics
    
    latency = latency_data.get("latency", {})
    
    metrics[f"{prefix}min_ns"] = latency.get("min_ns", "")
    metrics[f"{prefix}max_ns"] = latency.get("max_ns", "")
    metrics[f"{prefix}mean_ns"] = latency.get("mean_ns", "")
    
    # Get p99 from percentiles array
    percentiles = latency.get("percentiles", [])
    for p in percentiles:
        if p.get("percentile", 0) == 99.0:
            metrics[f"{prefix}p99_ns"] = p.get("value_ns", "")
            break
    
    return metrics


def extract_metrics(data: Dict[str, Any]) -> Dict[str, Any]:
    """Extract relevant metrics from JSON data."""
    metrics = {
        "total_messages_sent": "",
        "average_throughput_mb_s": "",
        # One-way latency
        "ow_min_ns": "",
        "ow_max_ns": "",
        "ow_mean_ns": "",
        "ow_p99_ns": "",
        # Round-trip latency
        "rt_min_ns": "",
        "rt_max_ns": "",
        "rt_mean_ns": "",
        "rt_p99_ns": "",
    }
    
    results = data.get("results", [])
    if not results:
        return metrics
    
    result = results[0]
    
    # Extract one-way metrics
    one_way = result.get("one_way_results", {})
    if one_way:
        ow_metrics = extract_latency_metrics(one_way, "ow_")
        metrics.update(ow_metrics)
        # Get throughput from one-way results
        throughput = one_way.get("throughput", {})
        metrics["total_messages_sent"] = throughput.get("total_messages", "")
    
    # Extract round-trip metrics
    round_trip = result.get("round_trip_results", {})
    if round_trip:
        rt_metrics = extract_latency_metrics(round_trip, "rt_")
        metrics.update(rt_metrics)
        # If no one-way results, get throughput from round-trip
        if not one_way:
            throughput = round_trip.get("throughput", {})
            metrics["total_messages_sent"] = throughput.get("total_messages", "")
    
    # Get average_throughput_mb_s from result summary
    result_summary = result.get("summary", {})
    metrics["average_throughput_mb_s"] = result_summary.get("average_throughput_mb_s", "")
    
    return metrics


def main():
    """Generate CSV from all summary JSON files."""
    print(f"Scanning {OUTPUT_DIR} for summary files...")
    
    if not OUTPUT_DIR.exists():
        print(f"ERROR: Output directory not found: {OUTPUT_DIR}")
        sys.exit(1)
    
    # Include normal fullrun outputs and ad-hoc verification outputs.
    json_files = sorted(OUTPUT_DIR.glob("*_summary.json"))
    json_files.extend(sorted(OUTPUT_DIR.glob("*_verify*.json")))
    
    if not json_files:
        print("No summary JSON files found!")
        sys.exit(1)
    
    print(f"Found {len(json_files)} summary files")
    
    rows = []
    
    for json_file in json_files:
        try:
            with open(json_file, 'r') as f:
                data = json.load(f)
            
            config = parse_filename(json_file.name)
            metrics = extract_metrics(data)
            
            row = {
                "test_type": config["test_type"],
                "mode": config["mode"],
                "variant": config["variant"],
                "mechanism": config["mechanism"],
                "message_size": config["size"],
                **metrics,
                "filename": json_file.name,
            }
            
            rows.append(row)
            
        except json.JSONDecodeError as e:
            print(f"  Warning: Failed to parse {json_file.name}: {e}")
        except Exception as e:
            print(f"  Warning: Error processing {json_file.name}: {e}")
    
    if not rows:
        print("No valid data extracted!")
        sys.exit(1)
    
    # Sort by test_type (iter first), then mode, mechanism, variant, size
    def sort_key(row):
        test_order = 0 if row["test_type"] == "iter" else 1
        mode_order = {
            "standalone": 0,
            "h2c": 1,
            "h2qm": 2,
            "h2nqm": 3,
            "c2c": 4,
            "qm_c2c": 5,
        }.get(row["mode"], 9)
        variant_order = {"async": 0, "blocking": 1, "shm_direct": 2}.get(row["variant"], 9)
        mech_order = {"uds": 0, "tcp": 1, "shm": 2, "pmq": 3}.get(row["mechanism"], 9)
        try:
            size = int(row["message_size"])
        except:
            size = 0
        return (test_order, mode_order, mech_order, variant_order, size)
    
    rows.sort(key=sort_key)
    
    # Define CSV columns
    columns = [
        "test_type",
        "mode",
        "variant", 
        "mechanism",
        "message_size",
        "total_messages_sent",
        "average_throughput_mb_s",
        # One-way latency
        "ow_min_ns",
        "ow_max_ns",
        "ow_mean_ns",
        "ow_p99_ns",
        # Round-trip latency
        "rt_min_ns",
        "rt_max_ns",
        "rt_mean_ns",
        "rt_p99_ns",
        "filename",
    ]
    
    # Write CSV
    with open(CSV_OUTPUT, 'w', newline='') as f:
        writer = csv.DictWriter(f, fieldnames=columns, extrasaction='ignore')
        writer.writeheader()
        writer.writerows(rows)
    
    print(f"\nCSV written to: {CSV_OUTPUT}")
    print(f"Total rows: {len(rows)}")
    
    # Print preview
    print("\nPreview (first 10 rows):")
    print("-" * 140)
    header = f"{'type':<5} {'mode':<10} {'var':<10} {'mech':<4} {'size':<6} {'msgs':<8} {'MB/s':<10} {'ow_mean':<12} {'rt_mean':<12}"
    print(header)
    print("-" * 140)
    
    for row in rows[:10]:
        ow_mean = row.get('ow_mean_ns', '')
        rt_mean = row.get('rt_mean_ns', '')
        ow_str = f"{float(ow_mean):.0f}" if ow_mean else "-"
        rt_str = f"{float(rt_mean):.0f}" if rt_mean else "-"
        
        line = f"{row.get('test_type', ''):<5} {row.get('mode', ''):<10} {row.get('variant', ''):<10} "
        line += f"{row.get('mechanism', ''):<4} {row.get('message_size', ''):<6} "
        line += f"{str(row.get('total_messages_sent', '')):<8} "
        line += f"{str(row.get('average_throughput_mb_s', ''))[:8]:<10} "
        line += f"{ow_str:<12} {rt_str:<12}"
        print(line)
    
    if len(rows) > 10:
        print(f"... and {len(rows) - 10} more rows")
    
    return 0


if __name__ == "__main__":
    sys.exit(main())
