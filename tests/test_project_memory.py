"""Explicit project memory remains scoped, editable and readable by the agent."""

import asyncio
import json

import pytest

from minicode.context.memory import ProjectMemoryStore
from minicode.storage import SqliteStore
from minicode.tools.base import ToolContext
from minicode.tools.memory import MemoryListTool


def test_project_memory_scope_and_edit_lifecycle(tmp_path):
    workspace = tmp_path / "repo"
    (workspace / "src").mkdir(parents=True)
    other = tmp_path / "other"
    other.mkdir()
    with SqliteStore(tmp_path / "db.sqlite3") as store:
        memory = ProjectMemoryStore(store)
        root = memory.add(workspace, "Use Python 3.11", "README.md")
        local = memory.add(workspace, "Parser uses UTF-8", "src/parser.py", "src")
        memory.add(other, "Other project", "user")
        assert {row.id for row in memory.list(workspace)} == {root.id, local.id}
        assert {row.id for row in memory.list(workspace, path="src/parser.py")} == {root.id, local.id}
        assert [row.id for row in memory.list(workspace, path="README.md")] == [root.id]
        with pytest.raises(ValueError):
            memory.add(workspace, "bad", "user", "../other")
        changed = memory.update(workspace, local.id, "Parser uses UTF-8 strictly", "src/parser.py:8")
        assert changed.fact.endswith("strictly")
        assert changed.source.endswith(":8")
        outcome = asyncio.run(MemoryListTool().run({"path": "src/parser.py"},
                                                    ToolContext(workspace=workspace, project_memory=memory)))
        assert outcome.success
        assert len(json.loads(outcome.output)) == 2
        memory.delete(workspace, local.id)
        assert [row.id for row in memory.list(workspace)] == [root.id]


def test_project_memory_survives_store_reopen(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    db = tmp_path / "db.sqlite3"
    with SqliteStore(db) as store:
        saved = ProjectMemoryStore(store).add(workspace, "Fact", "user")
    with SqliteStore(db) as store:
        assert ProjectMemoryStore(store).list(workspace)[0].id == saved.id
