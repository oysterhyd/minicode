"""Task dependency store: durable, session-scoped task graph on SQLite.

Tasks are created with dependencies, claimed by workers and completed with a
boolean outcome. A task starts ``ready`` when it has no dependencies and
``pending`` otherwise; completing a task cascades ``ready`` onto every
pending task whose dependencies are now all ``done``.

All writes run inside :meth:`SqliteStore.transaction` (``BEGIN IMMEDIATE``),
so dependency validation and the corresponding insert/update are atomic —
two workers racing to claim the same ready task serialize, and a validation
failure rolls back completely. The table lives in the shared session
database; :meth:`TaskStore.ensure_schema` creates it idempotently.
"""

from __future__ import annotations

import json
import sqlite3
from typing import Literal, Sequence

from pydantic import BaseModel, Field

from minicode.core.clock import utc_now
from minicode.storage.sqlite_store import SqliteStore

TaskStatus = Literal["pending", "ready", "running", "done", "failed"]


class TaskRow(BaseModel):
    """One row of the task table."""

    task_id: str
    title: str
    depends_on: list[str] = Field(default_factory=list)
    status: TaskStatus
    owner: str | None = None
    created_at: str
    updated_at: str


def _find_cycle(graph: dict[str, list[str]], start: str) -> list[str] | None:
    """DFS from *start* over dependency edges; return one cycle path or None.

    Edges point from a task to the tasks it depends on. A cycle exists when
    the search reaches a node already on the current path.
    """
    if start not in graph:
        return None
    visited = {start}
    active = {start: 0}
    end = object()
    stack = [(start, iter(graph[start]))]
    while stack:
        node, children = stack[-1]
        child = next(children, end)
        if child is end:
            stack.pop()
            del active[node]
        elif child in active:
            return [name for name, _ in stack[active[child]:]]
        elif child not in visited and child in graph:
            visited.add(child)
            active[child] = len(stack)
            stack.append((child, iter(graph[child])))
    return None


