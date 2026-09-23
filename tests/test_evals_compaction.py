"""The offline b1 path actually runs the same compactor as the app."""

from __future__ import annotations

import asyncio
import json
from dataclasses import replace

from evals.run_eval import AcceptanceItem, TaskDef, run_combo
from minicode.context.compact import CompactConfig


def test_b1_compacts_long_script_while_b0_does_not(tmp_path):
    task_dir = tmp_path / "task"
    repo = task_dir / "repo"
    repo.mkdir(parents=True)
    for index in range(4):
        (repo / f"large{index}.txt").write_text("evidence " * 1500, encoding="utf-8")
    script = task_dir / "script.json"
    script.write_text(json.dumps({"turns": [
        {"tool_calls": [{"name": "read", "arguments": {"path": f"large{index}.txt"}}]}
        for index in range(4)
    ] + [{"text": "done"}]}), encoding="utf-8")
    task = TaskDef(
        id="task", title="Read a long document", prompt="inspect",
        allowed_paths=[f"large{index}.txt" for index in range(4)],
        protected_paths=["large0.txt"],
        items=[AcceptanceItem(id="unchanged", type="protected", path="large0.txt")],
        max_rounds=10, max_tokens=100000, max_seconds=30,
        max_fix_attempts=0, repo_dir=repo, script_path=script,
    )
    compact = CompactConfig(max_context_tokens=20000, tail_keep_tokens=500,
                            keep_recent_tool_results=1)
    b0 = asyncio.run(run_combo(task, "b0", compactor=compact))
    b1 = asyncio.run(run_combo(task, "b1", compactor=compact))
    assert b0.passed and b1.passed
    assert b0.compactions == 0
    assert b1.compactions > 0
    assert b1.estimated_context_reduction > 0
    assert b1.to_json()["compactions"] == b1.compactions

    paused = asyncio.run(run_combo(replace(task, max_rounds=1), "b0"))
    assert not paused.passed
    assert "agent stopped before completion" in (paused.error or "")
