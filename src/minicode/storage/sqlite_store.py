"""SQLite-backed session persistence for minicode.

One database file holds everything a session needs to survive a process exit:
the session row (workspace, provider, model, status, exit reason, token
totals), the conversation messages (Anthropic-style block lists serialized as
JSON) and the append-only event trace.

Threading / async note
----------------------
All :class:`SqliteStore` methods are **synchronous** (plain ``sqlite3``). They
are designed to be called directly from the async runtime: every call performs
a handful of indexed operations against one local file and completes in
microseconds to low milliseconds, so it does not meaningfully block the event
loop. The store is not thread-safe beyond what a single shared connection
implies; the runtime uses it from one task at a time.
"""

from __future__ import annotations

import json
import sqlite3
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, Protocol

from pydantic import BaseModel

from minicode.core.clock import utc_now
from minicode.core.models import Event, EventType, Message

__all__ = [
    "DEFAULT_DB_PATH",
    "SCHEMA_VERSION",
    "SessionStore",
    "SessionSummary",
    "SqliteStore",
]

DEFAULT_DB_PATH = Path.home() / ".minicode" / "sessions.db"

#: Layout version recorded in ``PRAGMA user_version``. Bump when the schema
#: changes in a way older code cannot read.
SCHEMA_VERSION = 4

_SESSION_COLUMNS = (
    "session_id, created_at, workspace, provider, model, "
    "status, exit_reason, rounds, input_tokens, output_tokens, "
    "cache_read_tokens, cache_write_tokens, usage_available"
)


class SessionSummary(BaseModel):
    """Lightweight projection of a stored session (no messages or events)."""

    session_id: str
    created_at: str
    workspace: str
    provider: str
    model: str
    status: str = "running"  # free-form: "running", or ExitReason values like "completed"
    exit_reason: str | None = None
    rounds: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    usage_available: bool = True


class SessionStore(Protocol):
    """Storage interface consumed by the runtime; :class:`SqliteStore` implements it."""

    def create_session(self, *, workspace: str, provider: str, model: str) -> str: ...

    def update_session(
        self,
        session_id: str,
        *,
        status: str | None = None,
        # `...` sentinel means "leave unchanged"; None explicitly clears.
        exit_reason: str | None = ...,
        rounds: int | None = None,
        input_tokens: int | None = None,
        output_tokens: int | None = None,
        cache_read_tokens: int | None = None,
        cache_write_tokens: int | None = None,
        usage_available: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        workspace: str | None = None,
    ) -> None: ...

    def get_goal_state(self, session_id: str) -> dict[str, Any] | None: ...

    def save_goal_state(self, session_id: str, state: dict[str, Any]) -> None: ...

    def append_message(self, session_id: str, message: Message) -> int: ...

    def get_messages(self, session_id: str) -> list[Message]: ...

    def append_event(
        self, session_id: str, type: EventType, data: dict[str, Any] | None = None
    ) -> Event: ...

    def get_events(self, session_id: str) -> list[Event]: ...

    def get_session(self, session_id: str) -> SessionSummary | None: ...

    def list_sessions(self) -> list[SessionSummary]: ...


