from math import isclose

from minicode.core.models import EventType
from minicode.storage.sqlite_store import SqliteStore
from minicode.storage.statistics import SessionStatistics


def test_incremental_statistics_paging_isolation_and_session_switch(tmp_path):
    with SqliteStore(tmp_path / "stats.db") as store:
        sid = store.create_session(workspace="work", provider="fake", model="fake")
        other = store.create_session(workspace="work", provider="fake", model="fake")
        expected_seconds = []
        with store.transaction() as conn:
            import json
            rows = []
            for i in range(1200):
                seconds = 0.1 + i / 1000
                expected_seconds.append(seconds)
                data = {"usage": {"output_tokens": 600}, "request_seconds": seconds, "generation_seconds": 2}
                rows.append((sid, i * 2, "assistant_message", "now", json.dumps(data)))
                rows.append((sid, i * 2 + 1, "tool_call_start", "now", "{}"))
            conn.executemany("INSERT INTO events VALUES (?, ?, ?, ?, ?)", rows)
        stats = SessionStatistics()
        first = stats.read(store, sid)
        assert first["requests"] == first["toolCalls"] == 1200
        assert first["tps"] == 300
        assert isclose(first["modelSeconds"], sum(expected_seconds), rel_tol=1e-14)
        assert first["samples"][0]["round"] == 1169 and len(first["samples"]) == 32
        first["samples"][0]["output"] = -1
        assert stats.read(store, sid)["samples"][0]["output"] == 600
        store.append_event(sid, EventType.TOOL_OUTPUT, {"output_preview": "x" * 10000})
        assert stats.read(store, sid)["requests"] == 1200
        assert stats.after_seq == store.latest_event_seq(sid)
        store.append_event(sid, EventType.ASSISTANT_MESSAGE, {"usage": {"available": False, "output_tokens": 9999},
                           "request_seconds": 10, "generation_seconds": 1})
        unknown = stats.read(store, sid)
        assert unknown["requests"] == 1201 and unknown["lastTps"] is None and unknown["tps"] == 300
        assert stats.read(store, other)["requests"] == 0
        assert stats.read(store, sid) == unknown
        assert stats.read(store, None)["samples"] == []
        assert store.get_events(sid, types=()) == []
        assert len(store.get_events(sid, types=(EventType.ASSISTANT_MESSAGE,), newest_first=True, limit=2)) == 2


def test_statistics_reads_only_new_relevant_events(tmp_path, monkeypatch):
    with SqliteStore(tmp_path / "stats.db") as store:
        sid = store.create_session(workspace="work", provider="fake", model="fake")
        store.append_event(sid, EventType.ASSISTANT_MESSAGE, {"usage": {"output_tokens": 100}, "generation_seconds": 2})
        stats = SessionStatistics()
        stats.read(store, sid)
        calls = []
        original = store.get_events
        def read(*args, **kwargs):
            calls.append(kwargs)
            return original(*args, **kwargs)
        monkeypatch.setattr(store, "get_events", read)
        assert stats.read(store, sid)["requests"] == 1 and calls == []
        store.append_event(sid, EventType.TOOL_CALL_START)
        assert stats.read(store, sid)["toolCalls"] == 1
        assert calls[0]["after_seq"] == 0 and calls[0]["types"] == (EventType.ASSISTANT_MESSAGE, EventType.TOOL_CALL_START)
