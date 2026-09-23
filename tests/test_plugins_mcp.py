"""Local plugin validation and real stdio MCP execution."""

from __future__ import annotations

import asyncio
import json
import shutil
from pathlib import Path

import pytest

from minicode.context.extensions import SkillCatalog
from minicode.core.models import EventType, ToolResultBlock
from minicode.plugins import PluginCatalog
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.runtime.loop import AgentRuntime
from minicode.security.policy import ModePolicy, PolicyBehavior, PermissionMode
from minicode.security.policy import AutoAllowPolicy
from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.base import ToolContext
from minicode.tools.registry import default_registry


EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "mcp_docs"


def workspace_with_plugin(tmp_path):
    workspace = tmp_path / "repo"
    plugin = workspace / ".minicode" / "plugins" / "docs"
    plugin.mkdir(parents=True)
    shutil.copy(EXAMPLE / "plugin.json", plugin / "plugin.json")
    shutil.copy(EXAMPLE / "server.py", plugin / "server.py")
    (workspace / "docs").mkdir()
    (workspace / "docs" / "guide.md").write_text("hello docs", encoding="utf-8")
    return workspace, plugin


def test_plugin_lock_disabled_and_namespaced_skill(tmp_path):
    workspace, plugin = workspace_with_plugin(tmp_path)
    skill_dir = plugin / "skills" / "inspect"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: inspect\ndescription: Read docs\n---\nPrivate body\n", encoding="utf-8"
    )
    manifest = json.loads((plugin / "plugin.json").read_text(encoding="utf-8"))
    manifest["skills"] = "skills"
    (plugin / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    catalog = PluginCatalog(workspace)
    skills = SkillCatalog(workspace, user_root=tmp_path / "none", plugin_roots=catalog.skill_roots())
    assert "plugin:docs:inspect" in skills.listing()
    assert "Private body" not in skills.listing()
    assert "Private body" in skills.load("plugin:docs:inspect").content
    catalog.write_lock()
    assert PluginCatalog(workspace).plugins["docs"].digest == catalog.plugins["docs"].digest
    (plugin / "server.py").write_text("# changed\n", encoding="utf-8")
    with pytest.raises(ValueError, match="plugin lock differs"):
        PluginCatalog(workspace)
    manifest["enabled"] = False
    (plugin / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    disabled = PluginCatalog(workspace, check_lock=False)
    assert disabled.servers() == [] and disabled.skill_roots() == []


def test_plugin_rejects_server_name_collision(tmp_path):
    workspace, plugin = workspace_with_plugin(tmp_path)
    manifest = json.loads((plugin / "plugin.json").read_text(encoding="utf-8"))
    manifest["mcp_servers"].append(dict(manifest["mcp_servers"][0]))
    (plugin / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="duplicate or invalid MCP server"):
        PluginCatalog(workspace)


def test_plugin_agent_is_namespaced_and_lazily_loaded(tmp_path):
    workspace, plugin = workspace_with_plugin(tmp_path)
    agent = plugin / "agents" / "audit"
    agent.mkdir(parents=True)
    (agent / "AGENT.md").write_text(
        "---\nname: audit\ndescription: Inspect a module\n---\nAudit the requested module.\n",
        encoding="utf-8",
    )
    manifest = json.loads((plugin / "plugin.json").read_text(encoding="utf-8"))
    manifest["agents"] = "agents"
    (plugin / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    catalog = PluginCatalog(workspace)
    assert catalog.agent_names() == ["plugin:docs:audit"]
    instructions, source = catalog.agent_instructions("plugin:docs:audit")
    assert "Audit the requested module" in instructions
    assert source["plugin_version"] == "0.1.0"
    registry = default_registry(delegation=True, agent_kinds=catalog.agent_names())
    assert "plugin:docs:audit" in registry.get("delegate").description
    catalog.write_lock()
    (agent / "AGENT.md").write_text("changed", encoding="utf-8")
    with pytest.raises(ValueError, match="plugin content changed"):
        catalog.agent_instructions("plugin:docs:audit")


def test_plugin_agent_delegation_keeps_child_read_only(tmp_path):
    workspace, plugin = workspace_with_plugin(tmp_path)
    agent = plugin / "agents" / "audit"
    agent.mkdir(parents=True)
    (agent / "AGENT.md").write_text(
        "---\nname: audit\ndescription: Inspect a module\n---\nInspect only.\n",
        encoding="utf-8",
    )
    manifest = json.loads((plugin / "plugin.json").read_text(encoding="utf-8"))
    manifest["agents"] = "agents"
    (plugin / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
    catalog = PluginCatalog(workspace)
    registry = default_registry(delegation=True, agent_kinds=catalog.agent_names())
    registry.plugin_catalog = catalog
    report = json.dumps({"summary": "No change", "findings": [],
                         "evidence_refs": [], "unresolved": []})
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="delegate", arguments={
            "kind": "plugin:docs:audit", "task": "inspect source",
        })]),
        FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={
            "path": "forbidden.txt", "content": "bad",
        })]),
        FakeTurn(text=report),
        FakeTurn(text="done"),
    ]))
    store = SqliteStore(tmp_path / "sessions.db")
    try:
        runtime = AgentRuntime(
            provider=provider, registry=registry, store=store,
            policy=AutoAllowPolicy(), workspace=workspace,
            provider_name="fake", model="fake",
        )
        result = asyncio.run(runtime.run_turn("audit"))
        parent_events = store.get_events(result.session_id)
        child_start = next(event for event in parent_events
                           if event.type is EventType.SUBAGENT_START)
        assert child_start.data["source"]["plugin"] == "docs"
        child_id = child_start.data["child_session_id"]
        child_blocks = [b for message in store.get_messages(child_id)
                        for b in message.content if isinstance(b, ToolResultBlock)]
        assert any("unknown tool: write" in block.content for block in child_blocks)
        assert not (workspace / "forbidden.txt").exists()
    finally:
        store.close()


