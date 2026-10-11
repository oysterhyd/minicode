"""Discovery is inert; explicit availability reaches the real runtime."""
import asyncio
import json
import sys
from pathlib import Path

import pytest

from minicode.configuration import HarnessConfiguration
from minicode.context.extensions import SkillCatalog
from minicode.local_extensions import LocalExtensionCatalog
from minicode.runtime.services import workspace_services
from minicode.tools.base import ToolContext


def setup(tmp_path):
    home, workspace = tmp_path / "home", tmp_path / "repo"
    home.mkdir()
    workspace.mkdir()
    config = HarnessConfiguration(home / ".minicode" / "desktop-config.json")
    return home, workspace, config, LocalExtensionCatalog(workspace, home=home, configuration=config)


def skill(root, name="audit"):
    path = root / name / "SKILL.md"
    path.parent.mkdir(parents=True)
    path.write_text(f"---\nname: {name}\ndescription: Inspect changes\n---\nPRIVATE BODY\n", encoding="utf-8")
    return path


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def test_scan_activation_is_local_lazy_and_source_preserving(tmp_path):
    home, workspace, config, catalog = setup(tmp_path)
    original = skill(home / ".agents" / "skills")
    native = skill(workspace / ".minicode" / "skills", "native")
    entry = next(e for e in catalog.scan() if e["name"] == "audit")
    before = original.read_bytes()
    assert entry["enabled"] is False and entry["scope"] == "global"
    assert "PRIVATE BODY" not in json.dumps(catalog.scan())
    selected = catalog.set_enabled(entry["id"], True)
    assert next(e for e in selected if e["id"] == entry["id"])["enabled"]
    skills = SkillCatalog(workspace, user_root=home / ".minicode" / "skills")
    catalog.apply_skills(skills)
    alias = "local:" + entry["id"].split(":")[1][:12] + ":audit"
    assert alias in skills.skills and "PRIVATE BODY" not in skills.listing()
    assert "PRIVATE BODY" in skills.load(alias).content
    native_entry = next(e for e in catalog.scan() if e["path"] == str(native))
    assert native_entry["enabled"]
    catalog.set_enabled(native_entry["id"], False)
    skills = SkillCatalog(workspace, user_root=home / ".minicode" / "skills")
    catalog.apply_skills(skills)
    assert "native" not in skills.skills
    catalog.set_enabled(entry["id"], False)
    assert original.read_bytes() == before
    assert config.read()["services"] == []


def test_mcp_discovery_never_connects_and_never_returns_credentials(tmp_path):
    home, workspace, config, catalog = setup(tmp_path)
    source = home / ".agents" / "servers" / "remote.json"
    write(source, {"id": "remote", "transport": "http", "url": "https://example.invalid/mcp?token=SECRET",
                   "headers": {"Authorization": "Bearer SECRET"}})
    row = catalog.scan()[0]
    assert row["supported"] and not row["enabled"]
    assert "SECRET" not in json.dumps(row)
    assert catalog.servers() == []
    catalog.set_enabled(row["id"], True)
    server = catalog.servers()[0]
    assert server.transport == "http" and server.headers["Authorization"] == "Bearer SECRET"
    assert server.url.endswith("token=SECRET")
    before = source.read_bytes()
    assert before == source.read_bytes()
    source.write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError, match="configuration changed"):
        server.verify()


