#!/usr/bin/env bash
set -euo pipefail

MSRV="$(awk -F '"' '/rust-version/ {print $2}' Cargo.toml)"
if [[ -z "${MSRV}" ]]; then
  echo "[MSRV] No rust-version found in Cargo.toml. Skipping."
  exit 0
fi

echo "[MSRV] Checking Rust ${MSRV} compatibility..."

if ! command -v rustup &>/dev/null; then
  echo "[MSRV] rustup not found. Skipping MSRV check."
  exit 0
fi

if ! rustup toolchain list | grep -q "${MSRV}"; then
  echo "[MSRV] Installing Rust ${MSRV} toolchain..."
  rustup toolchain install "${MSRV}" --profile minimal
fi

echo "[MSRV] Building with Rust ${MSRV}..."
rustup run "${MSRV}" cargo build --locked -q

echo "[MSRV] Testing with Rust ${MSRV}..."
rustup run "${MSRV}" cargo test --locked -q

echo "[MSRV] Rust ${MSRV} build/tests passed."
