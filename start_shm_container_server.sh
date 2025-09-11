#!/bin/bash

#SHM_NAME=ipc_benchmark_shm_crossenv ./start_shm_container_server.sh
#SHM_NAME=ipc_benchmark_shm_crossenv BUFFER_SIZE=131072 ./start_shm_container_server.sh

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

CONTAINER_NAME="rusty-comms-shm-server"
IMAGE_TAG="rusty-comms:latest"

msg() { echo -e "${BLUE}[INFO]${NC} $*"; }
ok() { echo -e "${GREEN}[OK]${NC} $*"; }
err() { echo -e "${RED}[ERR]${NC} $*"; }

SHM_NAME=${SHM_NAME:-ipc_benchmark_shm_crossenv}
SHM_MSG_COUNT=${SHM_MSG_COUNT:-1000}
SHM_MSG_SIZE=${SHM_MSG_SIZE:-1024}
BUFFER_SIZE=${BUFFER_SIZE:-65536}

msg "Building image ${IMAGE_TAG}..."
podman build -t "${IMAGE_TAG}" .
ok "Image built"

msg "Ensuring output directory exists..."
mkdir -p ./output

msg "Removing any existing ${CONTAINER_NAME}..."
podman rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true

msg "Starting SHM server container with host IPC namespace..."
podman run -d --name "${CONTAINER_NAME}" \
  --ipc=host \
  --security-opt label=disable \
  --user 0:0 \
  -v "$(pwd)/output:/app/output:z" \
  -e RUST_LOG=warn \
  -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
  "${IMAGE_TAG}" sh -c "\
    umask 000; mkdir -p ./output; \
    echo 'Starting SHM server (client mode)...'; \
    # Best-effort cleanup of stale segment before starting
    rm -f /dev/shm/${SHM_NAME} 2>/dev/null || true; \
    ipc-benchmark -m shm --mode client --shm-name '${SHM_NAME}' \
      --msg-count ${SHM_MSG_COUNT} --message-size ${SHM_MSG_SIZE} \
      --buffer-size ${BUFFER_SIZE} \
      --output-file ./output/container_server_results.json"

ok "SHM server container started"

msg "IPC mode: $(podman inspect ${CONTAINER_NAME} --format '{{.HostConfig.IpcMode}}')"
msg "Recent logs:"
podman logs "${CONTAINER_NAME}" | tail -n 20 | sed 's/^/  /'
