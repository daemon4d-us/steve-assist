#!/usr/bin/env bash
set -euo pipefail

TAG="${1:-latest}"

echo "==> Building backend..."
./build.sh "$TAG"

echo "==> Building frontend..."
./build-frontend.sh "$TAG"

echo "==> All builds complete."
