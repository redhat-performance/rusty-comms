#!/bin/bash

# Simple UDS Server for Cross-Environment Testing
# Creates a socket and waits for external client connections

set -e

SOCKET_PATH=${1:-"/app/sockets/ipc_benchmark.sock"}
MSG_COUNT=${2:-1000}
MSG_SIZE=${3:-1024}

echo "Creating UDS server at: $SOCKET_PATH"
echo "Message Count: $MSG_COUNT"
echo "Message Size: $MSG_SIZE"

# Create socket directory
mkdir -p "$(dirname "$SOCKET_PATH")"

# Remove existing socket if present
rm -f "$SOCKET_PATH"

# Use socat to create a listening socket that forwards to our benchmark
echo "Starting UDS server listening on $SOCKET_PATH"
echo "Waiting for client connections..."

# Simple approach: use socat to bridge the gap
socat UNIX-LISTEN:"$SOCKET_PATH",fork EXEC:"ipc-benchmark -m uds --msg-count $MSG_COUNT --message-size $MSG_SIZE --log-file stderr",pty