def test_foreign_plugin_resources_activate_as_bundle_hooks_are_not_executed(tmp_path):
    home, workspace, config, catalog = setup(tmp_path)
    root = home / ".claude/plugins/cache/store/docs/1.0.0"
    write(root / ".claude-plugin/plugin.json", {"name": "docs", "description": "Local docs", "hooks": "hooks.json"})
    path = skill(root / "skills")
    write(root / ".mcp.json", {"mcpServers": {"docs": {"command": "python", "args": ["${CLAUDE_PLUGIN_ROOT}/server.py"]}}})
    write(root / "hooks.json", {"shouldNeverRun": True})
    rows = catalog.scan()
    parent = next(e for e in rows if e["kind"] == "plugin")
    assert parent["supported"] and not parent["enabled"]
    assert not catalog.servers()
    catalog.set_enabled(parent["id"], True)
    server = catalog.servers()[0]
    assert server.args == (str(root) + "/server.py",)
    skills = SkillCatalog(workspace, user_root=home / "none")
    catalog.apply_skills(skills)
    assert next(s for s in skills.skills.values() if s.name.endswith(":audit")).path == path
    child = next(e for e in catalog.scan() if e["kind"] == "skill")
    with pytest.raises(ValueError, match="parent plugin"):
        catalog.set_enabled(child["id"], False)
    catalog.set_enabled(parent["id"], False)
    assert catalog.servers() == []


def test_unsupported_resources_cannot_activate_and_broken_files_do_not_hide_valid_ones(tmp_path):
    home, workspace, config, catalog = setup(tmp_path)
    write(home / ".agents/servers/bad.json", {"id": "bad", "transport": "websocket", "url": "ws://x"})
    invalid = skill(home / ".agents/skills", "broken")
    invalid.write_text("---\nname: [broken\n---\n", encoding="utf-8")
    skill(home / ".agents/skills", "good")
    rows = catalog.scan()
    assert any(e["name"] == "good" and e["supported"] for e in rows)
    unsupported = next(e for e in rows if e["kind"] == "mcp")
    with pytest.raises(ValueError, match="incompatible"):
        catalog.set_enabled(unsupported["id"], True)
    assert not config.path.exists()


@pytest.mark.parametrize("transport", [{"type": "stdio"}, ["stdio"], None, 7, True])
def test_malformed_transport_never_blocks_other_resource_discovery(tmp_path, transport):
    home, workspace, config, catalog = setup(tmp_path)
    write(home / ".agents/servers/bad.json", {"id": "bad", "transport": transport, "command": "python"})
    write(home / ".agents/servers/good.json", {"id": "good", "transport": "stdio", "command": "python"})
    rows = catalog.scan()
    bad = next(e for e in rows if e["name"] == "bad")
    good = next(e for e in rows if e["name"] == "good")
    assert not bad["supported"] and good["supported"]
    with pytest.raises(ValueError, match="incompatible"):
        catalog.set_enabled(bad["id"], True)
    catalog.set_enabled(good["id"], True)
    assert len(catalog.servers()) == 1


def test_selected_stdio_mcp_reaches_workspace_registry_and_real_sdk(tmp_path, monkeypatch):
    home, workspace, config, catalog = setup(tmp_path)
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    server = Path(__file__).resolve().parents[1] / "examples/mcp_docs/server.py"
    (workspace / "docs").mkdir()
    (workspace / "docs/guide.md").write_text("hello", encoding="utf-8")
    source = home / ".agents/servers/docs.json"
    write(source, {"id": "docs", "transport": "stdio", "command": sys.executable,
                   "args": [str(server)], "env": {"LOCAL_RESOURCE_TEST": "true"}})
    assert not workspace_services(workspace, configuration=config).registry._mcp
    row = catalog.scan()[0]
    original = source.read_bytes()
    catalog.set_enabled(row["id"], True)
    services = workspace_services(workspace, configuration=config)
    connector = services.registry._mcp[0]

    async def run():
        try:
            statuses = await services.registry.prepare()
            assert statuses[0]["error"] is None
            name = f"mcp__{connector.config.name}__list_documents"
            result = await services.registry.get(name).run({}, ToolContext(workspace=workspace))
            assert result.success and "guide.md" in result.output
        finally:
            await services.registry.aclose()

    asyncio.run(run())
    assert source.read_bytes() == original
    catalog.set_enabled(row["id"], False)
    assert not workspace_services(workspace, configuration=config).registry._mcp


