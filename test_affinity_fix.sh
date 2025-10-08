#!/bin/bash

# Test script to verify CPU affinity fix is working correctly
# This script runs a benchmark and monitors process affinity in real-time

echo "Testing CPU affinity fix..."
echo "Starting benchmark: server on core 0, client on core 1"

# Start benchmark in background with 15 second duration
./target/release/ipc-benchmark -m uds --duration 15s -s 1024 --server-affinity 0 --client-affinity 1 > /tmp/affinity_fix_test.log 2>&1 &
BENCHMARK_PID=$!

echo "Benchmark started with PID: $BENCHMARK_PID"
echo "Waiting 3 seconds for processes to start..."
sleep 3

echo ""
echo "Monitoring process affinity every 2 seconds:"
echo "PID    CPU  COMMAND"

# Monitor for 10 seconds
for i in {1..5}; do
    echo "--- Check $i ---"
    ps -eo pid,psr,comm | grep ipc-benchmark | head -10
    sleep 2
done

echo ""
echo "Waiting for benchmark to complete..."
wait $BENCHMARK_PID

echo ""
echo "Final results from log:"
grep -E "(Successfully set|Server Affinity|Client Affinity)" /tmp/affinity_fix_test.log

echo ""
echo "Test completed. Expected results:"
echo "1. Server should be on core 0"
echo "2. Client should be on core 1"
echo "3. Both processes should stay on their assigned cores throughout the test"


