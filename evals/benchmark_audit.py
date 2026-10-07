"""Offline, reproducible hot-path benchmarks; accepts a clean baseline source tree.

python evals/benchmark_audit.py --source-root <checkout> --output <result.json>
No network, credentials or production database. Timings exclude fixture setup.
"""
from __future__ import annotations

import argparse
import gc
import json
import platform
import statistics
import sys
import tempfile
import time
import tracemalloc
from pathlib import Path


def measure(fn, repeats=5):
    samples = []
    for _ in range(repeats):
        gc.collect()
        start = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - start) * 1000)
    gc.collect()
    tracemalloc.start()
    fn()
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return {"median_ms": round(statistics.median(samples), 4), "min_ms": round(min(samples), 4),
            "max_ms": round(max(samples), 4), "peak_mib": round(peak / 1024**2, 4), "repeats": repeats}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.source_root.resolve() / "src"))
    from minicode.storage import SqliteStore, ArtifactStore
    from minicode.core.models import Message, TextBlock, ToolUseBlock, ToolResultBlock
    from minicode.context.compact import ContextCompactor, CompactConfig
    from minicode.providers.commandcode import _absorb_tool_call_delta
    import inspect

    results = {"source_root": str(args.source_root.resolve()), "python": platform.python_version(),
               "platform": platform.platform(), "benchmarks": {}}
    with tempfile.TemporaryDirectory(prefix="minicode-benchmark-") as directory:
        with SqliteStore(Path(directory) / "sessions.db") as store:
            sids = [store.create_session(workspace="offline", provider="fake", model="fake") for _ in range(100)]
            with store.transaction() as conn:
                conn.executemany("INSERT INTO events VALUES (?, ?, 'round_start', ?, '{}')",
                    ((sid, seq, f"2026-09-30T{seq % 24:02d}:{seq % 60:02d}:00+00:00")
                     for sid in sids for seq in range(1500)))
                conn.executemany("INSERT INTO artifacts VALUES (?, ?, 'tool_output', ?, '2026-09-30')",
                    ((sids[0], f"artifact-{i}", f"artifact-{i}.txt") for i in range(20_000)))
            artifacts = ArtifactStore(store)
            results["fixture_database_bytes"] = (store.conn.execute("PRAGMA page_count").fetchone()[0]
                                                * store.conn.execute("PRAGMA page_size").fetchone()[0])

            def activity():
                assert len(store.last_activity()) == 100
            results["benchmarks"]["sidebar_activity_150k_events"] = measure(activity)

            def manifest():
                assert artifacts._relpath(sids[0], "artifact-19999") == "artifact-19999.txt"
            results["benchmarks"]["artifact_lookup_20k_rows"] = measure(manifest)

            original = "汉🙂abc" * 200_000  # one million characters, UTF-8 multibyte
            ref = artifacts.spill(sids[1], "tool_output", original)

            def pages():
                reader = ArtifactStore(store)  # cold per-reader seek index
                count = 0
                for offset in range(0, len(original), 20_000):
                    page, _, _ = reader.read_page(sids[1], ref.artifact_id, offset, 20_000)
                    count += len(page)
                assert count == len(original)
            results["benchmarks"]["artifact_sequential_1m_chars_50_pages"] = measure(pages, 3)

    messages = [Message(role="user", content=[TextBlock(text="original task")])]
    for index in range(1000):
        messages.extend([
            Message(role="assistant", content=[ToolUseBlock(id=f"call-{index}", name="read", input={"path": "file"})]),
            Message(role="user", content=[ToolResultBlock(tool_use_id=f"call-{index}", content="x" * 200)]),
        ])
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000, tail_keep_rounds=2))

    def compact():
        result = compactor.compact(None, messages)
        assert result.stats.archived_units == 998
    results["benchmarks"]["compact_1000_closed_exchanges"] = measure(compact, 3)

    bounded = "remaining_bytes" in inspect.signature(_absorb_tool_call_delta).parameters

    def fragments():
        buffers, order = {}, []
        remaining = 16_000_000
        delta = {"index": 0, "function": {"arguments": "x" * 256}}
        for _ in range(8000):
            if bounded:
                remaining -= _absorb_tool_call_delta(delta, buffers, order, remaining)
            else:
                _absorb_tool_call_delta(delta, buffers, order)
        value = buffers[0]["arguments"]
        value = "".join(value) if isinstance(value, list) else value
        assert len(value) == 8000 * 256
    results["benchmarks"]["tool_arguments_8000_fragments_2m_bytes"] = measure(fragments, 3)
    output = json.dumps(results, indent=2, ensure_ascii=False)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + "\n", encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