class SqliteStore:
    """SQLite-backed :class:`SessionStore`.

    Synchronous implementation on the stdlib ``sqlite3`` module; methods are
    safe to call directly from the async runtime (see the module docstring).

    Every write runs inside an explicit ``BEGIN IMMEDIATE`` transaction, so a
    crash mid-write cannot leave partial state — in particular the per-session
    sequence allocation (``SELECT MAX(seq) + 1``) and its ``INSERT`` share one
    transaction, which also keeps concurrent writers from computing the same
    sequence number. Message content and event payloads are stored as JSON
    (UTF-8, non-ASCII kept literal via ``ensure_ascii=False``).

    The layout is versioned via ``PRAGMA user_version``: opening a database
    written by a newer minicode raises :class:`RuntimeError`.
    """

    def __init__(self, db_path: str | Path = DEFAULT_DB_PATH) -> None:
        self._db_path = Path(db_path)
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(self._db_path)
        self._conn.row_factory = sqlite3.Row
        self._init_schema()

    # -- lifecycle ---------------------------------------------------------

    def _init_schema(self) -> None:
        version = self._conn.execute("PRAGMA user_version").fetchone()[0]
        if version > SCHEMA_VERSION:
            self._conn.close()
            raise RuntimeError("minicode database was created by a newer version")
        self._conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS sessions (
                session_id    TEXT PRIMARY KEY,
                created_at    TEXT NOT NULL,
                workspace     TEXT NOT NULL,
                provider      TEXT NOT NULL,
                model         TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'running',
                exit_reason   TEXT,
                rounds        INTEGER NOT NULL DEFAULT 0,
                input_tokens  INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
                cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                usage_available    INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE IF NOT EXISTS messages (
                session_id TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                role       TEXT NOT NULL,
                content    TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );

            CREATE TABLE IF NOT EXISTS events (
                session_id TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                type       TEXT NOT NULL,
                timestamp  TEXT NOT NULL,
                data       TEXT NOT NULL,
                PRIMARY KEY (session_id, seq)
            );

            CREATE TABLE IF NOT EXISTS artifacts (
                session_id  TEXT NOT NULL,
                artifact_id TEXT NOT NULL,
                kind        TEXT NOT NULL,
                relpath     TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                PRIMARY KEY (session_id, artifact_id)
            );

            CREATE TABLE IF NOT EXISTS session_goals (
                session_id TEXT PRIMARY KEY,
                state TEXT NOT NULL
            );
            """
        )
        # v3 -> v4: keep cumulative cache usage and whether the provider
        # actually reported usage.  Column checks also repair databases that
        # were created by an interrupted migration before user_version moved.
        session_columns = {
            row["name"]
            for row in self._conn.execute("PRAGMA table_info(sessions)").fetchall()
        }
        if "cache_read_tokens" not in session_columns:
            self._conn.execute(
                "ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0"
            )
        if "cache_write_tokens" not in session_columns:
            self._conn.execute(
                "ALTER TABLE sessions ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0"
            )
        if "usage_available" not in session_columns:
            self._conn.execute(
                "ALTER TABLE sessions ADD COLUMN usage_available INTEGER NOT NULL DEFAULT 0"
            )
        # PRAGMAs cannot bind parameters; SCHEMA_VERSION is a module constant.
        self._conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")
        self._conn.commit()

    def close(self) -> None:
        """Close the underlying connection (tidy teardown in tests / CLI)."""
        self._conn.close()

    def __enter__(self) -> "SqliteStore":
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    @contextmanager
    def transaction(self) -> Iterator[sqlite3.Connection]:
        """Write transaction: BEGIN IMMEDIATE up front, commit on success,
        full rollback on any error. The store's own writes and P1 modules
        (task store) share the single connection through this."""
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            yield self._conn
        except BaseException:
            self._conn.rollback()
            raise
        self._conn.commit()

    @property
    def conn(self) -> sqlite3.Connection:
        """The underlying connection (read queries and migrations only)."""
        return self._conn

    def _require_session(self, conn: sqlite3.Connection, session_id: str) -> None:
        row = conn.execute(
            "SELECT 1 FROM sessions WHERE session_id = ?", (session_id,)
        ).fetchone()
        if row is None:
            raise ValueError(f"unknown session: {session_id}")

    # -- sessions ----------------------------------------------------------

    def create_session(self, *, workspace: str, provider: str, model: str) -> str:
        session_id = uuid.uuid4().hex
        with self.transaction() as conn:
            conn.execute(
                "INSERT INTO sessions "
                "(session_id, created_at, workspace, provider, model, usage_available)"
                " VALUES (?, ?, ?, ?, ?, 1)",
                (session_id, utc_now(), workspace, provider, model),
            )
        return session_id

    def update_session(
        self,
        session_id: str,
        *,
        status: str | None = None,
        exit_reason: str | None = ...,  # noqa: B008 -- `...` sentinel: unchanged; None clears
        rounds: int | None = None,
        input_tokens: int | None = None,
        output_tokens: int | None = None,
        cache_read_tokens: int | None = None,
        cache_write_tokens: int | None = None,
        usage_available: bool | None = None,
        model: str | None = None,
        provider: str | None = None,
        workspace: str | None = None,
    ) -> None:
        assignments: list[str] = []
        params: list[Any] = []
        if status is not None:
            assignments.append("status = ?")
            params.append(status)
        if exit_reason is not ...:
            assignments.append("exit_reason = ?")
            params.append(exit_reason)  # None is intentional here: it clears
        if rounds is not None:
            assignments.append("rounds = ?")
            params.append(rounds)
        if input_tokens is not None:
            assignments.append("input_tokens = ?")
            params.append(input_tokens)
        if output_tokens is not None:
            assignments.append("output_tokens = ?")
            params.append(output_tokens)
        if cache_read_tokens is not None:
            assignments.append("cache_read_tokens = ?")
            params.append(cache_read_tokens)
        if cache_write_tokens is not None:
            assignments.append("cache_write_tokens = ?")
            params.append(cache_write_tokens)
        if usage_available is not None:
            assignments.append("usage_available = ?")
            params.append(int(usage_available))
        if model is not None:
            assignments.append("model = ?")
            params.append(model)
        if provider is not None:
            assignments.append("provider = ?")
            params.append(provider)
        if workspace is not None:
            assignments.append("workspace = ?")
            params.append(workspace)

        with self.transaction() as conn:
            self._require_session(conn, session_id)
            if assignments:
                params.append(session_id)
                conn.execute(
                    f"UPDATE sessions SET {', '.join(assignments)} WHERE session_id = ?",
                    params,
                )

    def get_goal_state(self, session_id: str) -> dict[str, Any] | None:
        row = self._conn.execute(
            "SELECT state FROM session_goals WHERE session_id = ?", (session_id,)
        ).fetchone()
        return json.loads(row["state"]) if row is not None else None

    def save_goal_state(self, session_id: str, state: dict[str, Any]) -> None:
        with self.transaction() as conn:
            self._require_session(conn, session_id)
            conn.execute(
                "INSERT INTO session_goals (session_id, state) VALUES (?, ?) "
                "ON CONFLICT(session_id) DO UPDATE SET state = excluded.state",
                (session_id, json.dumps(state, ensure_ascii=False)),
            )

    def get_session(self, session_id: str) -> SessionSummary | None:
        row = self._conn.execute(
            f"SELECT {_SESSION_COLUMNS} FROM sessions WHERE session_id = ?",
            (session_id,),
        ).fetchone()
        return self._summary(row) if row is not None else None

    def list_sessions(self) -> list[SessionSummary]:
        # `rowid DESC` breaks ties when two sessions land on the same timestamp
        # tick (Windows clock granularity): the later INSERT sorts first, so
        # "newest first" stays deterministic even for identical created_at.
        rows = self._conn.execute(
            f"SELECT {_SESSION_COLUMNS} FROM sessions ORDER BY created_at DESC, rowid DESC"
        ).fetchall()
        return [self._summary(row) for row in rows]

    def first_user_texts(self) -> dict[str, str]:
        """Read only each session's first user message for sidebar titles."""
        rows = self._conn.execute(
            "SELECT s.session_id, (SELECT m.content FROM messages m "
            "WHERE m.session_id = s.session_id AND m.role = 'user' "
            "ORDER BY m.seq LIMIT 1) AS content FROM sessions s"
        ).fetchall()
        titles: dict[str, str] = {}
        for row in rows:
            if row["content"] is None:
                continue
            for block in json.loads(row["content"]):
                if block.get("type") == "text" and str(block.get("text", "")).strip():
                    titles[row["session_id"]] = str(block["text"]).strip().splitlines()[0][:72]
                    break
        return titles

    @staticmethod
    def _summary(row: sqlite3.Row) -> SessionSummary:
        return SessionSummary(
            session_id=row["session_id"],
            created_at=row["created_at"],
            workspace=row["workspace"],
            provider=row["provider"],
            model=row["model"],
            status=row["status"],
            exit_reason=row["exit_reason"],
            rounds=row["rounds"],
            input_tokens=row["input_tokens"],
            output_tokens=row["output_tokens"],
            cache_read_tokens=row["cache_read_tokens"],
            cache_write_tokens=row["cache_write_tokens"],
            usage_available=bool(row["usage_available"]),
        )

    # -- messages ----------------------------------------------------------

    def append_message(self, session_id: str, message: Message) -> int:
        content_json = json.dumps(message.model_dump()["content"], ensure_ascii=False)
        with self.transaction() as conn:
            self._require_session(conn, session_id)
            row = conn.execute(
                "SELECT COALESCE(MAX(seq) + 1, 0) AS next_seq FROM messages"
                " WHERE session_id = ?",
                (session_id,),
            ).fetchone()
            seq: int = row["next_seq"]
            conn.execute(
                "INSERT INTO messages (session_id, seq, role, content) VALUES (?, ?, ?, ?)",
                (session_id, seq, message.role, content_json),
            )
        return seq

    def get_messages(self, session_id: str) -> list[Message]:
        rows = self._conn.execute(
            "SELECT role, content FROM messages WHERE session_id = ? ORDER BY seq ASC",
            (session_id,),
        ).fetchall()
        # pydantic validates the discriminated block union on the way in.
        return [
            Message(role=row["role"], content=json.loads(row["content"])) for row in rows
        ]

    def replace_messages(self, session_id: str, messages: list[Message]) -> None:
        """Atomically rewrite the whole conversation (context compaction).

        Compaction archives the original content to artifacts first; this
        method then stores the compacted view so a resumed session sees
        exactly what the model last saw. Row order is renumbered 0..n-1.
        """
        with self.transaction() as conn:
            self._require_session(conn, session_id)
            conn.execute("DELETE FROM messages WHERE session_id = ?", (session_id,))
            for seq, message in enumerate(messages):
                content_json = json.dumps(
                    message.model_dump()["content"], ensure_ascii=False
                )
                conn.execute(
                    "INSERT INTO messages (session_id, seq, role, content)"
                    " VALUES (?, ?, ?, ?)",
                    (session_id, seq, message.role, content_json),
                )

    # -- artifacts (manifest; file bytes live in storage.artifacts.ArtifactStore) --

    def record_artifact(
        self, session_id: str, artifact_id: str, kind: str, relpath: str
    ) -> None:
        created_at = utc_now()
        with self.transaction() as conn:
            self._require_session(conn, session_id)
            conn.execute(
                "INSERT OR REPLACE INTO artifacts"
                " (session_id, artifact_id, kind, relpath, created_at)"
                " VALUES (?, ?, ?, ?, ?)",
                (session_id, artifact_id, kind, relpath, created_at),
            )

    def list_artifacts(self, session_id: str) -> list[dict[str, str]]:
        rows = self._conn.execute(
            "SELECT artifact_id, kind, relpath, created_at FROM artifacts"
            " WHERE session_id = ? ORDER BY created_at ASC, artifact_id ASC",
            (session_id,),
        ).fetchall()
        return [
            {
                "artifact_id": row["artifact_id"],
                "kind": row["kind"],
                "relpath": row["relpath"],
                "created_at": row["created_at"],
            }
            for row in rows
        ]

    # -- events ------------------------------------------------------------

    def append_event(
        self, session_id: str, type: EventType, data: dict[str, Any] | None = None
    ) -> Event:
        payload = data if data is not None else {}
        data_json = json.dumps(payload, ensure_ascii=False)
        timestamp = utc_now()
        with self.transaction() as conn:
            self._require_session(conn, session_id)
            row = conn.execute(
                "SELECT COALESCE(MAX(seq) + 1, 0) AS next_seq FROM events"
                " WHERE session_id = ?",
                (session_id,),
            ).fetchone()
            seq: int = row["next_seq"]
            conn.execute(
                "INSERT INTO events (session_id, seq, type, timestamp, data)"
                " VALUES (?, ?, ?, ?, ?)",
                (session_id, seq, type.value, timestamp, data_json),
            )
        return Event(seq=seq, type=type, timestamp=timestamp, data=payload)

    def get_events(self, session_id: str) -> list[Event]:
        rows = self._conn.execute(
            "SELECT seq, type, timestamp, data FROM events WHERE session_id = ?"
            " ORDER BY seq ASC",
            (session_id,),
        ).fetchall()
        return [
            Event(
                seq=row["seq"],
                type=EventType(row["type"]),
                timestamp=row["timestamp"],
                data=json.loads(row["data"]),
            )
            for row in rows
        ]
