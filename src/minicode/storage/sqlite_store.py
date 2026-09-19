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
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator, Protocol

from pydantic import BaseModel

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
SCHEMA_VERSION = 1

_SESSION_COLUMNS = (
    "session_id, created_at, workspace, provider, model, "
    "status, exit_reason, rounds, input_tokens, output_tokens"
)


def _utc_now() -> str:
    """UTC ISO-8601 timestamp with timezone offset (our storage convention)."""
    return datetime.now(timezone.utc).isoformat()


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
    ) -> None: ...

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
                output_tokens INTEGER NOT NULL DEFAULT 0
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
            """
        )
        # PRAGMAs cannot bind parameters; SCHEMA_VERSION is a module constant.
        self._conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")

    def close(self) -> None:
        """Close the underlying connection (tidy teardown in tests / CLI)."""
        self._conn.close()

    @contextmanager
    def _write_txn(self) -> Iterator[sqlite3.Connection]:
        """Explicit write transaction: BEGIN IMMEDIATE up front, commit on
        success, full rollback on any error."""
        self._conn.execute("BEGIN IMMEDIATE")
        try:
            yield self._conn
        except BaseException:
            self._conn.rollback()
            raise
        self._conn.commit()

    def _require_session(self, conn: sqlite3.Connection, session_id: str) -> None:
        row = conn.execute(
            "SELECT 1 FROM sessions WHERE session_id = ?", (session_id,)
        ).fetchone()
        if row is None:
            raise ValueError(f"unknown session: {session_id}")

    # -- sessions ----------------------------------------------------------

    def create_session(self, *, workspace: str, provider: str, model: str) -> str:
        session_id = uuid.uuid4().hex
        with self._write_txn() as conn:
            conn.execute(
                "INSERT INTO sessions (session_id, created_at, workspace, provider, model)"
                " VALUES (?, ?, ?, ?, ?)",
                (session_id, _utc_now(), workspace, provider, model),
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

        with self._write_txn() as conn:
            self._require_session(conn, session_id)
            if assignments:
                params.append(session_id)
                conn.execute(
                    f"UPDATE sessions SET {', '.join(assignments)} WHERE session_id = ?",
                    params,
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
        )

    # -- messages ----------------------------------------------------------

    def append_message(self, session_id: str, message: Message) -> int:
        content_json = json.dumps(message.model_dump()["content"], ensure_ascii=False)
        with self._write_txn() as conn:
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

    # -- events ------------------------------------------------------------

    def append_event(
        self, session_id: str, type: EventType, data: dict[str, Any] | None = None
    ) -> Event:
        payload = data if data is not None else {}
        data_json = json.dumps(payload, ensure_ascii=False)
        timestamp = _utc_now()
        with self._write_txn() as conn:
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
