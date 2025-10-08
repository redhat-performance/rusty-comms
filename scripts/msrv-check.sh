#!/usr/bin/env bash
set -euo pipefail

echo "[MSRV] Running Rust 1.70 build/tests in container..."

# Find container runtime
RUNTIME="$(command -v podman || command -v docker || true)"
if [[ -z "${RUNTIME}" ]]; then
  echo "[MSRV] No container runtime (podman/docker) found. Skipping MSRV check."
  exit 0
fi

IMAGE="docker.io/library/rust:1.70"
WORKDIR="/work"
MOUNT="$(pwd):${WORKDIR}"
if [[ "${RUNTIME}" == *podman ]]; then
  MOUNT="${MOUNT}:Z"
fi

CMD='. /usr/local/cargo/env && cargo -V && rustc -V && cargo build --locked -q && cargo test --locked -q'

"${RUNTIME}" run --rm -v "${MOUNT}" -w "${WORKDIR}" "${IMAGE}" bash -lc "${CMD}"

echo "[MSRV] Rust 1.70 build/tests passed."


