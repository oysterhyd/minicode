import asyncio
import sys
from pathlib import Path

import pytest

from minicode.configuration import HarnessConfiguration
from minicode.context.extensions import ProjectInstructions
from minicode.core.models import EventType
from minicode.instructions import GlobalInstructions
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeTurn
from minicode.runtime.loop import AgentRuntime
from minicode.security.policy import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.tools.registry import default_registry

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "desktop"))
import bridge as bridge_mod


def test_global_instructions_atomic_utf8_size_and_empty(tmp_path):
    instructions = GlobalInstructions(tmp_path / "config" / "AGENTS.md")
    assert instructions.read()["content"] == ""
    assert not instructions.read()["exists"]
    assert instructions.save("请使用中文。\n")["content"] == "请使用中文。\n"
    with pytest.raises(ValueError, match="64 KB"):
        instructions.save("汉" * 22000)
    assert instructions.read()["content"] == "请使用中文。\n"
    with pytest.raises(ValueError, match="string"):
        instructions.save(None)
    assert not list(instructions.path.parent.glob(".agents-*.tmp"))
    assert instructions.save("")["content"] == ""


def test_globals_reload_between_requests_and_resume_without_changing_project_scope(tmp_path):
    workspace = tmp_path / "project"
    workspace.mkdir()
    (workspace / "AGENTS.md").write_text("PROJECT RULE", encoding="utf-8")
    globals = GlobalInstructions(tmp_path / "config" / "AGENTS.md")
    globals.save("GLOBAL FIRST")
    store = SqliteStore(tmp_path / "sessions.db")
    instructions = ProjectInstructions(workspace, globals.path)
    runtime = AgentRuntime(provider=FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="first"), FakeTurn(text="second")])),
                           registry=default_registry(), store=store, policy=AutoAllowPolicy(), workspace=workspace,
                           provider_name="fake", model="fake-model", project_instructions=instructions)
    assert runtime.system_prompt.index("GLOBAL FIRST") < runtime.system_prompt.index("PROJECT RULE")

    async def scenario():
        result = await runtime.run_turn("first")
        globals.save("GLOBAL UPDATED")
        await runtime.run_turn("second")
        assert "GLOBAL UPDATED" in runtime.system_prompt
        assert "GLOBAL FIRST" not in runtime.system_prompt
        assert "PROJECT RULE" in runtime.system_prompt
        assert any(e.type is EventType.PROJECT_INSTRUCTIONS and e.data.get("scope") == "global"
                   for e in store.get_events(result.session_id))
        await runtime.aclose()
        globals.save("")
        resumed = AgentRuntime.resume(session_id=result.session_id, provider=FakeProvider(FakeProviderOptions()),
                                      registry=default_registry(), store=store, policy=AutoAllowPolicy(),
                                      workspace=workspace, provider_name="fake", model="fake-model",
                                      project_instructions=ProjectInstructions(workspace, globals.path))
        assert "GLOBAL UPDATED" not in resumed.system_prompt
        assert "PROJECT RULE" in resumed.system_prompt
        await resumed.aclose()
        (workspace / "AGENTS.md").write_text("CHANGED PROJECT", encoding="utf-8")
        with pytest.raises(ValueError, match="project instructions changed"):
            AgentRuntime.resume(session_id=result.session_id, provider=FakeProvider(FakeProviderOptions()),
                                registry=default_registry(), store=store, policy=AutoAllowPolicy(), workspace=workspace,
                                provider_name="fake", model="fake-model",
                                project_instructions=ProjectInstructions(workspace, globals.path))

    asyncio.run(scenario())
    store.close()


def test_bridge_global_editor_works_without_workspace(tmp_path, monkeypatch):
    config = HarnessConfiguration(tmp_path / "profile" / "desktop-config.json")
    monkeypatch.setattr(bridge_mod, "HarnessConfiguration", lambda: config)

    async def scenario():
        router = bridge_mod.BridgeRouter(SqliteStore(tmp_path / "sessions.db"))
        assert (await router.handle("getInstructions", {}))["content"] == ""
        saved = await router.handle("saveInstructions", {"content": "GLOBAL EDITOR"})
        assert saved["path"] == str(config.path.parent / "AGENTS.md")
        assert (await router.handle("getInstructions", {}))["content"] == "GLOBAL EDITOR"
        await router.aclose()

    asyncio.run(scenario())


def test_bridge_local_resource_switch_and_mcp_test_without_workspace(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    config = HarnessConfiguration(tmp_path / ".minicode" / "desktop-config.json")
    monkeypatch.setattr(bridge_mod, "HarnessConfiguration", lambda: config)
    source = tmp_path / ".agents" / "skills" / "local-check" / "SKILL.md"
    source.parent.mkdir(parents=True)
    source.write_text("---\nname: local-check\ndescription: Check local work\n---\nBODY NOT IN SCAN\n", encoding="utf-8")
    original = source.read_bytes()

    async def scenario():
        router = bridge_mod.BridgeRouter(SqliteStore(tmp_path / "sessions.db"))
        rows = (await router.handle("getLocalExtensions", {}))["entries"]
        assert len(rows) == 1 and rows[0]["scope"] == "global" and not rows[0]["enabled"]
        assert "BODY NOT IN SCAN" not in str(rows)
        with pytest.raises(ValueError, match="boolean"):
            await router.handle("setLocalExtensionEnabled", {"id": rows[0]["id"], "enabled": 1})
        selected = await router.handle("setLocalExtensionEnabled", {"id": rows[0]["id"], "enabled": True})
        assert selected["entries"][0]["enabled"]
        assert (await router.handle("refreshMcp", {}))["servers"] == []
        assert source.read_bytes() == original
        await router.aclose()

    asyncio.run(scenario())
