//! ide99-bench / read_first_page
//!
//! Mirrors the IDE's read-path for opening a large table:
//!   1. open a tokio-postgres connection (the same client crate ide99 uses
//!      under the hood for its connection pool)
//!   2. run a "first page" query
//!   3. decode every column into an owned `serde_json::Value` (same shape
//!      the IDE serialises into the result-grid IPC payload)
//!   4. serialise the resulting `Vec<Vec<Value>>` to JSON to capture the
//!      full Rust→JSON cost (matches the IDE's `query_result_to_json`
//!      path benched in `result_grid_format`)
//!
//! Three scenarios run, each 30 timed iterations after one warm-up:
//!   A. limit_1000_natural  — `SELECT * FROM events_10m LIMIT 1000`
//!                            (no ORDER BY: planner picks the cheapest plan)
//!   B. limit_1000_pk       — `SELECT * FROM events_10m ORDER BY id LIMIT 1000`
//!                            (IDE's stable paginated mode)
//!   C. range_scan_indexed  — `SELECT * FROM events_10m
//!                              WHERE created_at > now() - interval '7 days'
//!                              ORDER BY created_at DESC LIMIT 1000`
//!                            (typical "recent rows" query — index on created_at)
//!
//! What we do NOT measure:
//!   - Tauri IPC frame
//!   - JS-side virtualised render
//! Both add a small constant overhead (~5–25ms on Apple silicon) on top of
//! the numbers below.

use std::env;
use std::time::Instant;

use serde_json::{json, Value};
use tokio_postgres::types::Type;
use tokio_postgres::{Column, NoTls, Row};

const DEFAULT_ITERATIONS: usize = 30;

fn pct(values: &[u128], p: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u128> = values.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn cell_to_json(row: &Row, idx: usize, col: &Column) -> Value {
    let ty = col.type_();
    match *ty {
        Type::INT2 => row.get::<_, Option<i16>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::INT4 => row.get::<_, Option<i32>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::INT8 => row.get::<_, Option<i64>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::FLOAT4 => row.get::<_, Option<f32>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::FLOAT8 => row.get::<_, Option<f64>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::BOOL => row.get::<_, Option<bool>>(idx).map_or(Value::Null, |v| json!(v)),
        Type::TEXT | Type::VARCHAR | Type::NAME | Type::CHAR | Type::BPCHAR => row
            .get::<_, Option<String>>(idx)
            .map_or(Value::Null, Value::String),
        Type::JSON | Type::JSONB => row
            .get::<_, Option<Value>>(idx)
            .unwrap_or(Value::Null),
        Type::TIMESTAMPTZ | Type::TIMESTAMP => row
            .get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx)
            .map_or(Value::Null, |v| Value::String(v.to_rfc3339())),
        Type::NUMERIC => row
            .get::<_, Option<rust_decimal::Decimal>>(idx)
            .map_or(Value::Null, |v| Value::String(v.to_string())),
        _ => Value::String(format!("<unsupported type {}>", ty.name())),
    }
}

fn project(rows: &[Row]) -> Vec<Vec<Value>> {
    rows.iter()
        .map(|row| {
            let cols = row.columns();
            (0..cols.len())
                .map(|i| cell_to_json(row, i, &cols[i]))
                .collect()
        })
        .collect()
}

struct Scenario {
    name: &'static str,
    sql: &'static str,
}

async fn run_scenario(
    client: &tokio_postgres::Client,
    s: &Scenario,
    iters: usize,
) -> Result<Value, Box<dyn std::error::Error>> {
    let stmt = client.prepare(s.sql).await?;

    {
        let rows = client.query(&stmt, &[]).await?;
        let _ = project(&rows);
    }

    let mut samples_ns: Vec<u128> = Vec::with_capacity(iters);
    let mut last_byte_count = 0usize;
    let mut last_row_count = 0usize;

    for _ in 0..iters {
        let t0 = Instant::now();
        let rows = client.query(&stmt, &[]).await?;
        let grid: Vec<Vec<Value>> = project(&rows);
        let payload = serde_json::to_vec(&grid)?;
        samples_ns.push(t0.elapsed().as_nanos());
        last_byte_count = payload.len();
        last_row_count = grid.len();
    }

    let min_ns = *samples_ns.iter().min().unwrap();
    let max_ns = *samples_ns.iter().max().unwrap();
    let mean_ns: u128 = samples_ns.iter().sum::<u128>() / samples_ns.len() as u128;
    let median_ns = pct(&samples_ns, 0.5);
    let p95_ns = pct(&samples_ns, 0.95);

    Ok(json!({
        "name": s.name,
        "sql": s.sql,
        "iterations": iters,
        "rows_per_iter": last_row_count,
        "json_payload_bytes": last_byte_count,
        "ms": {
            "min": min_ns as f64 / 1e6,
            "median": median_ns as f64 / 1e6,
            "p95": p95_ns as f64 / 1e6,
            "max": max_ns as f64 / 1e6,
            "mean": mean_ns as f64 / 1e6,
        },
        "ns": {
            "min": min_ns,
            "median": median_ns,
            "p95": p95_ns,
            "max": max_ns,
            "mean": mean_ns,
        },
        "samples_ns": samples_ns,
    }))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iters: usize = env::var("BENCH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS);
    let conn_string = env::var("BENCH_DB_URL").unwrap_or_else(|_| {
        "host=127.0.0.1 port=55433 user=bench password=bench dbname=bench".into()
    });

    eprintln!("# read_first_page");
    eprintln!("# iters     : {iters}");
    eprintln!("# conn      : {conn_string}");

    let (client, conn) = tokio_postgres::connect(&conn_string, NoTls).await?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("connection error: {e}");
        }
    });

    let scenarios = [
        Scenario {
            name: "limit_1000_natural",
            sql: "SELECT id, created_at, user_id, lookup_id, amount, status, payload, note \
                  FROM public.events_10m LIMIT 1000",
        },
        Scenario {
            name: "limit_1000_pk",
            sql: "SELECT id, created_at, user_id, lookup_id, amount, status, payload, note \
                  FROM public.events_10m ORDER BY id LIMIT 1000",
        },
        Scenario {
            name: "range_scan_indexed",
            sql: "SELECT id, created_at, user_id, lookup_id, amount, status, payload, note \
                  FROM public.events_10m \
                  WHERE created_at > now() - interval '7 days' \
                  ORDER BY created_at DESC LIMIT 1000",
        },
    ];

    let mut results = Vec::new();
    for s in &scenarios {
        eprintln!("# scenario : {}", s.name);
        results.push(run_scenario(&client, s, iters).await?);
    }

    let report = json!({
        "bench": "read_first_page",
        "scenarios": results,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
