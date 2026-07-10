#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

pushd "$SCRIPT_DIR" >/dev/null
python3 backend.py >/dev/null 2>&1 &
PY_PID=$!
popd >/dev/null

cleanup() {
    kill "$PY_PID" >/dev/null 2>&1 || true
    wait "$PY_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

URL="http://127.0.0.1:5000/"
DURATION="10s"
THREADS="12"
CONNS="100"
HOST="Host: example.com"

wrk -t"$THREADS" -c"$CONNS" -d"$DURATION" \
    -H "$HOST" \
    --latency \
    "$URL"
