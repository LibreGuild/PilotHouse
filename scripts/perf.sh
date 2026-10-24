#!/usr/bin/env bash
# Measures PilotHouse against the Phase 1 performance budget (docs/performance.md):
# binary size, cold start to interactive, idle RAM and idle CPU.
#
# Usage: scripts/perf.sh [path-to-binary] [project-folder]
# Defaults: target/release/pilothouse and this repository.
# Exits non-zero when a budget is exceeded, so CI can run it.
set -euo pipefail

BIN="${1:-target/release/pilothouse}"
PROJECT="${2:-$(pwd)}"
PROFILE="$(mktemp -d)"
export PILOTHOUSE_DATA_DIR="$PROFILE"

# Budgets (see docs/performance.md).
MAX_BINARY_MB=50
MAX_START_MS=500
MAX_IDLE_RSS_MB=150
MAX_IDLE_CPU=1.0

cleanup() { [[ -n "${PID:-}" ]] && kill "$PID" 2>/dev/null || true; rm -rf "$PROFILE"; }
trap cleanup EXIT

[[ -x "$BIN" ]] || { echo "binary not found: $BIN (build with: cargo build --release -p pilothouse)"; exit 2; }

binary_mb=$(( $(wc -c < "$BIN") / 1024 / 1024 ))

# Register the project first so the app opens straight into it.
sqlite3 "$PROFILE/pilothouse.db" "select 1" >/dev/null 2>&1 || true

start_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
"$BIN" >"$PROFILE/app.log" 2>&1 &
PID=$!
until "$BIN" ctl app.ping >/dev/null 2>&1; do
  sleep 0.01
  kill -0 "$PID" 2>/dev/null || { echo "app exited during startup:"; cat "$PROFILE/app.log"; exit 1; }
done
end_ns=$(python3 -c 'import time; print(time.monotonic_ns())')
start_ms=$(( (end_ns - start_ns) / 1000000 ))

# Let startup work settle, then sample while idle.
sleep 3
rss_mb=$(( $(ps -o rss= -p "$PID") / 1024 ))
if [[ "$(uname)" == "Darwin" ]]; then
  idle_cpu=$(top -l 2 -s 5 -pid "$PID" -stats cpu | tail -1 | tr -d ' ')
else
  idle_cpu=$(top -b -n 2 -d 5 -p "$PID" | awk -v pid="$PID" '$1==pid {cpu=$9} END {print cpu}')
fi

status=0
check() { # name value budget unit
  if awk "BEGIN {exit !($2 <= $3)}"; then mark="ok  "; else mark="FAIL"; status=1; fi
  printf '%s %-22s %8s %-4s (budget %s %s)\n' "$mark" "$1" "$2" "$4" "$3" "$4"
}
echo "PilotHouse performance ($(uname -sm))"
check "binary size" "$binary_mb" "$MAX_BINARY_MB" "MB"
check "start to interactive" "$start_ms" "$MAX_START_MS" "ms"
check "idle RAM" "$rss_mb" "$MAX_IDLE_RSS_MB" "MB"
check "idle CPU" "$idle_cpu" "$MAX_IDLE_CPU" "%"
exit $status
