#!/bin/bash

# Host-to-Container IPC Benchmark Script
# Supports UDS, SHM, and PMQ mechanisms for cross-environment testing
# Usage: ./run_host_container.sh [mechanism] [messages|duration] [message_size] [workers]

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Default values
DEFAULT_MECHANISM="uds"
DEFAULT_MESSAGES=1000
DEFAULT_MESSAGE_SIZE=1024
DEFAULT_WORKERS=1
DEFAULT_BUFFER_SIZE=65536

# Configuration directories
SOCKET_DIR="./sockets"
OUTPUT_DIR="./output"

# Help function
show_help() {
    cat << EOF
${CYAN}=== Host-to-Container IPC Benchmark Script ===${NC}

${YELLOW}USAGE:${NC}
  ./run_host_container.sh [MECHANISM] [MESSAGES/DURATION] [MESSAGE_SIZE] [WORKERS]

${YELLOW}MECHANISMS:${NC}
  uds    Unix Domain Sockets (default)
  shm    Shared Memory
  pmq    POSIX Message Queues

${YELLOW}PARAMETERS:${NC}
  MESSAGES/DURATION  Number of messages OR duration (e.g., 30s, 5m, 1h)
  MESSAGE_SIZE       Message size in bytes (default: 1024)
  WORKERS           Number of concurrent workers (default: 1)

${YELLOW}ENVIRONMENT VARIABLES:${NC}
  MECHANISM          Override mechanism (uds|shm|pmq)
  DURATION           Test duration (e.g., 30s, 5m, 1h)
  OUTPUT_FILE        Output JSON file path
  STREAM_JSON        Streaming JSON output file
  STREAM_CSV         Streaming CSV output file
  SOCKET_PATH        UDS socket path (default: ./sockets/ipc_benchmark.sock)
  SHM_NAME           Shared memory name (default: ipc_benchmark_shm_crossenv)
  BUFFER_SIZE        Buffer size for SHM (default: 65536)
  ROUND_TRIP         Enable round-trip testing (true/false)
  VERBOSE            Enable verbose output (true/false)

${YELLOW}EXAMPLES:${NC}
  # Basic UDS test with 1000 messages
  ./run_host_container.sh uds 1000 1024 1

  # 30-second duration test with SHM
  ./run_host_container.sh shm 30s 4096 2

  # PMQ test with environment variables
  DURATION=5m OUTPUT_FILE=./results/pmq_test.json ./run_host_container.sh pmq

  # UDS test with custom socket path
  SOCKET_PATH=/tmp/custom.sock ./run_host_container.sh uds 500 2048

  # Round-trip SHM test with streaming output
  ROUND_TRIP=true STREAM_JSON=./live.json ./run_host_container.sh shm 10s 1024

  # Verbose PMQ test
  VERBOSE=true ./run_host_container.sh pmq 2000 512 4

${YELLOW}OUTPUT:${NC}
  Results are saved to JSON files in ./output/ directory
  Use jq for pretty-printing: cat output.json | jq

EOF
}

# Check for help request
if [[ "${1:-}" =~ ^(-h|--help|help)$ ]]; then
    show_help
    exit 0
fi

# Parse parameters with smart detection
MECHANISM="${MECHANISM:-${1:-$DEFAULT_MECHANISM}}"
PARAM2="${2:-$DEFAULT_MESSAGES}"
MESSAGE_SIZE="${3:-$DEFAULT_MESSAGE_SIZE}"
WORKERS="${4:-$DEFAULT_WORKERS}"

# Smart duration/message count detection
if [[ "$PARAM2" =~ ^[0-9]+[smh]?$ ]]; then
    if [[ "$PARAM2" =~ [smh]$ ]]; then
        # Has time unit suffix, treat as duration
        DURATION="${DURATION:-$PARAM2}"
        MESSAGES=""
    else
        # Pure number, could be messages or seconds
        if [ -n "${DURATION:-}" ]; then
            # DURATION env var is set, use that
            MESSAGES=""
        else
            # No DURATION env var, treat as message count
            MESSAGES="$PARAM2"
            DURATION=""
        fi
    fi
else
    # Invalid format, treat as message count
    MESSAGES="$PARAM2"
    DURATION="${DURATION:-}"
fi

# Environment variable overrides
SOCKET_PATH="${SOCKET_PATH:-${SOCKET_DIR}/ipc_benchmark.sock}"
SHM_NAME="${SHM_NAME:-ipc_benchmark_shm_crossenv}"
BUFFER_SIZE="${BUFFER_SIZE:-$DEFAULT_BUFFER_SIZE}"
ROUND_TRIP="${ROUND_TRIP:-false}"
VERBOSE="${VERBOSE:-false}"

# Set output file with mechanism-specific defaults
if [ -z "${OUTPUT_FILE:-}" ]; then
    case "$MECHANISM" in
        uds) OUTPUT_FILE="${OUTPUT_DIR}/host_uds_results.json" ;;
        shm) OUTPUT_FILE="${OUTPUT_DIR}/host_shm_results.json" ;;
        pmq) OUTPUT_FILE="${OUTPUT_DIR}/host_pmq_results.json" ;;
        *) OUTPUT_FILE="${OUTPUT_DIR}/host_${MECHANISM}_results.json" ;;
    esac
