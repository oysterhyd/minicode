"""Session-scoped, paginated artifact readback."""

from __future__ import annotations

import asyncio

from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.artifacts import ReadArtifactTool
from minicode.tools.base import ToolContext


def run(tool, args, ctx):
    return asyncio.run(tool.run(args, ctx))


def test_read_artifact_pages_and_reports_next_offset(tmp_path):
    store = SqliteStore(tmp_path / "sessions.db")
    try:
        session_id = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        artifacts = ArtifactStore(store)
        ref = artifacts.spill(session_id, "tool_output", "abcdefghij")
        ctx = ToolContext(
            workspace=tmp_path,
            artifact_store=artifacts,
            session_id=session_id,
        )

        first = run(ReadArtifactTool(), {"artifact_id": ref.artifact_id, "limit": 4}, ctx)
        second = run(
            ReadArtifactTool(),
            {"artifact_id": ref.artifact_id, "offset": 4, "limit": 20},
            ctx,
        )

        assert first.success is True
        assert "next_offset=4" in first.output and first.output.endswith("abcd")
        assert "· end" in second.output and second.output.endswith("efghij")
    finally:
        store.close()

def test_read_artifact_cannot_cross_session_boundary(tmp_path):
    store = SqliteStore(tmp_path / "sessions.db")
    try:
        owner = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        other = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        artifacts = ArtifactStore(store)
        ref = artifacts.spill(owner, "tool_output", "secret")
        ctx = ToolContext(workspace=tmp_path, artifact_store=artifacts, session_id=other)

        outcome = run(ReadArtifactTool(), {"artifact_id": ref.artifact_id}, ctx)

        assert outcome.success is False
        assert "unknown artifact for this session" in (outcome.error or "")
    finally:
        store.close()
