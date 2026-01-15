#!/bin/bash

# Test script to verify CPU affinity is working correctly
# This script runs a benchmark in the background and checks process affinity

echo "Testing CPU affinity isolation..."
echo "Starting benchmark: server on core 4, client on core 3"

# Start benchmark in background with 60 second duration
./target/release/ipc-benchmark -m uds --duration 60s -s 1024 --server-affinity 4 --client-affinity 3 > /tmp/affinity_test.log 2>&1 &
BENCHMARK_PID=$!

echo "Benchmark started with PID: $BENCHMARK_PID"
echo "Waiting 5 seconds for processes to start..."
sleep 5

echo "Checking process affinity:"
echo "PID    CPU  COMMAND"
ps -eo pid,psr,comm | grep ipc-benchmark

echo ""
echo "Waiting for benchmark to complete..."
wait $BENCHMARK_PID

echo ""
echo "Benchmark results:"
grep "Successfully set client affinity" /tmp/affinity_test.log
grep "Server Affinity:" /tmp/affinity_test.log
grep "Client Affinity:" /tmp/affinity_test.log

echo ""
echo "Test completed. Check above output to verify:"
echo "1. Client affinity was set to core 3"
echo "2. Server process (if visible) should be on core 4"
echo "3. Client process should be on core 3"







