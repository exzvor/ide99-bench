#!/usr/bin/env bash
# scripts/run-all.sh — full benchmark pass.
#
# Assumes:
#   - docker container `ide99-bench-pg` is running on 127.0.0.1:55433 with
#     the seed.sql fixture already loaded
#   - ../ide is checked out next to this repo and `cargo tauri build --release
#     --no-bundle` has produced ../ide/target/release/ide99
#
# Override either with env:
#   BENCH_DB_URL=...      tokio-postgres connection string
#   IDE99_BINARY=/path    explicit path to the ide99 binary

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RESULTS="$ROOT/results"
mkdir -p "$RESULTS"

echo "==> [1/3] boot + idle RSS"
python3 "$ROOT/scripts/boot_and_idle.py" \
  > "$RESULTS/boot_and_idle.json" 2> "$RESULTS/boot_and_idle.log"

echo "==> [2/3] first-page reads"
(cd "$ROOT/scripts/read_first_page" && cargo build --release --quiet)
"$ROOT/scripts/read_first_page/target/release/read_first_page" \
  > "$RESULTS/read_first_page.json" 2> "$RESULTS/read_first_page.log"

echo "==> [3/3] virtualised grid scroll"
node "$ROOT/scripts/grid_scroll.mjs" > "$RESULTS/grid_scroll.json"

echo
echo "==> Summary"
python3 - "$RESULTS" <<'PY'
import json, sys, pathlib
results = pathlib.Path(sys.argv[1])

bi = json.loads((results / "boot_and_idle.json").read_text())
print(f"  boot      median = {bi['boot']['median']:7.1f} ms   (n={bi['boot']['n']}, p95={bi['boot']['p95']:.1f} ms)")
print(f"  idle RSS  median = {bi['idle_rss']['median']:7.1f} MB   (n={bi['idle_rss']['n']}, p95={bi['idle_rss']['p95']:.1f} MB)")

rp = json.loads((results / "read_first_page.json").read_text())
for s in rp["scenarios"]:
    print(f"  read [{s['name']:21s}] median = {s['ms']['median']:7.2f} ms   (rows={s['rows_per_iter']}, payload={s['json_payload_bytes']} B)")

gs = json.loads((results / "grid_scroll.json").read_text())
m = gs['ms_per_frame']
print(f"  grid 50M  median = {m['median']:7.4f} ms/frame  ({gs['fps']['capped_at_60']:.0f} fps, dropped={gs['dropped_frames_count']}/{gs['frames']})")
PY
