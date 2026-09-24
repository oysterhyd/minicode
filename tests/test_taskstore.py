"""Tests for minicode.tasks.taskstore (TaskStore) — dependency validation,
state machine transitions and the cascade, on a tmp-path SQLite database."""

from __future__ import annotations

import pytest

from minicode.storage import SqliteStore
from minicode.tasks.taskstore import TaskStore, _find_cycle


@pytest.fixture()
def store(tmp_path):
    db = SqliteStore(tmp_path / "db.sqlite3")
    yield db
    db.close()


@pytest.fixture()
def tasks(store):
    return TaskStore(store)


def by_id(rows):
    return {row.task_id: row for row in rows}


# ---------------------------------------------------------------------------
# add / list
# ---------------------------------------------------------------------------


def test_add_starts_ready_without_deps_and_pending_with_deps(tasks):
    tasks.add("s1", "t1", "First")
    tasks.add("s1", "t2", "Second", depends_on=["t1"])
    rows = by_id(tasks.list_tasks("s1"))
    assert rows["t1"].status == "ready"
    assert rows["t1"].owner is None
    assert rows["t1"].depends_on == []
    assert rows["t2"].status == "pending"
    assert rows["t2"].depends_on == ["t1"]
    assert rows["t1"].created_at and rows["t1"].updated_at


def test_add_duplicate_task_id_raises(tasks):
    tasks.add("s1", "t1", "First")
    with pytest.raises(ValueError, match="already exists"):
        tasks.add("s1", "t1", "First again")


def test_add_unknown_dependency_raises(tasks):
    with pytest.raises(ValueError, match="unknown dependency: nope"):
        tasks.add("s1", "t1", "First", depends_on=["nope"])


def test_dependencies_are_session_scoped(tasks):
    tasks.add("s1", "t1", "First")
    with pytest.raises(ValueError, match="unknown dependency: t1"):
        tasks.add("s2", "t2", "Second", depends_on=["t1"])
    assert tasks.list_tasks("s2") == []


def test_seeded_cycle_is_detected_on_add(tasks, store):
    # Build a cycle that bypasses add()'s own guards, then prove add() still
    # refuses to attach new work to it.
    tasks.add("s1", "a", "A")
    tasks.add("s1", "b", "B", depends_on=["a"])
    with store.transaction() as conn:
        conn.execute(
            "UPDATE tasks SET depends_on = ? WHERE session_id = 's1' AND task_id = 'a'",
            ('["b"]',),
        )
    with pytest.raises(ValueError, match="cycle"):
        tasks.add("s1", "c", "C", depends_on=["a"])


def test_find_cycle_unit():
    graph = {"a": ["b"], "b": ["a"], "c": ["a"], "d": []}
    assert _find_cycle(graph, "c") is not None
    assert _find_cycle(graph, "d") is None
    assert _find_cycle({"a": ["a"]}, "a") is not None


# ---------------------------------------------------------------------------
# claim
# ---------------------------------------------------------------------------


def test_claim_moves_ready_to_running_with_owner(tasks):
    tasks.add("s1", "t1", "First")
    tasks.claim("s1", "t1", owner="worker-1")
    (row,) = tasks.list_tasks("s1")
    assert row.status == "running"
    assert row.owner == "worker-1"


def test_claim_unknown_task_raises(tasks):
    with pytest.raises(ValueError, match="unknown task"):
        tasks.claim("s1", "missing", owner="w")


def test_claim_not_ready_reports_status_and_blocking_deps(tasks):
    tasks.add("s1", "zz-done", "Z")
    tasks.add("s1", "a", "A")
    tasks.add("s1", "b", "B", depends_on=["a", "zz-done"])
    tasks.claim("s1", "a", owner="w")
    tasks.complete("s1", "a", ok=True, owner="w")

    with pytest.raises(ValueError, match="current status: pending"):
        tasks.claim("s1", "b", owner="w2")
    with pytest.raises(ValueError, match="zz-done"):
        tasks.claim("s1", "b", owner="w2")  # message names unfinished deps

    tasks.claim("s1", "zz-done", owner="w")
    with pytest.raises(ValueError, match="current status: running"):
        tasks.claim("s1", "zz-done", owner="w3")


