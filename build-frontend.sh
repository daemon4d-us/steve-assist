#!/usr/bin/env bash
set -euo pipefail

PROJECT_ID="${GCP_PROJECT_ID:-steve-assist-491304}"
REGION="${GCP_REGION:-us-central1}"
REPO="steve-assist"
IMAGE="steve-assist-frontend"
TAG="${1:-latest}"

REGISTRY="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO}"
FULL_IMAGE="${REGISTRY}/${IMAGE}:${TAG}"

VITE_GOOGLE_CLIENT_ID="${VITE_GOOGLE_CLIENT_ID:-546677409941-emsag4m88tlrb89epl6799apc06kq4ep.apps.googleusercontent.com}"
VITE_API_BASE_URL="${VITE_API_BASE_URL:-https://steve.creativecaptains.com}"

echo "==> Configuring Docker for Artifact Registry..."
gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet

echo "==> Building ${FULL_IMAGE}..."
docker build \
  --build-arg "VITE_GOOGLE_CLIENT_ID=${VITE_GOOGLE_CLIENT_ID}" \
  --build-arg "VITE_API_BASE_URL=${VITE_API_BASE_URL}" \
  -t "${FULL_IMAGE}" \
  frontend/

echo "==> Pushing ${FULL_IMAGE}..."
docker push "${FULL_IMAGE}"

echo "==> Done: ${FULL_IMAGE}"
