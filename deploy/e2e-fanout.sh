#!/usr/bin/env bash
# Real-topology fan-out check: gateway + two daemon workers over
# docker compose, exercising push -> materialize -> index -> merge ->
# degrade — the path the stub-based e2e (research/e2e_gateway.py) mocks.
#
#   deploy/e2e-fanout.sh            # full run (build + up + assert + down)
#   KEEP_UP=1 deploy/e2e-fanout.sh  # leave the stack running afterwards
#
# Requires: docker compose, cargo (for cce-push), python3.
# CCE_DENSE=disabled keeps indexing sparse — no model download.

set -euo pipefail
cd "$(dirname "$0")/.."

GATEWAY=http://127.0.0.1:7735
export CCE_DENSE=${CCE_DENSE:-disabled}

say() { printf '\n== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

say "compose up (multi profile)"
docker compose --profile multi up -d --build

cleanup() {
  if [ "${KEEP_UP:-0}" != "1" ]; then
    docker compose --profile multi down -v >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

say "wait for gateway healthz"
for _ in $(seq 1 90); do
  curl -fsS "$GATEWAY/healthz" >/dev/null 2>&1 && break || sleep 2
done
curl -fsS "$GATEWAY/healthz" >/dev/null || fail "gateway never became healthy"

say "register both workers"
curl -fsS -X POST "$GATEWAY/v1/repos" -H 'content-type: application/json' \
  -d '{"id":"worker","name":"worker","workerUrl":"http://worker:7734"}' >/dev/null \
  || fail "register worker"
curl -fsS -X POST "$GATEWAY/v1/repos" -H 'content-type: application/json' \
  -d '{"id":"worker2","name":"worker2","workerUrl":"http://worker2:7734"}' >/dev/null \
  || fail "register worker2"

say "build + push a fixture repo to each worker"
FIXTURE=$(mktemp -d)
trap 'rm -rf "$FIXTURE"; cleanup' EXIT
cat > "$FIXTURE/zebra.rs" <<'RS'
/// Distinctive symbol used by the fan-out e2e.
pub fn cce_e2e_zebra_marker() -> &'static str {
    "zebra"
}
RS
cargo run -q -p cce-push -- --gateway "$GATEWAY" --repo worker "$FIXTURE" \
  || fail "push to worker"
cargo run -q -p cce-push -- --gateway "$GATEWAY" --repo worker2 "$FIXTURE" \
  || fail "push to worker2"

say "fan-out query"
# Indexing is async after push — poll until both repos report hits or
# the attempt window closes (sparse indexing of a 1-file repo is fast).
MERGED=""
for _ in $(seq 1 60); do
  OUT=$(curl -fsS -X POST "$GATEWAY/v1/search/all" \
    -H 'content-type: application/json' \
    -d '{"query":"cce_e2e_zebra_marker","limit":10}' 2>/dev/null || true)
  MERGED=$(printf '%s' "$OUT" | python3 -c '
import json, sys
try:
    body = json.load(sys.stdin)
except ValueError:
    print("")
    raise SystemExit
repos = sorted({hit.get("repo") for hit in body.get("hits", [])})
print(",".join(repos))
')
  [ "$MERGED" = "worker,worker2" ] && break || sleep 2
done
[ "$MERGED" = "worker,worker2" ] || fail "fanout merge — got repos: '$MERGED'"
echo "merged hits from: $MERGED"

say "degrade: stop worker2"
docker compose --profile multi stop worker2 >/dev/null
OUT=$(curl -fsS -X POST "$GATEWAY/v1/search/all" \
  -H 'content-type: application/json' \
  -d '{"query":"cce_e2e_zebra_marker","limit":10}')
printf '%s' "$OUT" | python3 -c '
import json, sys
body = json.load(sys.stdin)
degraded = {d["repoSlug"] for d in body.get("degraded", [])}
alive = {h.get("repo") for h in body.get("hits", [])}
assert "worker2" in degraded, f"worker2 not degraded: {degraded}"
assert alive == {"worker"}, f"unexpected merged repos: {alive}"
print("worker2 degraded; worker still merged")
'

say "metrics surfaces"
curl -fsS "$GATEWAY/metrics" | grep -q "cce_gateway_fanout_requests_total" \
  || fail "prometheus fanout series missing"
curl -fsS "$GATEWAY/v1/metrics" | python3 -c '
import json, sys
body = json.load(sys.stdin)
assert body["fanout"]["requests"] >= 2, body["fanout"]
assert body["fanout"]["reposDegraded"] >= 1, body["fanout"]
print("fanout counters:", body["fanout"])
'

say "E2E PASS"
