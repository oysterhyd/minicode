"""Supplemental scenarios cover multi-file repair, long logs, and retry."""

import asyncio
import json
import shutil
from pathlib import Path

import pytest

from evals.hidden import check_hidden
from evals.run_eval import load_task, run_combo


ROOT = Path(__file__).resolve().parents[1] / "evals" / "scenarios"
SCENARIOS = ("counter_retry", "invoice_discount", "log_context")


@pytest.mark.parametrize("task_id", SCENARIOS)
def test_scenario_hidden_red_green(task_id, tmp_path):
    source = ROOT / task_id
    workspace = tmp_path / "workspace"
    shutil.copytree(source / "repo", workspace)
    red, _ = asyncio.run(check_hidden(task_id, workspace))
    assert red is False
    script = json.loads((source / "script.json").read_text(encoding="utf-8"))
    for turn in script["turns"]:
        for call in turn.get("tool_calls", []):
            if call["name"] != "edit":
                continue
            edit = call["arguments"]
            target = workspace / edit["path"]
            original = target.read_text(encoding="utf-8")
            assert edit["old_text"] in original
            target.write_text(original.replace(edit["old_text"], edit["new_text"], 1),
                              encoding="utf-8")
    green, detail = asyncio.run(check_hidden(task_id, workspace))
    assert green is True, detail


def test_retry_scenario_exercises_b2_continuation():
    task = load_task(ROOT / "counter_retry")
    b0 = asyncio.run(run_combo(task, "b0"))
    b2 = asyncio.run(run_combo(task, "b2"))
    assert b0.passed is False
    assert b2.passed is True
    assert b2.attempts == 2
    assert b2.hidden_pass is True
