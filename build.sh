#!/usr/bin/env bash
set -euo pipefail

PROJECT_ID="${GCP_PROJECT_ID:-steve-assist-491304}"
REGION="${GCP_REGION:-us-central1}"
REPO="steve-assist"
IMAGE="steve-assist"
TAG="${1:-latest}"

REGISTRY="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO}"
FULL_IMAGE="${REGISTRY}/${IMAGE}:${TAG}"

echo "==> Configuring Docker for Artifact Registry..."
gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet

echo "==> Building ${FULL_IMAGE}..."
docker build -t "${FULL_IMAGE}" .

echo "==> Pushing ${FULL_IMAGE}..."
docker push "${FULL_IMAGE}"

echo "==> Done: ${FULL_IMAGE}"
