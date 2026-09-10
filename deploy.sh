#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="steve-assist"
RELEASE="steve-assist"
CHART="helm/steve-assist"
TAG="${1:-latest}"
ENV_FILE="${2:-.env.prod}"

if [ ! -f "$ENV_FILE" ]; then
  echo "Error: ${ENV_FILE} not found"
  exit 1
fi

echo "==> Creating namespace ${NAMESPACE} (if not exists)..."
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -

echo "==> Creating ConfigMap from ${ENV_FILE}..."
kubectl create configmap steve-assist-env \
  --from-file=.env="$ENV_FILE" \
  --namespace "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

# Compute checksum so pods restart when .env changes
ENV_CHECKSUM=$(sha256sum "$ENV_FILE" | cut -d' ' -f1)

echo "==> Deploying Helm chart..."
helm upgrade --install "$RELEASE" "$CHART" \
  --namespace "$NAMESPACE" \
  --set "envConfigChecksum=${ENV_CHECKSUM}" \
  --set "image.tag=${TAG}"

echo "==> Done. Pods:"
kubectl get pods -n "$NAMESPACE"
