#!/usr/bin/env bash
set -euo pipefail

NAMESPACE="steve-assist"
RELEASE="steve-assist-frontend"
CHART="helm/frontend"
TAG="${1:-latest}"

echo "==> Deploying Helm chart..."
helm upgrade --install "$RELEASE" "$CHART" \
  --namespace "$NAMESPACE" \
  --set "image.tag=${TAG}"

echo "==> Done. Pods:"
kubectl get pods -n "$NAMESPACE"
