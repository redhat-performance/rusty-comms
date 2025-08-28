#!/bin/bash

# Parameter Examples for Cross-Environment UDS Testing

echo "🎯 Cross-Environment UDS Parameter Examples"
echo "==========================================="
echo

echo "📊 Message Count Examples:"
echo "  ./run_container_client.sh 100 1024 1      # Quick test: 100 messages"
echo "  ./run_container_client.sh 1000 1024 1     # Standard: 1000 messages"
echo "  ./run_container_client.sh 10000 1024 1    # Thorough: 10000 messages"
echo

echo "📏 Message Size Examples:"
echo "  ./run_container_client.sh 1000 64 1       # Small: 64 bytes per message"
echo "  ./run_container_client.sh 1000 1024 1     # Standard: 1KB per message"
echo "  ./run_container_client.sh 1000 4096 1     # Large: 4KB per message"
echo "  ./run_container_client.sh 1000 65536 1    # Very large: 64KB per message"
echo

echo "⚡ Performance Testing Combinations:"
echo "  ./run_container_client.sh 10000 64 1      # Latency focus: many small messages"
echo "  ./run_container_client.sh 100 65536 1     # Throughput focus: few large messages"
echo "  ./run_container_client.sh 1000 1024 1     # Balanced: moderate count & size"
echo

echo "🔬 Direct Benchmark Examples (host-only):"
echo "  ./target/release/ipc-benchmark -m uds --msg-count 5000 --message-size 256"
echo "  ./target/release/ipc-benchmark -m uds --msg-count 1000 --message-size 2048 --warmup-iterations 100"
echo "  ./target/release/ipc-benchmark -m uds --msg-count 50000 --message-size 32"
echo

echo "📋 Current Default Configuration:"
echo "  Message Count: 1000"
echo "  Message Size: 1024 bytes"
echo "  Concurrency: 1"
echo "  Warmup: 500 iterations"

