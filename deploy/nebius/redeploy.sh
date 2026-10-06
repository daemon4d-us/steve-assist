#!/usr/bin/env bash
# Redeploy Steve on the Nebius VM from the current git HEAD.
#
#   deploy/nebius/redeploy.sh              # push main, rebuild what changed
#   deploy/nebius/redeploy.sh --all        # rebuild steve and frontend
#   deploy/nebius/redeploy.sh --no-push    # VM pulls whatever is already on origin
#   deploy/nebius/redeploy.sh --no-env     # leave the VM's /opt/steve/.env alone
#   deploy/nebius/redeploy.sh --status     # just show what is running (no changes)
#
# What it does: pushes origin/main, syncs .env.prod to /opt/steve/.env on the
# VM (the container's env_file; refuses if a key the VM has would go missing),
# then on the VM: git pull, `docker compose up -d --build` for the services
# whose sources changed (steve, frontend), a recreate of steve if only its env
# changed, `caddy reload` if the Caddyfile changed, and finally checks the
# commit the VM is on, the container states, and both public endpoints.
#
# Overrides (env): VM_HOST (ubuntu@204.12.169.20), APP_DIR (/opt/steve/app),
# ENV_FILE (.env.prod), STEVE_URL, DASHBOARD_URL.
set -euo pipefail

VM_HOST="${VM_HOST:-ubuntu@204.12.169.20}"
APP_DIR="${APP_DIR:-/opt/steve/app}"
ENV_FILE="${ENV_FILE:-.env.prod}"
VM_ENV=/opt/steve/.env
STEVE_URL="${STEVE_URL:-https://steve-nb.creativecaptains.com}"
DASHBOARD_URL="${DASHBOARD_URL:-https://dashboard-nb.creativecaptains.com}"
COMPOSE="sudo docker compose -f deploy/nebius/docker-compose.yml"

PUSH=1; ALL=0; STATUS_ONLY=0; SYNC_ENV=1
for arg in "$@"; do
  case "$arg" in
    --no-push) PUSH=0 ;;
    --no-env) SYNC_ENV=0 ;;
    --all) ALL=1 ;;
    --status) STATUS_ONLY=1 ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

step() { printf '\n==> %s\n' "$*"; }
ssh_vm() { ssh -o BatchMode=yes -o ConnectTimeout=15 "$VM_HOST" "$@"; }

verify() {
  step "Verifying"
  ssh_vm "cd $APP_DIR && echo \"VM commit: \$(git log --oneline -1)\" && $COMPOSE ps --format 'table {{.Name}}\t{{.Status}}'"
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$STEVE_URL/incoming-call" -d 'From=%2B1&CallSid=CAprobe' || echo 000)
  printf '%-40s %s\n' "$STEVE_URL/incoming-call" "$code"
  [ "$code" = "200" ] || { echo "!! webhook not healthy" >&2; return 1; }
  code=$(curl -sS -o /dev/null -w '%{http_code}' "$DASHBOARD_URL/" || echo 000)
  printf '%-40s %s\n' "$DASHBOARD_URL/" "$code"
  [ "$code" = "200" ] || { echo "!! dashboard not healthy" >&2; return 1; }
}

if [ "$STATUS_ONLY" = 1 ]; then
  echo "local HEAD: $(git log --oneline -1)"
  verify
  exit 0
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "!! working tree has uncommitted changes; commit first" >&2
  git status --short >&2
  exit 1
fi

if [ "$PUSH" = 1 ]; then
  step "Pushing origin main"
  git push origin main
fi
TARGET=$(git rev-parse HEAD)

