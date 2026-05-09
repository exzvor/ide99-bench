# ide99-bench

Reproducible performance benchmarks for [ide99](https://ide99.ru) — the desktop
PostgreSQL IDE built by the SPG99 team. Every number on the
[ide99.ru](https://ide99.ru) **Speed** section can be regenerated with the
scripts in this repo against a fresh PostgreSQL container.

The point of this repo is **transparency**: anyone can pull the scripts, point
them at a Postgres they trust, run them on their own hardware, and see the same
numbers (within the noise of their machine).

## What we measure

| Metric                | What it captures                                                            | Script                                  |
| --------------------- | --------------------------------------------------------------------------- | --------------------------------------- |
| Boot to ready         | Wall-clock from `ide99` process spawn to the `READY` handshake on stdout    | `scripts/boot_and_idle.py`              |
| Idle RSS              | Sum of resident set size across the ide99 process tree, 4 s after `READY`   | `scripts/boot_and_idle.py`              |
| First page (10M rows) | TCP query → tokio-postgres decode → `Vec<Vec<Value>>` → JSON serialise      | `scripts/read_first_page/`              |
| Grid scroll @ 50M     | Per-frame compute for the virtualised result grid scrolling a 50M-row table | `scripts/grid_scroll.mjs`               |
| Result-grid format    | `query_result_to_json` over 1k / 10k / 100k row pages                       | `ide/src-tauri/benches/result_grid_format.rs` (criterion) |
| SQL parser            | DDL / SELECT parse latency budget for the autocomplete inner loop           | `ide/src-tauri/benches/parser_bench.rs` (criterion) |
| Autocomplete          | Parse + scope walk for a 64-table JOIN — adversarial worst case             | `ide/src-tauri/benches/autocomplete_latency.rs` (criterion) |
| EXPLAIN render        | Parse + insights walk for a 500-node real-world EXPLAIN JSON                | `ide/src-tauri/benches/explain_render.rs` (criterion) |

The Rust criterion benches live in the main ide99 repo and run on every CI
build. The integration benches (boot, idle RSS, first page, grid) live here
because they need a real ide99 binary and a real Postgres.

## Reproducing the numbers

### 1. Spin up the bench Postgres

```bash
docker run -d --name ide99-bench-pg \
  -p 55433:5432 \
  -e POSTGRES_PASSWORD=bench \
  -e POSTGRES_DB=bench \
  -e POSTGRES_USER=bench \
  --shm-size=512m \
  postgres:17-alpine

PGPASSWORD=bench psql -h 127.0.0.1 -p 55433 -U bench -d bench \
  -v ON_ERROR_STOP=1 -f scripts/seed.sql
```

This creates `public.events_10m` (10,000,000 rows, ~2.6 GB on disk, mixed
types incl. `jsonb`) and `public.lookup_1k`. Seed time on Apple silicon is
~30 s. The fixture is `UNLOGGED` — we don't need crash safety in a throwaway
bench DB and it cuts seed time roughly in half.

### 2. Build the ide99 release binary

```bash
cd ../ide
cargo tauri build --release --no-bundle
# binary lands in target/release/ide99
```

The `--no-bundle` flag skips DMG/AppImage assembly — we only need the
executable. Set `IDE99_BINARY` if you put the binary somewhere else.

### 3. Run the integration benches

```bash
# from this repo's root
./scripts/run-all.sh
```

Or run the individual benches:

```bash
# boot + idle RSS (12 + 8 iterations)
python3 scripts/boot_and_idle.py > results/boot_and_idle.json

# first-page reads against the seeded DB (3 scenarios × 30 iters each)
(cd scripts/read_first_page && cargo build --release)
./scripts/read_first_page/target/release/read_first_page > results/read_first_page.json

# virtualised grid scroll over 50M rows (600 frames × 88-row window)
node scripts/grid_scroll.mjs > results/grid_scroll.json
```

### 4. Read the report

`reports/REPORT.md` is the human-readable summary of the latest run, with
medians, p95s, hardware notes and how each number maps to a claim on
[ide99.ru](https://ide99.ru).

## Methodology notes

- **Median over mean.** Every bench runs ≥10 iterations and reports the
  median; means and stdev are included for completeness.
- **One untimed warm-up** before each measured loop to absorb cold filesystem
  cache, JIT compilation, and tokio-postgres's first-request schema fetch.
- **Honest scope.** The first-page bench measures the same code path the IDE
  uses (Rust → tokio-postgres → JSON). We do **not** include the Tauri IPC
  frame or the React render in that number, because those are bench-able
  separately (`grid_scroll.mjs`) and lumping them together hides where time
  goes. The README flags every stage we leave out.
- **Worst-case Postgres.** The 10M-row fixture has wide rows: bigint, tstz,
  ints, numeric, enum-text, jsonb (with nested arrays + objects), and a
  long sparse `note`. JSON-serialised payload is ~160 KB per 1k-row page —
  representative of a real analytics workload, not a synthetic narrow table.

## Hardware

The numbers committed to `results/` were captured on:

- **Apple M-series, macOS 24.6.0** (Darwin), 16 GB RAM
- PostgreSQL 17.9 in Docker (`postgres:17-alpine`) on the same host
- ide99 release build, `cargo tauri build --release --no-bundle`

Re-run on your hardware: numbers should land within ±2× on consumer CPUs.
PRs welcome with results from other platforms.

## Layout

```
ide99-bench/
├── README.md                       — this file
├── scripts/
│   ├── seed.sql                    — Postgres fixture (10M rows + indexes)
│   ├── boot_and_idle.py            — Tauri spawn + RSS sampler
│   ├── grid_scroll.mjs             — virtualised grid frame-time bench
│   ├── read_first_page/            — Rust harness for the read path
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   └── run-all.sh                  — runs everything end-to-end
├── results/                        — JSON outputs from the latest run
│   ├── boot_and_idle.json
│   ├── read_first_page.json
│   ├── grid_scroll.json
│   └── criterion_summary.json
└── reports/
    └── REPORT.md                   — human-readable summary
```

## License

MIT. Use it, fork it, send PRs.