def test_home_scan_inline_plugin_and_missing_environment_remain_disableable(tmp_path):
    home, workspace, config, _ = setup(tmp_path)
    root = home / ".codex/plugins/cache/store/docs/1.0.0"
    write(root / ".codex-plugin/plugin.json", {"name": "docs", "mcpServers": {
        "inline": {"command": "python", "args": ["${CODEX_PLUGIN_ROOT}/server.py"],
                   "env": {"AUTH": "${MINICODE_TEST_NONEXISTENT_VARIABLE}"}}}})
    catalog = LocalExtensionCatalog(home, home=home, configuration=config)
    rows = catalog.scan()
    assert all(e["scope"] == "global" for e in rows)
    parent = next(e for e in rows if e["kind"] == "plugin")
    assert "inline" in parent["detail"]
    catalog.set_enabled(parent["id"], True)
    server = catalog.servers()[0]
    assert server.args == (str(root) + "/server.py",)
    with pytest.raises(ValueError, match="requires environment variable"):
        server.verify()
    catalog.set_enabled(parent["id"], False)
    assert catalog.servers() == []


def test_pi_desktop_installed_manifests_are_listed_without_running_hooks(tmp_path):
    home, workspace, config, catalog = setup(tmp_path)
    root = home / ".pi-desktop/plugins/installed/local.docs"
    write(root / "manifest.json", {"id": "local.docs", "name": "Local docs", "main": "main.js",
          "contributes": {"mcpServers": [{"id": "docs", "transport": "stdio", "command": "python", "args": []}]}})
    other = home / ".pi-desktop/plugins/installed/local.view"
    write(other / "manifest.json", {"id": "local.view", "name": "View only", "main": "main.js",
                                   "contributes": {"views": [{"id": "panel"}]}})
    rows = catalog.scan()
    parent = next(e for e in rows if e["kind"] == "plugin" and e["name"] == "Local docs")
    assert parent["supported"] and "docs" in parent["detail"]
    view = next(e for e in rows if e["name"] == "View only")
    assert not view["supported"]
    catalog.set_enabled(parent["id"], True)
    assert catalog.servers()[0].plugin == "local:Pi Desktop"


def test_imported_mcp_errors_do_not_expose_authenticated_urls(tmp_path, monkeypatch):
    from minicode.tools.mcp import McpConnector
    from minicode.tools.registry import default_registry

    home, workspace, config, catalog = setup(tmp_path)
    write(home / ".agents/servers/remote.json", {"id": "remote", "transport": "http",
          "url": "https://example.invalid/mcp?token=secret-should-stay-private"})
    catalog.set_enabled(catalog.scan()[0]["id"], True)
    server = catalog.servers()[0]
    connector = McpConnector(server)

    async def fail(*args):
        raise ValueError("https://example.invalid/mcp?token=secret-should-stay-private")

    monkeypatch.setattr(connector, "_request", fail)
    result = asyncio.run(connector.call("test", {}))
    assert "secret-should-stay-private" not in result.error
    registry = default_registry()
    registry.add_mcp_server(server)
    monkeypatch.setattr(registry._mcp[0], "discover", fail)

    async def discover():
        try:
            statuses = await registry.prepare()
            assert "secret-should-stay-private" not in json.dumps(statuses)
        finally:
            await registry.aclose()

    asyncio.run(discover())


