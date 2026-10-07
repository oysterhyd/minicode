"""Session artifacts: full tool outputs and archived history kept on disk.

Context compaction keeps the model-facing message list small by replacing
large or old content with a short preview plus an artifact reference. The
original bytes are stored here — one directory per session under
``<db dir>/artifacts/<session_id>/`` — so nothing is lost and the model (or a
human reviewing the report) can read the full content back on demand.

The manifest (id, kind, relative path) lives in the ``artifacts`` table; the
files themselves are plain UTF-8 text written atomically (write temp file,
then :meth:`Path.replace`).
"""

from __future__ import annotations

import asyncio
import codecs
import locale
import os
import uuid
from pathlib import Path
from typing import BinaryIO

from pydantic import BaseModel

from minicode.storage.sqlite_store import SqliteStore
from minicode.storage.paging import TextPageReader

__all__ = ["ArtifactRef", "ArtifactStore"]


class ArtifactRef(BaseModel):
    """Reference to one stored artifact; safe to embed in model-facing text."""

    artifact_id: str
    kind: str
    size: int


class ArtifactStore:
    """File-backed artifact storage for one database."""

    def __init__(self, store: SqliteStore) -> None:
        self._store = store
        self._root = store._db_path.parent / "artifacts"
        self._pages = TextPageReader()

    def _session_dir(self, session_id: str) -> Path:
        return self._root / session_id

    def spill(self, session_id: str, kind: str, content: str) -> ArtifactRef:
        """Persist *content* as a new artifact and record it in the manifest."""
        artifact_id = f"{kind}_{uuid.uuid4().hex[:12]}"
        session_dir = self._session_dir(session_id)
        session_dir.mkdir(parents=True, exist_ok=True)
        target = session_dir / f"{artifact_id}.txt"
        tmp = target.with_suffix(".tmp")
        try:
            with tmp.open("w", encoding="utf-8", newline="") as stream:
                stream.write(content)
                stream.flush()
                os.fsync(stream.fileno())
            tmp.replace(target)
            self._store.record_artifact(session_id, artifact_id, kind, target.name)
        except BaseException:
            tmp.unlink(missing_ok=True)
            target.unlink(missing_ok=True)
            raise
        return ArtifactRef(artifact_id=artifact_id, kind=kind, size=len(content))

    async def spill_binary_stream(
        self, session_id: str, kind: str, source: BinaryIO
    ) -> ArtifactRef:
        """Store a captured process log without loading the full log in memory."""
        artifact_id = f"{kind}_{uuid.uuid4().hex[:12]}"
        session_dir = self._session_dir(session_id)
        session_dir.mkdir(parents=True, exist_ok=True)
        target = session_dir / f"{artifact_id}.txt"
        tmp = target.with_suffix(".tmp")

        def copy() -> int:
            source.seek(0)
            sample = source.read(8192)
            try:
                # A bounded sample may end inside a valid UTF-8 character.
                codecs.getincrementaldecoder("utf-8")().decode(sample, final=False)
                encoding = "utf-8"
            except UnicodeDecodeError:
                encoding = locale.getpreferredencoding(False) or "utf-8"
            source.seek(0)
            decoder = codecs.getincrementaldecoder(encoding)(errors="replace")
            size = 0
            with tmp.open("w", encoding="utf-8", newline="") as output:
                while chunk := source.read(64 * 1024):
                    decoded = decoder.decode(chunk)
                    output.write(decoded)
                    size += len(decoded)
                tail = decoder.decode(b"", final=True)
                output.write(tail)
                size += len(tail)
                output.flush()
                os.fsync(output.fileno())
            return size

        copy_task = asyncio.create_task(asyncio.to_thread(copy))
        try:
            size = await asyncio.shield(copy_task)
            tmp.replace(target)
            self._store.record_artifact(session_id, artifact_id, kind, target.name)
        except asyncio.CancelledError:
            while not copy_task.done():
                try:
                    await asyncio.shield(copy_task)
                except asyncio.CancelledError:
                    continue
                except Exception:
                    break
            if copy_task.done() and not copy_task.cancelled():
                copy_task.exception()
            tmp.unlink(missing_ok=True)
            raise
        except BaseException:
            tmp.unlink(missing_ok=True)
            target.unlink(missing_ok=True)
            raise
        return ArtifactRef(artifact_id=artifact_id, kind=kind, size=size)

    def read(self, session_id: str, artifact_id: str) -> str | None:
        """Return the full content of one artifact, or ``None`` if unknown."""
        relpath = self._relpath(session_id, artifact_id)
        if relpath is None:
            return None
        target = self._session_dir(session_id) / relpath
        try:
            with target.open("r", encoding="utf-8", newline="") as handle:
                return handle.read()
        except (OSError, UnicodeDecodeError):
            return None

    def read_page(
        self, session_id: str, artifact_id: str, offset: int, limit: int
    ) -> tuple[str, int | None, bool] | None:
        """Read only a character page; total is unknown until the last page."""
        relpath = self._relpath(session_id, artifact_id)
        if relpath is None:
            return None
        target = self._session_dir(session_id) / relpath
        return self._read_page_path(target, offset, limit)

    async def aread_page(
        self, session_id: str, artifact_id: str, offset: int, limit: int
    ) -> tuple[str, int | None, bool] | None:
        """Resolve manifest on the owner thread, then read large pages off-loop."""
        relpath = self._relpath(session_id, artifact_id)
        if relpath is None:
            return None
        target = self._session_dir(session_id) / relpath
        return await asyncio.to_thread(self._read_page_path, target, offset, limit)

    def _read_page_path(
        self, target: Path, offset: int, limit: int
    ) -> tuple[str, int | None, bool] | None:
        return self._pages.read(target, offset, limit)

    def _relpath(self, session_id: str, artifact_id: str) -> str | None:
        return self._store.get_artifact_relpath(session_id, artifact_id)
