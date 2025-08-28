#!/bin/bash

# Start Host UDS Server for Cross-Environment Testing
# This creates a server that container clients can connect to

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SOCKET_PATH="./sockets/ipc_benchmark.sock"
OUTPUT_DIR="./output"
MSG_COUNT=${1:-1000}
MSG_SIZE=${2:-1024}

echo -e "${BLUE}=== Starting Host UDS Server ===${NC}"
echo "Socket Path: ${SOCKET_PATH}"
echo "Message Count: ${MSG_COUNT}"
echo "Message Size: ${MSG_SIZE}"
echo

# Create directories
mkdir -p sockets output

# Remove existing socket
rm -f "${SOCKET_PATH}"

echo -e "${BLUE}[INFO]${NC} Starting host server in coordinator mode..."
echo -e "${BLUE}Command:${NC} ./target/release/ipc-benchmark -m uds --mode host --ipc-path ./sockets/ipc_benchmark.sock --msg-count ${MSG_COUNT} --message-size ${MSG_SIZE} --client-count 1 --output-file ${OUTPUT_DIR}/host_server_results.json"
echo

# Start the host server
./target/release/ipc-benchmark \
    -m uds \
    --mode host \
    --ipc-path ./sockets/ipc_benchmark.sock \
    --msg-count ${MSG_COUNT} \
    --message-size ${MSG_SIZE} \
    --client-count 1 \
    --output-file "${OUTPUT_DIR}/host_server_results.json" \
    --log-file stderr

echo -e "${GREEN}[SUCCESS]${NC} Host server session completed!"

