#!/usr/bin/env bash
# Measure Sift cold-start phases end-to-end.
#
# Runs the release binary with SIFT_STARTUP_TRACE=1, waits for the app to be
# fully up, then kills it and prints the trace. The first line reported by the
# process (`origin-wall-ms`) is its wall clock at the first Rust instruction;
# subtracting the launcher's spawn timestamp gives dyld/pre-main time.
#
# Usage: scripts/measure-startup.sh [runs]
set -euo pipefail

BIN="${SIFT_BIN:-/Users/xiaocy/Documents/fancy2110/sift/target/release/sift}"
RUNS="${1:-3}"
HOLD="${HOLD:-5}"

for run in $(seq 1 "$RUNS"); do
  LOG="/tmp/sift-startup-run-${run}.log"
  SPAWN_MS=$(python3 -c 'import time; print(int(time.time()*1000))')
  SIFT_STARTUP_TRACE=1 "$BIN" >"$LOG" 2>&1 &
  PID=$!
  sleep "$HOLD"
  kill "$PID" 2>/dev/null || true
  wait "$PID" 2>/dev/null || true

  echo "=== run $run ==="
  ORIGIN=$(grep -oE 'origin-wall-ms=[0-9]+' "$LOG" | cut -d= -f2 || true)
  if [ -n "${ORIGIN:-}" ]; then
    echo "pre-main (spawn -> first Rust line): $((ORIGIN - SPAWN_MS)) ms"
  fi
  grep '\[startup\]' "$LOG" | grep -v origin-wall-ms
done
