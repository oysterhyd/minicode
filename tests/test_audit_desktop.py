"""Desktop bridge lifecycle regressions using real runtime, no remote model."""
from __future__ import annotations

import asyncio

import pytest

from test_desktop_bridge_router import bridge_mod, scripted, store, workspace, events


def test_stale_draft_slash_preserves_running_runtime_and_cancel_route(monkeypatch, store, workspace, events):
    monkeypatch.setattr(bridge_mod, "provider_for", scripted())

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            started = await router.handle("sendPrompt", {"clientKey": "draft-audit", "sessionId": None,
                 "workspace": str(workspace), "model": "fake", "text": "do work"})
            sid = started["sessionId"]
            owner = router.sessions[sid]
            for _ in range(100):
                if owner.approvals:
                    break
                await asyncio.sleep(.01)
            assert owner.approvals
            previous = owner.runtime
            registry = owner.registry
            result = await router.handle("runSlash", {"clientKey": "draft-audit", "sessionId": None,
                    "workspace": str(workspace), "text": "/skill"})
            assert result["action"] == "skills"
            assert owner.runtime is previous and owner.registry is registry
            other = workspace / "other"
            other.mkdir()
            with pytest.raises((ValueError, RuntimeError)):
                await owner.ensure_runtime(other, None)
            assert owner.runtime is previous and owner.registry is registry
            assert await router.handle("cancelTurn", {"clientKey": "draft-audit", "sessionId": sid})
            await owner.run_task
            assert store.get_session(sid).exit_reason == "cancelled"
        finally:
            await router.aclose()
    asyncio.run(scenario())