# ---------------------------------------------------------------------------
# complete + cascade
# ---------------------------------------------------------------------------


def test_complete_requires_running(tasks):
    tasks.add("s1", "t1", "First")
    with pytest.raises(ValueError, match="not running"):
        tasks.complete("s1", "t1", ok=True, owner="w")


def test_complete_rejects_different_owner(tasks):
    tasks.add("s1", "t1", "First")
    tasks.claim("s1", "t1", owner="worker-1")
    with pytest.raises(ValueError, match="owned by"):
        tasks.complete("s1", "t1", ok=True, owner="worker-2")
    assert tasks.list_tasks("s1")[0].status == "running"


def test_complete_ok_done_and_cascade_ready(tasks):
    tasks.add("s1", "a", "A")
    tasks.add("s1", "b", "B", depends_on=["a"])
    tasks.claim("s1", "a", owner="w")
    tasks.complete("s1", "a", ok=True, owner="w")

    rows = by_id(tasks.list_tasks("s1"))
    assert rows["a"].status == "done"
    assert rows["b"].status == "ready"  # cascade unblocked the dependent


def test_complete_failed_does_not_cascade(tasks):
    tasks.add("s1", "a", "A")
    tasks.add("s1", "b", "B", depends_on=["a"])
    tasks.claim("s1", "a", owner="w")
    tasks.complete("s1", "a", ok=False, owner="w")

    rows = by_id(tasks.list_tasks("s1"))
    assert rows["a"].status == "failed"
    assert rows["b"].status == "pending"


def test_cascade_waits_for_all_dependencies(tasks):
    tasks.add("s1", "a", "A")
    tasks.add("s1", "c", "C")
    tasks.add("s1", "m", "M", depends_on=["a", "c"])

    tasks.claim("s1", "a", owner="w")
    tasks.complete("s1", "a", ok=True, owner="w")
    assert by_id(tasks.list_tasks("s1"))["m"].status == "pending"  # c still open

    tasks.claim("s1", "c", owner="w")
    tasks.complete("s1", "c", ok=True, owner="w")
    assert by_id(tasks.list_tasks("s1"))["m"].status == "ready"


def test_cascade_is_transitive_in_one_pass(tasks):
    # a <- b <- c <- d: completing a readies only b; d becomes ready stepwise.
    tasks.add("s1", "a", "A")
    tasks.add("s1", "b", "B", depends_on=["a"])
    tasks.add("s1", "c", "C", depends_on=["b"])
    tasks.add("s1", "d", "D", depends_on=["c"])

    tasks.claim("s1", "a", owner="w")
    tasks.complete("s1", "a", ok=True, owner="w")
    rows = by_id(tasks.list_tasks("s1"))
    assert rows["b"].status == "ready"
    assert rows["c"].status == "pending"
    assert rows["d"].status == "pending"

    tasks.claim("s1", "b", owner="w")
    tasks.complete("s1", "b", ok=True, owner="w")
    assert by_id(tasks.list_tasks("s1"))["c"].status == "ready"


# ---------------------------------------------------------------------------
# durability / schema
# ---------------------------------------------------------------------------


def test_schema_survives_reopen_and_ensure_is_idempotent(tmp_path):
    db_path = tmp_path / "db.sqlite3"
    store = SqliteStore(db_path)
    first = TaskStore(store)
    first.add("s1", "t1", "First")
    store.close()

    reopened = SqliteStore(db_path)
    try:
        second = TaskStore(reopened)
        second.ensure_schema()  # idempotent CREATE TABLE IF NOT EXISTS
        assert [row.task_id for row in second.list_tasks("s1")] == ["t1"]
    finally:
        reopened.close()
