#!/usr/bin/env bash
set -euo pipefail

echo "Configuring git to use repository-managed hooks..."
git config core.hooksPath .githooks

# Ensure hook is executable (script may be run via 'bash', so exec bit here is optional)
chmod +x .githooks/pre-commit || true

echo "Repo hooks enabled. The following checks will run on commit:"
echo " - cargo check"
echo " - cargo clippy --all-targets --all-features -- -D warnings"
echo " - cargo fmt -- --check"
echo " - cargo test"
echo " - scripts/msrv-check.sh (Rust 1.70 in container, if podman/docker available)"


