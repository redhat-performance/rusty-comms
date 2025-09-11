#!/bin/bash

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

CONTAINER_NAME="rusty-comms-pmq-server"
IMAGE_TAG="rusty-comms:latest"

msg() { echo -e "${BLUE}[INFO]${NC} $*"; }
ok() { echo -e "${GREEN}[OK]${NC} $*"; }
err() { echo -e "${RED}[ERR]${NC} $*"; }

# Optional knobs
PMQ_MSG_COUNT=${PMQ_MSG_COUNT:-1000}
PMQ_MSG_SIZE=${PMQ_MSG_SIZE:-1024}

msg "Building image ${IMAGE_TAG}..."
podman build -t "${IMAGE_TAG}" .
ok "Image built"

msg "Ensuring output directory exists..."
mkdir -p ./output

msg "Removing any existing ${CONTAINER_NAME}..."
podman rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true

msg "Starting PMQ server container with host IPC namespace..."
podman run -d --name "${CONTAINER_NAME}" \
  --ipc=host \
  --security-opt label=disable \
  --user 0:0 \
  -v "$(pwd)/output:/app/output:z" \
  -e RUST_LOG=warn \
  -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
  "${IMAGE_TAG}" sh -c "\
    umask 000; mkdir -p ./output; \
    echo 'Starting PMQ server (client mode)...'; \
    ipc-benchmark -m pmq --mode client --msg-count ${PMQ_MSG_COUNT} --message-size ${PMQ_MSG_SIZE} \
      --output-file ./output/container_server_results.json"

ok "PMQ server container started"

msg "IPC mode: $(podman inspect ${CONTAINER_NAME} --format '{{.HostConfig.IpcMode}}')"
msg "Recent logs:"
podman logs "${CONTAINER_NAME}" | tail -n 20 | sed 's/^/  /'

echo
ok "To stop: podman rm -f ${CONTAINER_NAME}"
ok "To view logs: podman logs -f ${CONTAINER_NAME}"


