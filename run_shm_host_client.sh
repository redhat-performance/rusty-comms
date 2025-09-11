#!/bin/bash

set -euo pipefail

BLUE='\033[0;34m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

MSG_COUNT=${1:-1000}
MSG_SIZE=${2:-1024}
WORKERS=${3:-1}
SHM_NAME=${SHM_NAME:-ipc_benchmark_shm_crossenv}
BUFFER_SIZE=${BUFFER_SIZE:-65536}

OUTPUT_DIR="./output"
OUTPUT_FILE=${OUTPUT_FILE:-"${OUTPUT_DIR}/host_client_results.json"}

echo -e "${BLUE}=== SHM Host Client ===${NC}"
echo -e "Messages: ${GREEN}${MSG_COUNT}${NC}"
echo -e "Message Size: ${GREEN}${MSG_SIZE}${NC} bytes"
echo -e "Workers: ${GREEN}${WORKERS}${NC}"
echo -e "SHM Name: ${GREEN}${SHM_NAME}${NC}"
echo -e "Buffer Size: ${GREEN}${BUFFER_SIZE}${NC}"
echo -e "Output: ${GREEN}${OUTPUT_FILE}${NC}"

mkdir -p "${OUTPUT_DIR}"

if [ ! -f "./target/release/ipc-benchmark" ]; then
  echo -e "${BLUE}[INFO]${NC} Building..."
  cargo build --release
fi

export RUST_LOG=info
export IPC_BENCHMARK_OUTPUT_DIR="${OUTPUT_DIR}"

EXTRA_OPTS=()
if [ -n "${STREAM_JSON:-}" ]; then EXTRA_OPTS+=( --streaming-output-json "${STREAM_JSON}" ); fi
if [ -n "${STREAM_CSV:-}" ]; then EXTRA_OPTS+=( --streaming-output-csv "${STREAM_CSV}" ); fi

./target/release/ipc-benchmark \
  --mode host \
  --message-size ${MSG_SIZE} \
  --concurrency ${WORKERS} \
  -m shm \
  --shm-name "${SHM_NAME}" \
  --buffer-size ${BUFFER_SIZE} \
  --msg-count ${MSG_COUNT} \
  --output-file "${OUTPUT_FILE}" \
  "${EXTRA_OPTS[@]}"

status=$?
if [ $status -eq 0 ]; then
  echo -e "${GREEN}[SUCCESS]${NC} Host SHM benchmark completed"
  echo -e "${BLUE}[INFO]${NC} Results: ${OUTPUT_FILE}"
  if command -v jq >/dev/null 2>&1; then
    echo -e "${BLUE}[INFO]${NC} Summary:"; jq '{mechanism, totals, throughput, latency_ns} | with_entries(select(.value != null))' "${OUTPUT_FILE}" | sed -n '1,60p'
  else
    echo -e "${BLUE}[INFO]${NC} Summary (grep):"; grep -E 'mechanism|messages_per_second|bytes_transferred|latency' -n "${OUTPUT_FILE}" | sed -n '1,40p'
  fi
else
  echo -e "${RED}[ERROR]${NC} Host SHM benchmark failed"
fi





