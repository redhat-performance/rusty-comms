#!/bin/bash

#podman rm -f rusty-comms-pmq-server 2>/dev/null || true

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

CONTAINER_NAME="rusty-comms-uds-server"
IMAGE_TAG="rusty-comms:latest"
SOCKET_DIR="./sockets"
OUTPUT_DIR="./output"
SOCKET_PATH="${SOCKET_DIR}/ipc_benchmark.sock"

msg() { echo -e "${BLUE}[INFO]${NC} $*"; }
ok() { echo -e "${GREEN}[OK]${NC} $*"; }
err() { echo -e "${RED}[ERR]${NC} $*"; }

UDS_MSG_COUNT=${UDS_MSG_COUNT:-1000}
UDS_MSG_SIZE=${UDS_MSG_SIZE:-1024}

msg "Building image ${IMAGE_TAG}..."
podman build -t "${IMAGE_TAG}" .
ok "Image built"

msg "Preparing directories..."
mkdir -p "${SOCKET_DIR}" "${OUTPUT_DIR}"
rm -f "${SOCKET_PATH}" || true

msg "Removing any existing ${CONTAINER_NAME}..."
podman rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true

msg "Starting UDS server container..."
podman run -d --name "${CONTAINER_NAME}" \
  --security-opt label=disable \
  -v "$(pwd)/sockets:/app/sockets:z" \
  -v "$(pwd)/output:/app/output:z" \
  -e RUST_LOG=warn \
  -e IPC_BENCHMARK_SOCKET_DIR=/app/sockets \
  -e IPC_BENCHMARK_DEFAULT_SOCKET_PATH=/app/sockets/ipc_benchmark.sock \
  -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
  "${IMAGE_TAG}" sh -c "\
    umask 000; mkdir -p ./sockets ./output; rm -f ./sockets/ipc_benchmark.sock || true; \
    echo 'Starting UDS server (client mode)...'; \
    ipc-benchmark -m uds --mode client --ipc-path ./sockets/ipc_benchmark.sock \
      --round-trip \
      --msg-count ${UDS_MSG_COUNT} --message-size ${UDS_MSG_SIZE} \
      --output-file ./output/container_server_results.json"

ok "UDS server container started"

msg "Checking socket availability..."
sleep 2
if [ -S "${SOCKET_PATH}" ]; then
  ok "Socket created at ${SOCKET_PATH}"
else
  echo -e "${YELLOW}[WARN]${NC} Socket not yet available; check logs: podman logs ${CONTAINER_NAME}"
fi

msg "Recent logs:"
podman logs "${CONTAINER_NAME}" | tail -n 20 | sed 's/^/  /'

echo
ok "To stop: podman rm -f ${CONTAINER_NAME}"
ok "To view logs: podman logs -f ${CONTAINER_NAME}"
