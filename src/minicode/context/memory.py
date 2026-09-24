"""Explicit, project-scoped facts kept separately from session compression."""

from __future__ import annotations

import uuid
from pathlib import Path

from pydantic import BaseModel

from minicode.core.clock import utc_now
from minicode.core.paths import PathOutsideWorkspaceError, resolve_in_workspace
from minicode.storage.sqlite_store import SqliteStore


class MemoryFact(BaseModel):
    id: str
    workspace: str
    scope: str
    fact: str
    source: str
    updated_at: str


class ProjectMemoryStore:
    """Only the host CLI can change facts; the agent receives read access."""

    def __init__(self, store: SqliteStore):
        self._store = store
        with store.transaction() as conn:
            conn.execute("""CREATE TABLE IF NOT EXISTS project_memory (
                id TEXT PRIMARY KEY, workspace TEXT NOT NULL, scope TEXT NOT NULL,
                fact TEXT NOT NULL, source TEXT NOT NULL, updated_at TEXT NOT NULL
            )""")

    @staticmethod
    def _workspace(path: Path) -> str:
        resolved = Path(path).resolve()
        if not resolved.is_dir():
            raise ValueError(f"workspace does not exist: {resolved}")
        return str(resolved)

    @staticmethod
    def _scope(workspace: Path, scope: str) -> str:
        try:
            target = resolve_in_workspace(workspace, scope)
        except PathOutsideWorkspaceError as exc:
            raise ValueError(str(exc)) from exc
        return target.relative_to(Path(workspace).resolve()).as_posix()

    def add(self, workspace: Path, fact: str, source: str, scope: str = ".") -> MemoryFact:
        if not fact.strip() or not source.strip():
            raise ValueError("fact and source must be nonempty")
        root = self._workspace(workspace)
        row = MemoryFact(id=uuid.uuid4().hex, workspace=root,
                         scope=self._scope(Path(root), scope),
                         fact=fact.strip(), source=source.strip(), updated_at=utc_now())
        with self._store.transaction() as conn:
            conn.execute("INSERT INTO project_memory VALUES (?, ?, ?, ?, ?, ?)",
                         (row.id, row.workspace, row.scope, row.fact, row.source, row.updated_at))
        return row

    def list(self, workspace: Path, *, path: str | None = None) -> list[MemoryFact]:
        root = self._workspace(workspace)
        relative = self._scope(Path(root), path) if path is not None else None
        rows = self._store.conn.execute(
            "SELECT * FROM project_memory WHERE workspace = ? ORDER BY updated_at, id", (root,)
        ).fetchall()
        facts = [MemoryFact(**dict(row)) for row in rows]
        if relative is not None:
            facts = [fact for fact in facts if fact.scope == "." or relative == fact.scope
                     or relative.startswith(fact.scope + "/")]
        return facts

    def update(self, workspace: Path, memory_id: str, fact: str, source: str) -> MemoryFact:
        if not fact.strip() or not source.strip():
            raise ValueError("fact and source must be nonempty")
        root = self._workspace(workspace)
        with self._store.transaction() as conn:
            cursor = conn.execute(
                "UPDATE project_memory SET fact = ?, source = ?, updated_at = ? "
                "WHERE workspace = ? AND id = ?",
                (fact.strip(), source.strip(), utc_now(), root, memory_id),
            )
            if cursor.rowcount != 1:
                raise ValueError(f"unknown memory id: {memory_id}")
        return next(row for row in self.list(Path(root)) if row.id == memory_id)

    def delete(self, workspace: Path, memory_id: str) -> None:
        root = self._workspace(workspace)
        with self._store.transaction() as conn:
            cursor = conn.execute("DELETE FROM project_memory WHERE workspace = ? AND id = ?",
                                  (root, memory_id))
            if cursor.rowcount != 1:
                raise ValueError(f"unknown memory id: {memory_id}")