class TaskStore:
    """Dependency-aware task board backed by the shared session store."""

    def __init__(self, store: SqliteStore) -> None:
        self._store = store
        self.ensure_schema()

    # -- schema --------------------------------------------------------------

    def ensure_schema(self) -> None:
        """Create the ``tasks`` table if it does not exist (idempotent)."""
        with self._store.transaction() as conn:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS tasks (
                    session_id TEXT NOT NULL,
                    task_id    TEXT NOT NULL,
                    title      TEXT NOT NULL,
                    depends_on TEXT NOT NULL,
                    status     TEXT NOT NULL,
                    owner      TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, task_id)
                )
                """
            )

    # -- writes ---------------------------------------------------------------

    def add(
        self,
        session_id: str,
        task_id: str,
        title: str,
        depends_on: Sequence[str] | None = None,
    ) -> None:
        """Insert a task with validated dependencies (same transaction).

        Raises :class:`ValueError` when the task id already exists, when any
        dependency is unknown in this session, or when the dependency graph
        would contain a cycle. New tasks start ``ready`` when all dependencies
        are already done (including no dependencies), otherwise ``pending``.
        """
        deps = list(depends_on) if depends_on is not None else []
        now = utc_now()
        with self._store.transaction() as conn:
            if self._fetch(conn, session_id, task_id) is not None:
                raise ValueError(f"task already exists: {task_id}")
            ready = True
            # An isolated new node cannot reach a cycle. Dependency-bearing
            # additions still validate the reachable graph, including corrupt
            # legacy cycles, using one query rather than one per dependency.
            if deps:
                rows = {row["task_id"]: row for row in conn.execute(
                    "SELECT task_id, depends_on, status FROM tasks WHERE session_id = ?",
                    (session_id,),
                )}
                for dep in deps:
                    if dep not in rows:
                        raise ValueError(f"unknown dependency: {dep} (task {task_id!r} in session {session_id!r})")
                    ready = ready and rows[dep]["status"] == "done"
                graph = {key: json.loads(row["depends_on"]) for key, row in rows.items()}
                graph[task_id] = deps
                cycle = _find_cycle(graph, task_id)
                if cycle is not None:
                    raise ValueError("dependency cycle detected: " + " -> ".join(cycle))

            conn.execute(
                "INSERT INTO tasks (session_id, task_id, title, depends_on,"
                " status, owner, created_at, updated_at)"
                " VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
                (
                    session_id,
                    task_id,
                    title,
                    json.dumps(deps, ensure_ascii=False),
                    "ready" if ready else "pending",
                    now,
                    now,
                ),
            )

    def claim(self, session_id: str, task_id: str, owner: str) -> None:
        """Atomically move a ``ready`` task to ``running`` under *owner*.

        Raises :class:`ValueError` for unknown tasks and for tasks that are
        not ``ready`` (the message names the current status and, when
        applicable, the unfinished dependencies keeping it pending).
        """
        now = utc_now()
        with self._store.transaction() as conn:
            row = self._require(conn, session_id, task_id)
            if row["status"] != "ready":
                blocking = [
                    dep
                    for dep in json.loads(row["depends_on"])
                    if (dep_row := self._fetch(conn, session_id, dep)) is not None
                    and dep_row["status"] != "done"
                ]
                detail = f"; unfinished dependencies: {', '.join(blocking)}" if blocking else ""
                raise ValueError(
                    f"task {task_id} is not claimable (current status: {row['status']}{detail})"
                )
            conn.execute(
                "UPDATE tasks SET status = 'running', owner = ?, updated_at = ?"
                " WHERE session_id = ? AND task_id = ?",
                (owner, now, session_id, task_id),
            )

    def complete(self, session_id: str, task_id: str, ok: bool, *, owner: str) -> None:
        """Finish a ``running`` task as ``done``/``failed``, then cascade.

        On success every pending task whose dependencies are all ``done``
        becomes ``ready`` (cascade runs in the same transaction). Raises
        :class:`ValueError` for unknown tasks and for tasks not in
        ``running`` state.
        """
        now = utc_now()
        with self._store.transaction() as conn:
            row = self._require(conn, session_id, task_id)
            if row["status"] != "running":
                raise ValueError(
                    f"task {task_id} is not running (current status: {row['status']});"
                    " only running tasks can be completed"
                )
            if row["owner"] != owner:
                raise ValueError(f"task {task_id} is owned by {row['owner']!r}, not {owner!r}")
            new_status = "done" if ok else "failed"
            conn.execute(
                "UPDATE tasks SET status = ?, updated_at = ?"
                " WHERE session_id = ? AND task_id = ?",
                (new_status, now, session_id, task_id),
            )
            if not ok:
                return  # failed deps never unblock dependents

            statuses = {
                task_row["task_id"]: task_row["status"]
                for task_row in conn.execute(
                    "SELECT task_id, status FROM tasks WHERE session_id = ?",
                    (session_id,),
                ).fetchall()
            }
            for task_row in conn.execute(
                "SELECT task_id, depends_on FROM tasks"
                " WHERE session_id = ? AND status = 'pending'",
                (session_id,),
            ).fetchall():
                deps = json.loads(task_row["depends_on"])
                if all(statuses.get(dep) == "done" for dep in deps):
                    conn.execute(
                        "UPDATE tasks SET status = 'ready', updated_at = ?"
                        " WHERE session_id = ? AND task_id = ?",
                        (now, session_id, task_row["task_id"]),
                    )

    # -- reads ----------------------------------------------------------------

    def list_tasks(self, session_id: str) -> list[TaskRow]:
        """All tasks of *session_id*, oldest first."""
        rows = self._store.conn.execute(
            "SELECT task_id, title, depends_on, status, owner, created_at, updated_at"
            " FROM tasks WHERE session_id = ?"
            " ORDER BY created_at ASC, task_id ASC",
            (session_id,),
        ).fetchall()
        return [
            TaskRow(
                task_id=row["task_id"],
                title=row["title"],
                depends_on=json.loads(row["depends_on"]),
                status=row["status"],
                owner=row["owner"],
                created_at=row["created_at"],
                updated_at=row["updated_at"],
            )
            for row in rows
        ]

    # -- helpers ---------------------------------------------------------------

    @staticmethod
    def _fetch(
        conn: sqlite3.Connection, session_id: str, task_id: str
    ) -> sqlite3.Row | None:
        return conn.execute(
            "SELECT task_id, title, depends_on, status, owner, created_at, updated_at"
            " FROM tasks WHERE session_id = ? AND task_id = ?",
            (session_id, task_id),
        ).fetchone()

    @staticmethod
    def _require(
        conn: sqlite3.Connection, session_id: str, task_id: str
    ) -> sqlite3.Row:
        row = TaskStore._fetch(conn, session_id, task_id)
        if row is None:
            raise ValueError(f"unknown task: {task_id} (session {session_id!r})")
        return row
