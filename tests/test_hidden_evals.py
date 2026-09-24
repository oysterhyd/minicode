"""Held-out tests reject the buggy fixture and accept its reference repair."""

import asyncio
import json
import shutil
from pathlib import Path

import pytest

from evals.hidden import check_hidden


TASKS = sorted(path.name for path in
               (Path(__file__).resolve().parents[1] / "evals" / "tasks").iterdir()
               if (path / "task.yaml").is_file())


@pytest.mark.parametrize("task_id", TASKS)
def test_hidden_red_green(task_id, tmp_path):
    source = Path(__file__).resolve().parents[1] / "evals" / "tasks" / task_id
    workspace = tmp_path / "workspace"
    shutil.copytree(source / "repo", workspace)
    before_ok, before_detail = asyncio.run(check_hidden(task_id, workspace))
    assert before_ok is False, f"buggy fixture passed hidden test: {before_detail}"

    script = json.loads((source / "script.json").read_text(encoding="utf-8"))
    edits = [call["arguments"] for turn in script["turns"]
             for call in turn.get("tool_calls", []) if call["name"] == "edit"]
    assert edits
    for edit in edits:
        target = workspace / edit["path"]
        original = target.read_text(encoding="utf-8")
        assert edit["old_text"] in original
        target.write_text(original.replace(edit["old_text"], edit["new_text"], 1),
                          encoding="utf-8")
    after_ok, after_detail = asyncio.run(check_hidden(task_id, workspace))
    assert after_ok is True, f"reference repair failed hidden test: {after_detail}"
