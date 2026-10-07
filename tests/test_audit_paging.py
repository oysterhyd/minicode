"""Bounded indexed reads, equivalence and asynchronous UI paging contracts."""
from __future__ import annotations

import asyncio
import os
import random

import pytest

from minicode.core.models import EventType
from minicode.storage import ArtifactStore, SqliteStore
from minicode.storage.paging import TextPageReader


@pytest.fixture()
def fixture(tmp_path):
    with SqliteStore(tmp_path / "sessions.db") as store:
        sid = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        artifacts = ArtifactStore(store)
        original = "汉🙂\r\nabc\ndef\r" * 20_000
        ref = artifacts.spill(sid, "tool_output", original)
        yield store, sid, artifacts, ref, original


def test_artifact_lookup_uses_primary_key_not_full_manifest(fixture, monkeypatch):
    store, sid, artifacts, ref, original = fixture
    def forbidden(_):
        raise AssertionError("must not load the full manifest")
    monkeypatch.setattr(store, "list_artifacts", forbidden)
    assert artifacts.read_page(sid, ref.artifact_id, 0, 20)[0] == original[:20]
    assert artifacts.read_page("another-session", ref.artifact_id, 0, 20) is None


def test_sparse_paging_random_unicode_offsets_and_cache_invalidation(fixture):
    _, sid, artifacts, ref, original = fixture
    rng = random.Random(42)
    for offset in [rng.randrange(len(original)) for _ in range(60)] + [len(original), len(original) + 10]:
        page, total, more = artifacts.read_page(sid, ref.artifact_id, offset, 1000)
        assert page == original[offset:offset + 1000]
        assert more == (offset + 1000 < len(original))
        assert total == (None if more else len(original))
    target = artifacts._session_dir(sid) / artifacts._relpath(sid, ref.artifact_id)
    previous = target.stat()
    target.write_bytes("changed🙂\r\n".encode())
    os.utime(target, ns=(previous.st_atime_ns, previous.st_mtime_ns + 1_000_000))
    assert artifacts.read_page(sid, ref.artifact_id, 0, 100) == ("changed🙂\r\n", 10, False)


def test_sequential_paging_does_not_rescan_previous_page(fixture, monkeypatch):
    _, sid, artifacts, ref, original = fixture
    from pathlib import Path
    opened = Path.open
    read_characters = 0
    class Counted:
        def __init__(self, handle): self.handle = handle
        def __enter__(self): return self
        def __exit__(self, *args): return self.handle.__exit__(*args)
        def __getattr__(self, name): return getattr(self.handle, name)
        def read(self, size=-1):
            nonlocal read_characters
            value = self.handle.read(size)
            read_characters += len(value)
            return value
    monkeypatch.setattr(Path, "open", lambda self, *args, **kwargs: Counted(opened(self, *args, **kwargs)))
    assert artifacts.read_page(sid, ref.artifact_id, 0, 20_000)[0] == original[:20_000]
    assert artifacts.read_page(sid, ref.artifact_id, 20_000, 20_000)[0] == original[20_000:40_000]
    assert read_characters == 40_002  # page bytes decoded once, plus each lookahead


def test_concurrent_off_loop_pages_have_independent_seek_positions(fixture):
    _, sid, artifacts, ref, original = fixture
    offsets = [150_000, 1, 70_000, 35_000, 200_000, 0]
    async def scenario():
        pages = await asyncio.gather(*(artifacts.aread_page(sid, ref.artifact_id, offset, 1234) for offset in offsets))
        for offset, page in zip(offsets, pages):
            assert page[0] == original[offset:offset + 1234]
    asyncio.run(scenario())


def test_reader_cache_is_bounded(tmp_path):
    reader = TextPageReader()
    for index in range(40):
        path = tmp_path / f"{index}.txt"
        path.write_bytes(b"abc")
        assert reader.read(path, 0, 2) == ("ab", None, True)
    assert len(reader._indices) == 32


def test_event_pages_preserve_order_and_activity_handles_clock_reversal(fixture):
    store, sid, _, _, _ = fixture
    for _ in range(10): store.append_event(sid, EventType.ROUND_START)
    assert [e.seq for e in store.get_events(sid, after_seq=5, limit=2)] == [6, 7]
    assert [e.seq for e in store.get_events(sid, newest_first=True, limit=3)] == [9, 8, 7]
    assert store.get_events(sid, limit=0) == []
    with pytest.raises(ValueError): store.get_events(sid, limit=-1)
    with store.transaction() as conn:
        conn.execute("UPDATE events SET timestamp = '2026-10-01' WHERE seq = 0")
        conn.execute("UPDATE events SET timestamp = '2026-09-30' WHERE seq > 0")
    assert store.last_activity()[sid] == "2026-10-01"
    plan = store.conn.execute("EXPLAIN QUERY PLAN SELECT MAX(timestamp) FROM events WHERE session_id = ?", (sid,)).fetchall()
    assert any("events_activity" in row[3] for row in plan)