fi

# Validate mechanism
case "$MECHANISM" in
    uds|shm|pmq) ;;
    *)
        echo -e "${RED}[ERROR]${NC} Invalid mechanism: $MECHANISM"
        echo -e "${YELLOW}Valid mechanisms: uds, shm, pmq${NC}"
        exit 1
        ;;
esac

# Display configuration
echo -e "${CYAN}=== Host-to-Container IPC Benchmark ===${NC}"
echo -e "Mechanism: ${GREEN}${MECHANISM^^}${NC}"
if [ -n "$DURATION" ]; then
    echo -e "Duration: ${GREEN}${DURATION}${NC}"
else
    echo -e "Messages: ${GREEN}${MESSAGES}${NC}"
fi
echo -e "Message Size: ${GREEN}${MESSAGE_SIZE}${NC} bytes"
echo -e "Workers: ${GREEN}${WORKERS}${NC}"

# Mechanism-specific configuration display
case "$MECHANISM" in
    uds)
        echo -e "Socket Path: ${GREEN}${SOCKET_PATH}${NC}"
        ;;
    shm)
        echo -e "SHM Name: ${GREEN}${SHM_NAME}${NC}"
        echo -e "Buffer Size: ${GREEN}${BUFFER_SIZE}${NC}"
        ;;
    pmq)
        echo -e "Queue Name: ${GREEN}ipc_benchmark_pmq_crossenv${NC}"
        ;;
esac

echo -e "Round Trip: ${GREEN}${ROUND_TRIP}${NC}"
echo -e "Output File: ${GREEN}${OUTPUT_FILE}${NC}"
if [ -n "${STREAM_JSON:-}" ]; then echo -e "Streaming JSON: ${GREEN}${STREAM_JSON}${NC}"; fi
if [ -n "${STREAM_CSV:-}" ]; then echo -e "Streaming CSV: ${GREEN}${STREAM_CSV}${NC}"; fi
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

# Set environment variables
export RUST_LOG=info
export IPC_BENCHMARK_TEMP_DIR=/tmp
export IPC_BENCHMARK_OUTPUT_DIR="${OUTPUT_DIR}"

if [ "$VERBOSE" = "true" ]; then
    export RUST_LOG=debug
fi

# Build command array
CMD=(./target/release/ipc-benchmark --mode host --message-size "${MESSAGE_SIZE}" --concurrency "${WORKERS}")

# Add mechanism-specific parameters
case "$MECHANISM" in
    uds)
        CMD+=(-m uds --ipc-path "${SOCKET_PATH}")
        ;;
    shm)
        CMD+=(-m shm --shm-name "${SHM_NAME}" --buffer-size "${BUFFER_SIZE}")
        ;;
    pmq)
        CMD+=(-m pmq)
        ;;
esac

# Add duration or message count
if [ -n "$DURATION" ]; then
    CMD+=(--duration "$DURATION")
else
    CMD+=(--msg-count "${MESSAGES}")
fi

# Add round-trip if enabled
if [ "$ROUND_TRIP" = "true" ]; then
    CMD+=(--round-trip)
fi

# Add output file
CMD+=(--output-file "${OUTPUT_FILE}")

# Add streaming options
if [ -n "${STREAM_JSON:-}" ]; then
    CMD+=(--streaming-output-json "${STREAM_JSON}")
fi
if [ -n "${STREAM_CSV:-}" ]; then
    CMD+=(--streaming-output-csv "${STREAM_CSV}")
fi

# Display command
echo -e "${BLUE}Command:${NC} ${CMD[*]}"
echo

# Run the benchmark
print_status "Starting IPC benchmark..."
"${CMD[@]}"

# Check results
if [ $? -eq 0 ]; then
    print_success "Benchmark completed successfully!"
    print_status "Results saved to: ${OUTPUT_FILE}"
    
    # Show summary if file exists
    if [ -f "${OUTPUT_FILE}" ]; then
        echo
        print_status "Performance Summary:"
        # Prefer jq if available for better formatting
        if command -v jq >/dev/null 2>&1; then
            jq '{mechanism, totals, throughput, latency_ns} | with_entries(select(.value != null))' "${OUTPUT_FILE}" | sed -n '1,60p'
        else
            print_warning "Install 'jq' for better formatted output"
            echo -e "${GREEN}$(grep -E '(mechanism|throughput|latency|messages_per_second|duration)' "${OUTPUT_FILE}" | head -8)${NC}"
        fi
    else
        print_warning "Expected results file not found: ${OUTPUT_FILE}"
    fi
else
    print_error "Benchmark failed!"
    exit 1
fi

echo
print_success "Benchmark execution complete!"
print_status "Results available in: ${OUTPUT_DIR}/"
if [ -n "${STREAM_JSON:-}" ]; then
    print_status "Streaming results in: ${STREAM_JSON}"
fi
if [ -n "${STREAM_CSV:-}" ]; then
    print_status "Streaming CSV in: ${STREAM_CSV}"
fi
