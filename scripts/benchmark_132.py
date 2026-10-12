"""Fixed offline workloads, runnable against either checkout with --root."""
from __future__ import annotations

import argparse
import gc
import json
import platform
import re
import statistics
import sys
import time
import tracemalloc
from pathlib import Path
from tempfile import TemporaryDirectory

parser = argparse.ArgumentParser()
parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
sys.path.insert(0, str(args.root.resolve() / "src"))

from minicode.context.compact import ContextCompactor, CompactConfig
from minicode.core.models import Message, TextBlock, ToolUseBlock, ToolResultBlock, EventType
from minicode.storage.sqlite_store import SqliteStore
from minicode.tools.base import ToolContext, ToolLimits, tail_output, utf8_prefix
from minicode.tools.search import GrepTool


def measure(fn):
    fn()
    samples = []
    for _ in range(7):
        gc.collect()
        start = time.perf_counter_ns()
        fn()
        samples.append((time.perf_counter_ns() - start) / 1e6)
    gc.collect()
    tracemalloc.start()
    fn()
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    return {"median_ms": statistics.median(samples), "samples_ms": samples, "peak_bytes": peak}


def main():
    workloads = {}
    output = "command output line\n" * 1_000_000
    prefix = "中文🙂abc" * 1_000_000
    workloads["command_tail_20MB"] = measure(lambda: tail_output(output))
    workloads["utf8_prefix_13MB"] = measure(lambda: utf8_prefix(prefix, 50 * 1024))

    store = SqliteStore(":memory:")
    sid = store.create_session(workspace="offline", provider="offline", model="offline")
    text = "x" * 10000
    with store.transaction() as conn:
        conn.executemany("INSERT INTO messages VALUES (?, ?, ?, ?)", (
            (sid, i, "user", json.dumps([{"type": "text", "text": text}])) for i in range(2000)))
        conn.executemany("INSERT INTO events VALUES (?, ?, ?, ?, ?)", (
            (sid, i, EventType.TOOL_OUTPUT.value, "2026-01-01T00:00:00Z", json.dumps({"output": text}))
            for i in range(2000)))
    workloads["messages_2000_10KB"] = measure(lambda: store.get_messages(sid))
    workloads["events_2000_10KB"] = measure(lambda: store.get_events(sid))
    assert len(store.get_messages(sid)) == len(store.get_events(sid)) == 2000
    store.close()

    # Every early prefix splits a call/result pair, forcing the full safety scan.
    messages = []
    for i in range(1200):
        messages.extend([Message(role="user", content=[TextBlock(text="task")]),
                         Message(role="assistant", content=[ToolUseBlock(id=str(i), name="read", input={})])])
    messages.append(Message(role="user", content=[ToolResultBlock(tool_use_id=str(i), content="x") for i in range(1200)]))
    compactor = ContextCompactor(CompactConfig(max_context_tokens=100000, tail_keep_rounds=1))
    assert compactor._archive_early_units(messages)[1] == 0
    workloads["compaction_crossing_1200_units"] = measure(lambda: compactor._archive_early_units(messages))
    normal = []
    for i in range(1200):
        normal.extend([Message(role="user", content=[TextBlock(text="task")]),
                       Message(role="assistant", content=[ToolUseBlock(id=str(i), name="read", input={})]),
                       Message(role="user", content=[ToolResultBlock(tool_use_id=str(i), content="x")])])
    assert compactor._archive_early_units(normal)[1] == 1199
    workloads["compaction_normal_1200_units"] = measure(lambda: compactor._archive_early_units(normal))

    with TemporaryDirectory(prefix="minicode-perf-") as directory:
        root = Path(directory)
        (root / "many.txt").write_text("needle here\n" * 80_000, encoding="utf-8", newline="")
        tool = GrepTool()
        ctx = ToolContext(root, limits=ToolLimits(search_max_file_bytes=1_000_000))
        search = lambda: tool._python_search(re.compile("needle"), root, None, 100, ctx)
        assert len(search()) == 101
        workloads["fallback_search_first_100_of_80000"] = measure(search)
        full_scan = lambda: tool._python_search(re.compile("absent"), root, None, 100, ctx)
        assert not full_scan()
        workloads["fallback_search_no_match_80000"] = measure(full_scan)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"python": platform.python_version(), "platform": platform.platform(),
                                      "source_root": str(args.root.resolve()), "benchmarks": workloads}, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