def test_stdio_mcp_tools_use_policy_events_and_artifacts(tmp_path):
    workspace, _ = workspace_with_plugin(tmp_path)
    (workspace / "docs" / "large.md").write_text("A" * 60000, encoding="utf-8")
    catalog = PluginCatalog(workspace)
    registry = default_registry()
    for server in catalog.servers():
        registry.add_mcp_server(server)
    policy = ModePolicy(PermissionMode.DEFAULT)
    assert asyncio.run(policy.check("mcp__docs__read_document", {})).behavior is PolicyBehavior.ASK

    async def run():
        statuses = await registry.prepare()
        assert statuses[0]["error"] is None
        assert {"mcp__docs__list_documents", "mcp__docs__read_document"} <= set(registry.names())
        output = await registry.get("mcp__docs__list_documents").run({}, ToolContext(workspace=workspace))
        assert output.success and "guide.md" in output.output
        provider = FakeProvider(FakeProviderOptions(turns=[
            FakeTurn(tool_calls=[FakeToolCall(name="mcp__docs__read_document", arguments={
                "path": "large.md", "limit": 60000,
            })]),
            FakeTurn(text="done"),
        ]))
        store = SqliteStore(tmp_path / "sessions.db")
        try:
            runtime = AgentRuntime(
                provider=provider, registry=registry, store=store,
                policy=ModePolicy(PermissionMode.BYPASS), workspace=workspace,
                provider_name="fake", model="fake", artifact_store=ArtifactStore(store),
            )
            result = await runtime.run_turn("read document")
            events = store.get_events(result.session_id)
            calls = [e for e in events if e.type is EventType.TOOL_CALL_RESULT]
            assert calls[0].data["source"]["plugin"] == "docs"
            assert calls[0].data["artifact_id"] is not None
            assert any(e.type is EventType.MCP_DISCOVERY for e in events)
            blocks = [b for message in store.get_messages(result.session_id)
                      for b in message.content if isinstance(b, ToolResultBlock)]
            assert "[artifact:" in blocks[0].content
        finally:
            store.close()
            await registry.aclose()

    asyncio.run(run())


def test_stdio_mcp_disconnect_becomes_an_explainable_tool_failure(tmp_path):
    workspace, plugin = workspace_with_plugin(tmp_path)
    (plugin / "server.py").write_text(
        "from mcp.server import MCPServer\n"
        "import os\n"
        "mcp = MCPServer('crasher')\n"
        "@mcp.tool()\n"
        "def crash() -> str:\n"
        "    os._exit(1)\n"
        "if __name__ == '__main__':\n"
        "    mcp.run()\n",
        encoding="utf-8",
    )
    registry = default_registry()
    registry.add_mcp_server(PluginCatalog(workspace).servers()[0])

    async def run():
        try:
            statuses = await registry.prepare()
            assert statuses[0]["error"] is None
            result = await registry.get("mcp__docs__crash").run(
                {}, ToolContext(workspace=workspace)
            )
            assert not result.success
            assert "disconnected or timed out" in (result.error or "")
        finally:
            await registry.aclose()

    asyncio.run(run())
