#!/usr/bin/env bash
set -uo pipefail

URL="http://127.0.0.1:5000"
DURATION=30s

CONCURRENCY=(
50
75
100
150
200
300
400
500
600
700
)
THREADS=8
mkdir -p results

echo "concurrency,rps,mean_ms,p50_ms,p75_ms,p90_ms,p99_ms,errors" \
    > results/results.csv

for c in "${CONCURRENCY[@]}"
do
    echo "Concurrency $c"
    out=$(
        wrk \
            -t"$THREADS" \
            -c"$c" \
            -d"$DURATION" \
            --latency \
            -s wrk_script.lua \
            "$URL" 2>&1
    )

    summary=$(echo "$out" | grep '^SUMMARY:')

    IFS=',' read -r f1 f2 f3 f4 f5 f6 f7 f8 <<< "$summary"

    rps=$(echo "$summary" | grep -oP 'rps=\K[0-9.]+')
    mean=$(echo "$summary" | grep -oP 'latency_mean=\K[0-9.]+')
    stdev=$(echo "$summary" | grep -oP 'latency_stdev=\K[0-9.]+')
    p50=$(echo "$summary" | grep -oP 'latency_50th=\K[0-9.]+')
    p75=$(echo "$summary" | grep -oP 'latency_75th=\K[0-9.]+')
    p90=$(echo "$summary" | grep -oP 'latency_90th=\K[0-9.]+')
    p99=$(echo "$summary" | grep -oP 'latency_99th=\K[0-9.]+')
    errors=$(echo "$summary" | grep -oP 'total_errors=\K[0-9]+')

    echo "DEBUG: $c $rps $mean $stdev $p50 $p75 $p90 $p99 $errors"
    echo ""
    echo "$c,$rps,$mean,$p50,$p75,$p90,$p99,$errors" \
        >> results/results.csv
done

python3 plot.py

echo
echo "Results written to results/results.csv"
