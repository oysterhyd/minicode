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

import uuid
from pathlib import Path

from pydantic import BaseModel

from minicode.storage.sqlite_store import SqliteStore

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

    def _session_dir(self, session_id: str) -> Path:
        return self._root / session_id

    def spill(self, session_id: str, kind: str, content: str) -> ArtifactRef:
        """Persist *content* as a new artifact and record it in the manifest."""
        artifact_id = f"{kind}_{uuid.uuid4().hex[:12]}"
        session_dir = self._session_dir(session_id)
        session_dir.mkdir(parents=True, exist_ok=True)
        target = session_dir / f"{artifact_id}.txt"
        tmp = target.with_suffix(".tmp")
        tmp.write_text(content, encoding="utf-8")
        tmp.replace(target)
        self._store.record_artifact(session_id, artifact_id, kind, target.name)
        return ArtifactRef(artifact_id=artifact_id, kind=kind, size=len(content))

    def read(self, session_id: str, artifact_id: str) -> str | None:
        """Return the full content of one artifact, or ``None`` if unknown."""
        relpath = self._relpath(session_id, artifact_id)
        if relpath is None:
            return None
        target = self._session_dir(session_id) / relpath
        try:
            return target.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            return None

    def _relpath(self, session_id: str, artifact_id: str) -> str | None:
        for entry in self._store.list_artifacts(session_id):
            if entry["artifact_id"] == artifact_id:
                return entry["relpath"]
        return None
