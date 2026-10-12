"""Deterministic local workloads; no model calls, credentials or user data."""
from __future__ import annotations

import argparse
import hashlib
import json
import platform
import statistics
import sys
import time
import tracemalloc
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / "src"), str(ROOT / "desktop")]

from bridge import Bridge
from minicode.core.models import EventType
from minicode.storage.sqlite_store import SqliteStore
from minicode.storage import attachments
from minicode.tasks.taskstore import TaskStore


def measure(fn):
    fn()
    samples = []
    for _ in range(7):
        start = time.perf_counter_ns()
        fn()
        samples.append((time.perf_counter_ns() - start) / 1e6)
    tracemalloc.start()
    fn()
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return {"median_ms": statistics.median(samples), "min_ms": min(samples),
            "max_ms": max(samples), "peak_bytes": peak, "repeats": len(samples)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    store = SqliteStore(":memory:")
    sid = store.create_session(workspace="benchmark", provider="offline", model="offline")
    data = {"output_preview": "x" * 10000}
    assistant = {"text": "x" * 10000, "reasoning": "r" * 1000,
                 "usage": {"output_tokens": 600}, "request_seconds": 7, "generation_seconds": 2}
    with store.transaction() as conn:
        conn.executemany("INSERT INTO events VALUES (?, ?, ?, ?, ?)", [
            (sid, i, EventType.ASSISTANT_MESSAGE.value if i % 10 == 0 else
             EventType.TOOL_CALL_START.value if i % 10 == 1 else EventType.TOOL_OUTPUT.value,
             "2026-01-01T00:00:00Z", json.dumps(assistant if i % 10 == 0 else data))
            for i in range(5000)])
    bridge = Bridge(store)
    bridge.runtime = SimpleNamespace(session_id=sid)
    stats = measure(bridge.statistics)
    def cold_statistics():
        if hasattr(bridge, "_statistics"):
            bridge._statistics.__init__()
        return bridge.statistics()
    cold_stats = measure(cold_statistics)
    result = bridge.statistics()
    assert result["toolCalls"] == 500 and result["requests"] == 500 and result["tps"] == 300

    tasks = TaskStore(store)
    with store.transaction() as conn:
        conn.executemany("INSERT INTO tasks VALUES (?, ?, ?, ?, ?, ?, ?, ?)", [
            (sid, str(i), "Task", "[]", "ready", None, "now", "now") for i in range(5000)])
    number = 5000
    def add():
        nonlocal number
        tasks.add(sid, str(number), "Independent task")
        number += 1
    add_result = measure(add)

    data = b"x" * 20_000_000
    digest = getattr(attachments, "_asset_digest", lambda mime, value:
                     hashlib.sha256(mime.encode() + b"\0" + value).hexdigest())
    digest_result = measure(lambda: digest("image/png", data))
    store.close()
    args.output.write_text(json.dumps({"python": platform.python_version(),
        "platform": platform.platform(), "benchmarks": {
            "statistics_warm_5000_events": stats, "statistics_cold_5000_events": cold_stats,
            "task_add_5000_independent": add_result,
            "attachment_digest_20MB": digest_result}}, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
