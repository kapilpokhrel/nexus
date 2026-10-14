#!/usr/bin/env bash
#
# Syscalls per request, for the steady-state request path.
#
#   benches/syscalls.sh nexus
#   benches/syscalls.sh nginx
#   benches/syscalls.sh both

set -u

DURATION=${DURATION:-10s}
THREADS=${THREADS:-8}
CONNS=${CONNS:-100}
MIN=${MIN:-0.01}          # hide syscalls below this many per request

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SCRIPT=$ROOT/benches/wrk_script.lua
OUT=/tmp/nexus-syscalls

die() { echo "error: $*" >&2; exit 1; }

need() { command -v "$1" >/dev/null || die "$1 is not installed"; }
need strace; need wrk; need ss

measure() {                       # measure <label> <port> <cmd...>
    local label=$1 port=$2; shift 2
    mkdir -p "$OUT"
    local trace=$OUT/$label.strace

    rm -f "$trace"
    setsid strace -c -f -o "$trace" "$@" >"$OUT/$label.server.log" 2>&1 &

    local i
    for i in $(seq 40); do
        ss -ltn | grep -q ":$port " && break
        sleep 0.25
    done
    ss -ltn | grep -q ":$port " || { cat "$OUT/$label.server.log"; die "$label never listened on :$port"; }
    sleep 1

    local out reqs
    out=$(wrk -t"$THREADS" -c"$CONNS" -d"$DURATION" -s "$SCRIPT" "http://127.0.0.1:$port" 2>&1)
    reqs=$(echo "$out" | grep -oP '[0-9]+(?= requests in)')

    pkill -x nginx 2>/dev/null
    pkill -x nexus 2>/dev/null
    sleep 2

    [ -n "${reqs:-}" ] || die "wrk reported no requests for $label"
    [ -s "$trace" ]    || die "no strace output for $label"

    printf '\n%s — %s total requests\n' "$label" "$reqs"
    awk -v n="$reqs" -v min="$MIN" '
        $NF ~ /^[a-z_0-9]+$/ && $NF != "total" && $4+0 > 0 { c[$NF] += $4 }
        END { for (s in c) if (c[s]/n >= min) printf "   %-18s %6.2f /req\n", s, c[s]/n }
    ' "$trace" | sort -k2 -rn
    awk -v n="$reqs" '
        $NF ~ /^[a-z_0-9]+$/ && $NF != "total" && $4+0 > 0 { t += $4 }
        END { printf "   %-18s %6.2f /req\n", "TOTAL", t/n }' "$trace"
}

nexus() {
    [ -x "$ROOT/target/release/nexus" ] || die "build first: cargo build --release"
    measure nexus 5000 "$ROOT/target/release/nexus"
}

nginx_() {
    need nginx
    mkdir -p /tmp/nginx
    measure nginx 6000 nginx -c "$ROOT/nginx/nginx.conf" -p /tmp/nginx -g "daemon off;"
}

case "${1:-both}" in
    nexus)   nexus ;;
    nginx)   nginx_ ;;
    both)    nexus; nginx_ ;;
    *) die "usage: $0 [nexus|nginx|both]" ;;
esac

printf '\ntraces kept in %s\n' "$OUT"
