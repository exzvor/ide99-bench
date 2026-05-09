#!/usr/bin/env python3
"""
ide99-bench / boot_and_idle.py

Drives the ide99 release binary and captures:

  - boot_to_ready_ms : wall-clock from process spawn to the first "READY"
                       line on stdout. The binary prints "READY" once the
                       Tauri main window has finished its on-load handshake
                       (see src-tauri/src/lib.rs `setup` block), so this
                       is the user-visible "click → window is alive"
                       latency.

  - idle_rss_mb      : Resident-set size of the parent process (and its
                       WebView / renderer children) sampled IDLE_SETTLE_S
                       seconds after READY. We sum every descendant PID
                       so cross-process renderers are accounted for, the
                       same way Activity Monitor does it.

Defaults: 12 boot iterations, 8 idle samples (4s settle each), one
graceful kill between runs to avoid OS-level filesystem-cache priming
biasing later iterations toward 0ms.

Outputs JSON on stdout and a structured summary to stderr.
"""

import json
import os
import shlex
import signal
import statistics as stats
import subprocess
import sys
import time
from pathlib import Path
from typing import List

BINARY = os.environ.get(
    "IDE99_BINARY",
    "/Users/exzvor/Desktop/spg99/ide/target/release/ide99",
)
BOOT_ITERATIONS = int(os.environ.get("BOOT_ITERATIONS", "12"))
IDLE_ITERATIONS = int(os.environ.get("IDLE_ITERATIONS", "8"))
IDLE_SETTLE_S = float(os.environ.get("IDLE_SETTLE_S", "4"))
READY_TIMEOUT_S = float(os.environ.get("READY_TIMEOUT_S", "30"))


def descendants(pid: int) -> List[int]:
    """Return [pid, *all_children] using `ps`. macOS-friendly."""
    out = subprocess.check_output(
        ["ps", "-o", "pid,ppid", "-A"], text=True, stderr=subprocess.DEVNULL
    )
    parents = {}
    for line in out.strip().splitlines()[1:]:
        parts = line.split()
        if len(parts) >= 2:
            try:
                cpid, ppid = int(parts[0]), int(parts[1])
                parents.setdefault(ppid, []).append(cpid)
            except ValueError:
                continue

    seen = set([pid])
    queue = [pid]
    while queue:
        x = queue.pop()
        for c in parents.get(x, []):
            if c not in seen:
                seen.add(c)
                queue.append(c)
    return sorted(seen)


def rss_bytes_for(pids: List[int]) -> int:
    if not pids:
        return 0
    out = subprocess.check_output(
        ["ps", "-o", "rss=", "-p", ",".join(str(p) for p in pids)],
        text=True,
        stderr=subprocess.DEVNULL,
    )
    total_kib = sum(int(line.strip()) for line in out.strip().splitlines() if line.strip())
    return total_kib * 1024


def kill_tree(pid: int) -> None:
    pids = descendants(pid)
    for p in reversed(pids):
        try:
            os.kill(p, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        alive = []
        for p in pids:
            try:
                os.kill(p, 0)
                alive.append(p)
            except ProcessLookupError:
                pass
        if not alive:
            return
        time.sleep(0.05)
    for p in pids:
        try:
            os.kill(p, signal.SIGKILL)
        except ProcessLookupError:
            pass


def measure_one(want_idle: bool):
    """Spawn binary, read until READY, capture boot ms, optionally idle RSS."""
    started = time.monotonic()
    proc = subprocess.Popen(
        [BINARY],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        env={**os.environ, "RUST_LOG": "warn"},
    )

    boot_ms = None
    deadline = started + READY_TIMEOUT_S
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"ide99 exited before READY (rc={proc.returncode})")
        line = proc.stdout.readline()
        if not line:
            time.sleep(0.005)
            continue
        if line.strip() == "READY":
            boot_ms = (time.monotonic() - started) * 1000.0
            break

    if boot_ms is None:
        kill_tree(proc.pid)
        raise RuntimeError(f"timed out waiting for READY after {READY_TIMEOUT_S}s")

    idle_rss_mb = None
    if want_idle:
        time.sleep(IDLE_SETTLE_S)
        if proc.poll() is None:
            pids = descendants(proc.pid)
            idle_rss_mb = rss_bytes_for(pids) / (1024 * 1024)

    kill_tree(proc.pid)
    return boot_ms, idle_rss_mb


def summary(name: str, samples: list, unit: str):
    if not samples:
        return {"name": name, "unit": unit, "samples": []}
    return {
        "name": name,
        "unit": unit,
        "n": len(samples),
        "min": min(samples),
        "median": stats.median(samples),
        "p95": sorted(samples)[max(0, int(round((len(samples) - 1) * 0.95)))],
        "max": max(samples),
        "mean": stats.fmean(samples),
        "stdev": stats.pstdev(samples),
        "samples": samples,
    }


def main():
    if not Path(BINARY).is_file():
        print(f"binary not found: {BINARY}", file=sys.stderr)
        sys.exit(2)

    print(f"# ide99-bench / boot_and_idle", file=sys.stderr)
    print(f"# binary           : {BINARY}", file=sys.stderr)
    print(f"# boot_iterations  : {BOOT_ITERATIONS}", file=sys.stderr)
    print(f"# idle_iterations  : {IDLE_ITERATIONS}", file=sys.stderr)
    print(f"# idle_settle_sec  : {IDLE_SETTLE_S}", file=sys.stderr)

    # one untimed warm-up to absorb cold filesystem cache
    print("# warm-up...", file=sys.stderr)
    measure_one(want_idle=False)

    boot_samples = []
    print(f"# boot phase: {BOOT_ITERATIONS} iterations", file=sys.stderr)
    for i in range(BOOT_ITERATIONS):
        b, _ = measure_one(want_idle=False)
        boot_samples.append(b)
        print(f"  boot[{i:02d}] = {b:7.1f} ms", file=sys.stderr)

    idle_samples = []
    print(f"# idle phase: {IDLE_ITERATIONS} iterations", file=sys.stderr)
    for i in range(IDLE_ITERATIONS):
        _, m = measure_one(want_idle=True)
        if m is not None:
            idle_samples.append(m)
            print(f"  idle[{i:02d}] = {m:7.1f} MB", file=sys.stderr)

    out = {
        "bench": "boot_and_idle",
        "binary": BINARY,
        "binary_size_mb": Path(BINARY).stat().st_size / (1024 * 1024),
        "boot": summary("boot_to_ready", boot_samples, "ms"),
        "idle_rss": summary("idle_rss_total_proc_tree", idle_samples, "MB"),
    }
    json.dump(out, sys.stdout, indent=2)
    print()


if __name__ == "__main__":
    main()
