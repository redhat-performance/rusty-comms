#!/bin/bash

# Rusty Comms - Host Client Script for UDS Host-Container IPC Testing
# This script runs the IPC benchmark client on the host side, connecting to a containerized server

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SOCKET_DIR="./sockets"
OUTPUT_DIR="./output"
SOCKET_PATH="${SOCKET_DIR}/ipc_benchmark.sock"
DEFAULT_MESSAGES=1000
DEFAULT_MESSAGE_SIZE=1024
DEFAULT_WORKERS=1

# Parse command line arguments
MESSAGES=${1:-$DEFAULT_MESSAGES}
MESSAGE_SIZE=${2:-$DEFAULT_MESSAGE_SIZE}
WORKERS=${3:-$DEFAULT_WORKERS}

# Optional flags:
#   DURATION (e.g., 30s), OUTPUT_FILE, STREAM_JSON, STREAM_CSV
DURATION=${DURATION:-}
OUTPUT_FILE=${OUTPUT_FILE:-"${OUTPUT_DIR}/host_client_results.json"}
STREAM_JSON=${STREAM_JSON:-}
STREAM_CSV=${STREAM_CSV:-}

echo -e "${BLUE}=== Rusty Comms Host Client Setup ===${NC}"
echo -e "Messages: ${GREEN}${MESSAGES}${NC}"
echo -e "Message Size: ${GREEN}${MESSAGE_SIZE}${NC} bytes"
echo -e "Workers: ${GREEN}${WORKERS}${NC}"
echo -e "Socket Path: ${GREEN}${SOCKET_PATH}${NC}"
if [ -n "$DURATION" ]; then echo -e "Duration: ${GREEN}${DURATION}${NC}"; fi
echo -e "Output: ${GREEN}${OUTPUT_FILE}${NC}"
if [ -n "$STREAM_JSON" ]; then echo -e "Streaming JSON: ${GREEN}${STREAM_JSON}${NC}"; fi
if [ -n "$STREAM_CSV" ]; then echo -e "Streaming CSV: ${GREEN}${STREAM_CSV}${NC}"; fi
echo

# Function to print status
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Cleanup function
cleanup() {
    print_status "Cleaning up..."
    # Intentionally do not delete the shared socket; it is owned by the server
}

# Set up trap for cleanup
trap cleanup EXIT

# Create necessary directories
print_status "Creating directories..."
mkdir -p "${SOCKET_DIR}" "${OUTPUT_DIR}"

# Build the project if needed
if [ ! -f "./target/release/ipc-benchmark" ]; then
    print_status "Building rusty-comms..."
    cargo build --release
    if [ $? -ne 0 ]; then
        print_error "Failed to build project"
        exit 1
    fi
    print_success "Build completed"
else
    print_status "Using existing build"
fi

# Do not wait for pre-existing socket in client mode; the program will create it

# Set environment variables
export RUST_LOG=info
export IPC_BENCHMARK_TEMP_DIR=/tmp
export IPC_BENCHMARK_OUTPUT_DIR="${OUTPUT_DIR}"

# Run the client
print_status "Starting IPC benchmark client..."
CMD=(
  ./target/release/ipc-benchmark
  -m uds
  --mode host
  --ipc-path "${SOCKET_PATH}"
  --message-size ${MESSAGE_SIZE}
  --concurrency ${WORKERS}
)
if [ -n "$DURATION" ]; then
  CMD+=( --duration "$DURATION" )
else
  CMD+=( --msg-count ${MESSAGES} )
fi
CMD+=( --output-file "${OUTPUT_FILE}" )
if [ -n "$STREAM_JSON" ]; then CMD+=( --streaming-output-json "$STREAM_JSON" ); fi
if [ -n "$STREAM_CSV" ]; then CMD+=( --streaming-output-csv "$STREAM_CSV" ); fi

echo -e "${BLUE}Command:${NC} ${CMD[*]}"
echo

# Run the actual benchmark
"${CMD[@]}"

if [ $? -eq 0 ]; then
    print_success "Benchmark completed successfully!"
    print_status "Results saved to: ${OUTPUT_FILE}"
    
    # Show summary if file exists
    if [ -f "${OUTPUT_FILE}" ]; then
        echo
        print_status "Performance Summary:"
        # Prefer jq if available for a more accurate summary
        if command -v jq >/dev/null 2>&1; then
            jq '{throughput, latency_ns, totals} | with_entries(select(.value != null))' "${OUTPUT_FILE}" | sed -n '1,40p'
        else
            echo -e "${GREEN}$(grep -E '(throughput|latency|messages_per_second)' "${OUTPUT_FILE}" | head -5)${NC}"
        fi
    else
        print_warning "Expected results file not found: ${OUTPUT_FILE}"
    fi
else
    print_error "Benchmark failed!"
    exit 1
fi

echo
print_success "Host client execution complete!"
print_status "Check ${OUTPUT_DIR}/ for detailed results"
