#!/usr/bin/env python3
"""
Parse benchmark JSON results and create CSV matching original format.
"""

import json
import os
import csv
import glob
import sys

OUTPUT_DIR = sys.argv[1] if len(sys.argv) > 1 else "/tmp/benchmark_results_atgimb7q"
CSV_OUTPUT = "/root/hosttocontblocking_with_futex.csv"

# CSV header matching original format
header = [
    'filename', 'group', 'mechanism', 'message_size',
    'one_way_max_ns', 'one_way_mean_ns', 'one_way_p99_ns',
    'one_way_messages_per_second', 'one_way_total_messages',
    'round_trip_max_ns', 'round_trip_mean_ns', 'round_trip_p99_ns',
    'round_trip_messages_per_second', 'round_trip_total_messages',
    'average_throughput_mbps', 'total_messages_sent'
]

results = []

for json_file in sorted(glob.glob(f'{OUTPUT_DIR}/*.json')):
    try:
        with open(json_file, 'r') as f:
            data = json.load(f)
        
        if 'headings' not in data or 'data' not in data:
            print(f"Skipping {json_file}: unexpected format")
            continue
        
        headings = data['headings']
        rows = data['data']
        
        if not rows:
            print(f"Skipping {json_file}: no data")
            continue
        
        # Find column indices
        cols = {h: i for i, h in enumerate(headings)}
        
        filename = os.path.basename(json_file)
        
        # Determine group
        group = 'dur' if 'dur' in filename else 'non_dur'
        
        # Get mechanism from first row
        mechanism_col = cols.get('mechanism', -1)
        mechanism = rows[0][mechanism_col] if mechanism_col >= 0 else 'Unknown'
        
        # Get message size from first row
        size_col = cols.get('message_size', -1)
        message_size = rows[0][size_col] if size_col >= 0 else 0
        
        # Extract latencies
        one_way_col = cols.get('one_way_latency_ns', -1)
        round_trip_col = cols.get('round_trip_latency_ns', -1)
        timestamp_col = cols.get('timestamp_ns', -1)
        
        one_way_latencies = []
        round_trip_latencies = []
        timestamps = []
        
        for row in rows:
            if timestamp_col >= 0 and row[timestamp_col]:
                timestamps.append(row[timestamp_col])
            
            if one_way_col >= 0 and row[one_way_col] is not None:
                one_way_latencies.append(row[one_way_col])
            
            if round_trip_col >= 0 and row[round_trip_col] is not None:
                round_trip_latencies.append(row[round_trip_col])
        
        result = {
            'filename': filename,
            'group': group,
            'mechanism': mechanism,
            'message_size': message_size,
        }
        
        # Calculate duration from timestamps
        if len(timestamps) >= 2:
            duration_ns = max(timestamps) - min(timestamps)
            duration_s = duration_ns / 1e9 if duration_ns > 0 else 1
        else:
            duration_s = 10  # default
        
        # Process one-way data
        if one_way_latencies:
            one_way_latencies.sort()
            total = len(one_way_latencies)
            result['one_way_total_messages'] = total
            result['one_way_mean_ns'] = sum(one_way_latencies) / total
            result['one_way_max_ns'] = max(one_way_latencies)
            p99_idx = int(total * 0.99)
            result['one_way_p99_ns'] = one_way_latencies[min(p99_idx, total - 1)]
            result['one_way_messages_per_second'] = total / duration_s if duration_s > 0 else 0
        
        # Process round-trip data
        if round_trip_latencies:
            round_trip_latencies.sort()
            total = len(round_trip_latencies)
            result['round_trip_total_messages'] = total
            result['round_trip_mean_ns'] = sum(round_trip_latencies) / total
            result['round_trip_max_ns'] = max(round_trip_latencies)
            p99_idx = int(total * 0.99)
            result['round_trip_p99_ns'] = round_trip_latencies[min(p99_idx, total - 1)]
            result['round_trip_messages_per_second'] = total / duration_s if duration_s > 0 else 0
        
        # Calculate totals
        total_msgs = len(one_way_latencies) + len(round_trip_latencies)
        result['total_messages_sent'] = total_msgs
        
        # Calculate throughput in Mbps
        throughput_bytes = total_msgs * message_size
        throughput_mbps = (throughput_bytes * 8) / (duration_s * 1e6) if duration_s > 0 else 0
        result['average_throughput_mbps'] = throughput_mbps
        
        results.append(result)
        print(f"Parsed {filename}: {total_msgs} messages, mean={result.get('one_way_mean_ns', 'N/A'):.2f}ns")
        
    except Exception as e:
        print(f"Error processing {json_file}: {e}")

# Sort results
results.sort(key=lambda r: (r['mechanism'], r['message_size'], r['filename']))

# Write CSV
with open(CSV_OUTPUT, 'w', newline='') as f:
    writer = csv.DictWriter(f, fieldnames=header, extrasaction='ignore')
    writer.writeheader()
    
    for r in results:
        row = {h: r.get(h, '') for h in header}
        writer.writerow(row)

print(f"\nCSV written to {CSV_OUTPUT} with {len(results)} rows")
