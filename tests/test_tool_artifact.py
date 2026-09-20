"""Tests for minicode.tools.artifact (read_artifact) — plain sync pytest
driving async with ``asyncio.run``; no pytest-asyncio. Uses a tiny fake
artifact store instead of SqliteStore."""

from __future__ import annotations

import asyncio
import uuid
from types import SimpleNamespace

from minicode.tools.artifact import ReadArtifactTool
from minicode.tools.base import ToolContext, ToolLimits


class FakeStore:
    """In-memory stand-in for storage.artifacts.ArtifactStore."""

    def __init__(self) -> None:
        self.data: dict[tuple[str, str], str] = {}

    def spill(self, session_id: str, kind: str, content: str) -> SimpleNamespace:
        artifact_id = f"{kind}_{uuid.uuid4().hex[:12]}"
        self.data[(session_id, artifact_id)] = content
        return SimpleNamespace(artifact_id=artifact_id, kind=kind, size=len(content))

    def read(self, session_id: str, artifact_id: str) -> str | None:
        return self.data.get((session_id, artifact_id))


def make_ctx(store: FakeStore | None = None, session_id: str | None = "sess-1") -> ToolContext:
    return ToolContext(
        workspace=SimpleNamespace(),  # unused by this tool
        limits=ToolLimits(max_output_chars=20_000),
        artifact_store=store,
        session_id=session_id,
    )


def run(tool: ReadArtifactTool, raw_args: dict, ctx: ToolContext):
    return asyncio.run(tool.run(raw_args, ctx))


# ---------------------------------------------------------------------------
# 1. Success: the exact spilled content comes back
# ---------------------------------------------------------------------------


def test_read_artifact_returns_full_content():
    store = FakeStore()
    ref = store.spill("sess-1", "tool_output", "FULL ORIGINAL OUTPUT")
    tool = ReadArtifactTool()

    outcome = run(tool, {"artifact_id": ref.artifact_id}, make_ctx(store))

    assert outcome.success is True
    assert outcome.error is None
    assert outcome.output == "FULL ORIGINAL OUTPUT"


# ---------------------------------------------------------------------------
# 2. Unknown id: clean failure naming the artifact
# ---------------------------------------------------------------------------


def test_read_artifact_unknown_id_fails():
    store = FakeStore()
    tool = ReadArtifactTool()

    outcome = run(tool, {"artifact_id": "archive_deadbeef01"}, make_ctx(store))

    assert outcome.success is False
    assert outcome.error == "unknown artifact: archive_deadbeef01"


# ---------------------------------------------------------------------------
# 3. Missing store or session: configuration failure
# ---------------------------------------------------------------------------


def test_read_artifact_without_store_fails():
    tool = ReadArtifactTool()
    outcome = run(tool, {"artifact_id": "archive_abc"}, make_ctx(store=None))
    assert outcome.success is False
    assert outcome.error == "no artifact store configured for this session"


def test_read_artifact_without_session_id_fails():
    store = FakeStore()
    tool = ReadArtifactTool()
    outcome = run(tool, {"artifact_id": "archive_abc"}, make_ctx(store, session_id=None))
    assert outcome.success is False
    assert outcome.error == "no artifact store configured for this session"


# ---------------------------------------------------------------------------
# 4. Output cap: stored content is truncated like any other tool output
# ---------------------------------------------------------------------------


def test_read_artifact_truncates_to_output_limit():
    store = FakeStore()
    ref = store.spill("sess-1", "tool_output", "x" * 500)
    tool = ReadArtifactTool()
    ctx = ToolContext(
        workspace=SimpleNamespace(),
        limits=ToolLimits(max_output_chars=100),
        artifact_store=store,
        session_id="sess-1",
    )

    outcome = run(tool, {"artifact_id": ref.artifact_id}, ctx)

    assert outcome.success is True
    assert outcome.output.startswith("x" * 100)
    assert "output truncated" in outcome.output
    assert len(outcome.output) < 200


# ---------------------------------------------------------------------------
# 5. Argument validation: malformed ids never reach execute
# ---------------------------------------------------------------------------


def test_read_artifact_rejects_malformed_id():
    tool = ReadArtifactTool()
    store = FakeStore()

    for bad in ("", "NOT-VALID", "has space", "UPPER_case", "semi;colon"):
        outcome = run(tool, {"artifact_id": bad}, make_ctx(store))
        assert outcome.success is False, bad
        assert "invalid arguments" in (outcome.error or ""), bad


def test_read_artifact_accepts_store_style_ids():
    # ids produced by ArtifactStore.spill: "<kind>_<hex>" with lowercase kind
    tool = ReadArtifactTool()
    outcome = run(
        tool,
        {"artifact_id": "tool_output_1a2b3c4d5e6f"},
        make_ctx(FakeStore()),
    )
    # reaches the store (unknown there) rather than failing validation
    assert outcome.error == "unknown artifact: tool_output_1a2b3c4d5e6f"