# Sync the container env. Only key names are ever printed, never values.
ENV_CHANGED=0
if [ "$SYNC_ENV" = 1 ]; then
  [ -f "$ENV_FILE" ] || { echo "!! $ENV_FILE not found (pass --no-env to skip the env sync)" >&2; exit 1; }
  step "Syncing $ENV_FILE -> $VM_HOST:$VM_ENV"
  scp -q -o BatchMode=yes "$ENV_FILE" "$VM_HOST:$VM_ENV.new"
  ENV_REPORT=$(ssh_vm "bash -s" <<EOF
set -e
keys() { grep -oE '^[A-Za-z_][A-Za-z0-9_]*=' "\$1" | tr -d = | sort; }
if [ -f $VM_ENV ]; then
  missing=\$(comm -23 <(keys $VM_ENV) <(keys $VM_ENV.new) | tr '\n' ' ')
  if [ -n "\$missing" ]; then
    rm -f $VM_ENV.new
    echo "MISSING \$missing"; exit 0
  fi
  if cmp -s $VM_ENV $VM_ENV.new; then rm -f $VM_ENV.new; echo "SAME"; exit 0; fi
  changed=\$(diff <(sort $VM_ENV) <(sort $VM_ENV.new) | sed -nE 's/^[<>] *([A-Za-z_][A-Za-z0-9_]*)=.*/\1/p' | sort -u | tr '\n' ' ')
else
  changed="(new file)"
fi
chmod 600 $VM_ENV.new && mv $VM_ENV.new $VM_ENV
echo "CHANGED \$changed"
EOF
  )
  case "$ENV_REPORT" in
    SAME) echo "env unchanged" ;;
    MISSING*) echo "!! refusing to sync: the VM's env has keys that $ENV_FILE lacks: ${ENV_REPORT#MISSING }" >&2
              echo "   add them to $ENV_FILE, or pass --no-env" >&2; exit 1 ;;
    CHANGED*) echo "env updated, changed keys: ${ENV_REPORT#CHANGED }"; ENV_CHANGED=1 ;;
    *) echo "!! unexpected env sync result: $ENV_REPORT" >&2; exit 1 ;;
  esac
fi

step "Pulling on the VM"
BEFORE=$(ssh_vm "cd $APP_DIR && git rev-parse HEAD")
ssh_vm "cd $APP_DIR && git pull -q --ff-only && git log --oneline -1"
AFTER=$(ssh_vm "cd $APP_DIR && git rev-parse HEAD")
if [ "$AFTER" != "$TARGET" ]; then
  echo "!! VM is at $AFTER but local HEAD is $TARGET (push first, or pass --no-push only when origin is current)" >&2
  exit 1
fi

# Decide what to rebuild from the files that changed between the two commits.
SERVICES=()
if [ "$ALL" = 1 ]; then
  SERVICES=(steve frontend)
elif [ "$BEFORE" = "$AFTER" ]; then
  echo "VM already at $(git log --oneline -1 "$AFTER"); nothing to rebuild"
else
  CHANGED=$(git diff --name-only "$BEFORE" "$AFTER")
  echo "$CHANGED" | grep -qE '^(src/|Cargo\.(toml|lock)|Dockerfile)' && SERVICES+=(steve)
  echo "$CHANGED" | grep -qE '^frontend/' && SERVICES+=(frontend)
  echo "$CHANGED" | grep -qE '^deploy/nebius/docker-compose\.yml' && SERVICES=(steve frontend)
  if echo "$CHANGED" | grep -qE '^deploy/nebius/Caddyfile'; then
    step "Reloading Caddy (Caddyfile changed)"
    ssh_vm "cd $APP_DIR && $COMPOSE exec caddy caddy reload --config /etc/caddy/Caddyfile"
  fi
fi

RESTARTED=0
if [ "${#SERVICES[@]}" -gt 0 ]; then
  step "Rebuilding: ${SERVICES[*]} (Rust builds take a few minutes on the VM)"
  ssh_vm "cd $APP_DIR && $COMPOSE up -d --build ${SERVICES[*]} 2>&1 | grep -E 'Built|Recreate|Started|Error|error' || true"
  RESTARTED=1
fi
if [ "$ENV_CHANGED" = 1 ] && [[ " ${SERVICES[*]-} " != *" steve "* ]]; then
  # env_file is read at container creation, so a changed env needs a recreate.
  step "Recreating steve (env changed)"
  ssh_vm "cd $APP_DIR && $COMPOSE up -d steve 2>&1 | grep -E 'Recreate|Started|Error|error' || true"
  RESTARTED=1
fi

if [ "$RESTARTED" = 1 ]; then
  step "Waiting for the server to come up"
  for _ in $(seq 1 30); do
    if ssh_vm "cd $APP_DIR && $COMPOSE logs --since 3m steve 2>&1 | grep -q 'Server running on port'"; then break; fi
    sleep 2
  done
fi

verify
step "Done"
