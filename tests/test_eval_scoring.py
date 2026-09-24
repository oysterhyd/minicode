"""The external evaluator enforces allowed paths independently of the agent."""

import shutil
from pathlib import Path

from evals.run_eval import check_allowed_paths, load_task, snapshot_workspace


def test_external_scorer_rejects_changes_outside_allowed_paths(tmp_path):
    task = load_task(Path(__file__).resolve().parents[1] / "evals" / "tasks" / "pagination_bounds")
    workspace = tmp_path / "workspace"
    shutil.copytree(task.repo_dir, workspace)
    before = snapshot_workspace(workspace)
    (workspace / "paginate.py").write_text("fixed = True\n", encoding="utf-8")
    assert check_allowed_paths(task, before, workspace) == []
    (workspace / "test_paginate.py").write_text("assert True\n", encoding="utf-8")
    assert "test_paginate.py" in check_allowed_paths(task, before, workspace)[0]
    (workspace / "test_paginate.py").unlink()
    assert "test_paginate.py" in check_allowed_paths(task, before, workspace)[0]