@pytest.mark.parametrize("foreign", [True, False])
def test_resume_after_selected_skill_is_disabled_and_alias_stays_stable(tmp_path, monkeypatch, foreign):
    from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
    from minicode.runtime.loop import AgentRuntime
    from minicode.security.policy import AutoAllowPolicy
    from minicode.storage import SqliteStore

    home, workspace, config, catalog = setup(tmp_path)
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
    root = home / ".agents/skills" if foreign else workspace / ".minicode/skills"
    path = skill(root)
    row = next(e for e in catalog.scan() if e["kind"] == "skill")
    catalog.set_enabled(row["id"], True)
    services = workspace_services(workspace, configuration=config)
    alias = next(name for name, value in services.skills.skills.items() if value.path == path)
    store = SqliteStore(tmp_path / "sessions.db")
    runtime = AgentRuntime(provider=FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="skill_load", arguments={"name": alias})]),
        FakeTurn(text="done")])), registry=services.registry, store=store,
        policy=AutoAllowPolicy(), workspace=workspace, provider_name="fake", model="fake-model",
        skills=services.skills, project_instructions=services.instructions)
    result = asyncio.run(runtime.run_turn("inspect"))
    assert "PRIVATE BODY" in runtime._system_prompt
    store.close()
    catalog.set_enabled(row["id"], False)
    fresh = workspace_services(workspace, configuration=config)
    assert alias in fresh.skills.disabled_skills and alias not in fresh.skills.skills
    store = SqliteStore(tmp_path / "sessions.db")
    try:
        resumed = AgentRuntime.resume(store=store, session_id=result.session_id,
            provider=FakeProvider(FakeProviderOptions()), registry=fresh.registry,
            policy=AutoAllowPolicy(), workspace=workspace, provider_name="fake", model="fake-model",
            skills=fresh.skills, project_instructions=fresh.instructions)
        assert "PRIVATE BODY" not in resumed._system_prompt
        # Re-enabling preserves the persisted alias and still checks changed bodies.
        catalog.set_enabled(row["id"], True)
        enabled = workspace_services(workspace, configuration=config)
        assert alias in enabled.skills.skills
        path.write_text(path.read_text(encoding="utf-8").replace("PRIVATE BODY", "CHANGED BODY"), encoding="utf-8")
        with pytest.raises(ValueError, match="skill.*changed"):
            AgentRuntime.resume(store=store, session_id=result.session_id,
                provider=FakeProvider(FakeProviderOptions()), registry=enabled.registry,
                policy=AutoAllowPolicy(), workspace=workspace, provider_name="fake", model="fake-model",
                skills=enabled.skills, project_instructions=enabled.instructions)
    finally:
        store.close()


@pytest.mark.parametrize("transport", ["http", "sse"])
def test_remote_mcp_transport_uses_sdk_and_keeps_headers(tmp_path, transport):
    import socket
    import uvicorn
    from mcp.server import MCPServer
    from starlette.responses import Response
    from minicode.tools.mcp import McpConnector

    home, workspace, config, catalog = setup(tmp_path)
    server = MCPServer("remote-test")

    @server.tool()
    def echo(value: str) -> str:
        return value

    app = server.streamable_http_app() if transport == "http" else server.sse_app()
    seen = []

    class AuthMiddleware:
        def __init__(self, app):
            self.app = app

        async def __call__(self, scope, receive, send):
            if scope["type"] == "http":
                authorization = dict(scope["headers"]).get(b"authorization", b"").decode()
                seen.append(authorization)
                if authorization != "Bearer secret-test":
                    return await Response(status_code=401)(scope, receive, send)
            return await self.app(scope, receive, send)

    app.add_middleware(AuthMiddleware)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
        write(home / ".agents/servers/remote.json", {"id": "remote", "transport": transport,
              "url": f"http://127.0.0.1:{port}/" + ("mcp" if transport == "http" else "sse"),
              "headers": {"Authorization": "Bearer secret-test"}})
        catalog.set_enabled(catalog.scan()[0]["id"], True)
        connector = McpConnector(catalog.servers()[0])
        host = uvicorn.Server(uvicorn.Config(app, log_level="error", lifespan="on"))

        async def run():
            task = asyncio.create_task(host.serve(sockets=[listener]))
            try:
                async with asyncio.timeout(10):
                    while not host.started:
                        if task.done():
                            await task
                        await asyncio.sleep(0.01)
                tools = await connector.discover()
                assert tools[0].remote_name == "echo"
                result = await connector.call("echo", {"value": "remote response"})
                assert result.success and "remote response" in result.output
                assert seen and set(seen) == {"Bearer secret-test"}
            finally:
                await connector.aclose()
                host.should_exit = True
                await asyncio.wait_for(task, 10)

        asyncio.run(run())
