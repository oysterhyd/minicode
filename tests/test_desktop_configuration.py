"""Desktop configuration persistence, live changes and delegate execution."""
import asyncio
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "desktop"))
import bridge as bridge_mod
from minicode.configuration import HarnessConfiguration
from minicode.core.models import Usage
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.storage import SqliteStore


@pytest.fixture
def configuration(tmp_path, monkeypatch):
    config = HarnessConfiguration(tmp_path / "desktop.json")
    config.write(dict(services=[], agents=[], disabledAgents=[], defaultModel=""))
    monkeypatch.setattr(bridge_mod, "HarnessConfiguration", lambda: config)
    return config


def service(name="Gateway", model="same-model"):
    return dict(id="", name=name, baseUrl="https://example.test/v1", apiStyle="openai", apiKey="test-private-key",
                enabled=True, models=[dict(modelId=model, name=model, contextWindow=1000000, maxOutputTokens=32000, supportsEffort=True)])


def test_fresh_configuration_is_empty_even_with_provider_environment(tmp_path, monkeypatch):
    monkeypatch.setenv("ANTHROPIC_API_KEY", "test-anthropic-placeholder")
    monkeypatch.setenv("COMMANDCODE_API_KEY", "test-commandcode-placeholder")
    config = HarnessConfiguration(tmp_path / "fresh.json")
    monkeypatch.setattr(bridge_mod, "HarnessConfiguration", lambda: config)
    assert config.read()["services"] == []
    assert config.models() == []
    assert config.public()["defaultModel"] == ""
    assert not config.path.exists()
    b = bridge_mod.Bridge(SqliteStore(":memory:"))
    assert b.state()["model"] == ""
    b.store.close()
    config.save_service(service())
    assert len(config.models()) == 1
    assert config.read()["defaultModel"] == config.models()[0]["id"]
    assert HarnessConfiguration(config.path).public() == config.public()


def test_removing_last_service_clears_idle_model(configuration):
    configuration.save_service(service())
    async def scenario():
        router = bridge_mod.BridgeRouter(SqliteStore(":memory:"))
        initial = await router.handle("initialize", dict(clientKey="blank"))
        assert initial["state"]["model"]
        await router.handle("deleteService", dict(clientKey="blank", id=configuration.read()["services"][0]["id"]))
        state = await router.handle("getState", dict(clientKey="blank"))
        assert state["model"] == ""
        assert configuration.read()["defaultModel"] == ""
        await router.aclose()
    asyncio.run(scenario())


def test_service_persistence_and_secret_redaction(configuration):
    public = configuration.save_service(service())
    assert "test-private-key" not in json.dumps(public)
    draft = public["services"][0]
    assert draft["hasApiKey"] and draft["apiKey"] == ""
    draft["models"][0]["contextWindow"] = 500000
    configuration.save_service(draft)
    assert configuration.read()["services"][0]["apiKey"] == "test-private-key"
    assert HarnessConfiguration(configuration.path).models()[0]["contextWindow"] == 500000
    configuration.save_service(service("Other"))
    assert len({m["id"] for m in configuration.models()}) == 2
    with pytest.raises(ValueError, match="输出上限"):
        draft["models"][0]["maxOutputTokens"] = 500000
        configuration.save_service(draft)


def test_provider_uses_service_endpoint_and_model_limits(configuration):
    configuration.save_service(service())
    b = bridge_mod.Bridge(SqliteStore(":memory:"))
    provider = b.make_provider(configuration.models()[0]["id"])
    assert provider.model == "same-model"
    assert provider.context_window == 1000000 and provider.max_tokens == 32000
    assert provider.base_url == "https://example.test/v1"
    assert provider.api_key == "test-private-key"
    b.store.close()


def test_custom_agent_validation_and_rename(configuration):
    draft = dict(name="docs", label="文档审查", description="审查文档", instructions="检查文档", tools=["read", "grep"], inheritTools=False)
    configuration.save_agent(draft)
    with pytest.raises(ValueError, match="已存在"):
        configuration.save_agent(draft)
    configuration.save_agent({**draft, "name": "docs-v2", "originalName": "docs"})
    assert "docs" not in {a["name"] for a in configuration.agents()}
    with pytest.raises(ValueError, match="名称"):
        configuration.save_agent({**draft, "name": "../escape"})


def test_live_model_change_does_not_close_inflight_provider(configuration, tmp_path, monkeypatch):
    async def scenario():
        started = asyncio.Event()
        release = asyncio.Event()
        class SlowProvider(FakeProvider):
            closed = False
            async def stream(self, **kwargs):
                started.set()
                await release.wait()
                assert not self.closed
                async for event in super().stream(**kwargs):
                    yield event
            async def aclose(self):
                self.closed = True
        slow = SlowProvider(FakeProviderOptions(turns=[FakeTurn(tool_calls=[FakeToolCall(name="ls", arguments={"path": "."})])]))
        fast = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="new model response")]))
        fast.reasoning_effort = None
        monkeypatch.setattr(bridge_mod, "provider_for", lambda model, effort: slow if model == "fake" else fast)
        b = bridge_mod.Bridge(SqliteStore(tmp_path / "live.sqlite"))
        await b.handle("sendPrompt", dict(text="go", workspace=str(tmp_path), model="fake"))
        await started.wait()
        state = await b.handle("setModel", {"model": bridge_mod.DEFAULT_MODEL})
        assert state["model"] == bridge_mod.DEFAULT_MODEL and state["activeModel"] == "fake"
        assert state["pendingSettings"] and not slow.closed
        await b.handle("setEffort", {"effort": "high"})
        await b.handle("setPermissionMode", {"mode": "bypass"})
        await b.handle("setBudget", dict(max_rounds=8, max_total_tokens=20000, max_seconds=30))
        assert b.busy()
        release.set()
        await asyncio.wait_for(b.run_task, 5)
        assert slow.closed and b.runtime.model == bridge_mod.DEFAULT_MODEL
        assert b.runtime._messages[-1].content[0].text == "new model response"
        assert b.state()["budget"]["max_rounds"] == 8
        await b.discard_runtime()
        b.store.close()
    asyncio.run(scenario())


def test_custom_delegate_tools_and_instructions_reach_runtime(configuration, tmp_path, monkeypatch):
    configuration.save_agent(dict(name="docs", label="Docs", description="Review documents", instructions="LOOK_FOR_DOCUMENT_DEFECTS", tools=["read", "grep"], inheritTools=False))
    child_policies = []
    original_run = bridge_mod.AgentRuntime.run_turn
    async def capture_child_policy(runtime, text):
        if not runtime._allow_delegation:
            child_policies.append(runtime._policy)
        return await original_run(runtime, text)
    monkeypatch.setattr(bridge_mod.AgentRuntime, 'run_turn', capture_child_policy)
    class RecordingProvider(FakeProvider):
        async def stream(self, **kwargs):
            if "LOOK_FOR_DOCUMENT_DEFECTS" in (kwargs["system"] or ""):
                assert {tool.name for tool in kwargs["tools"]} == {"read", "grep"}
            async for event in super().stream(**kwargs):
                yield event
    provider = RecordingProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="delegate", arguments={"kind": "docs", "task": "review"})]),
        FakeTurn(text=json.dumps(dict(summary="reviewed", findings=[], evidence_refs=[], unresolved=[]))),
        FakeTurn(text="done"),
    ]))
    monkeypatch.setattr(bridge_mod, "provider_for", lambda *_: provider)
    async def scenario():
        b = bridge_mod.Bridge(SqliteStore(tmp_path / "delegate.sqlite"))
        await b.handle("sendPrompt", dict(text="review", workspace=str(tmp_path), model="fake"))
        await asyncio.wait_for(b.run_task, 5)
        events = b.store.get_events(b.runtime.session_id)
        assert any(e.type.value == "subagent_result" and e.data["kind"] == "docs" for e in events)
        assert child_policies == [b.policy]
        await b.handle("setAgentEnabled", dict(name="docs", enabled=False))
        await b.before_round()
        assert "docs" not in b.registry.get("delegate").allowed_kinds
        await b.discard_runtime()
        b.store.close()
    asyncio.run(scenario())


def test_statistics_survive_resume_and_exclude_unmeasured_tps(configuration, tmp_path, monkeypatch):
    monkeypatch.setattr(bridge_mod, "provider_for", lambda *_: FakeProvider())
    async def scenario():
        b = bridge_mod.Bridge(SqliteStore(tmp_path / "stats.sqlite"))
        sid = b.store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        from minicode.core.models import EventType
        b.store.append_event(sid, EventType.ASSISTANT_MESSAGE, {"usage": Usage(input_tokens=100, output_tokens=20, cache_read_tokens=60).model_dump(), "request_seconds": 2})
        b.store.append_event(sid, EventType.ASSISTANT_MESSAGE, {"usage": Usage(input_tokens=200, output_tokens=30).model_dump()})
        await b.handle("selectSession", {"sessionId": sid})
        stats = b.state()["statistics"]
        assert stats["requests"] == 2 and stats["tps"] == 10
        assert stats["samples"][0]["cached"] == 60
        assert stats["samples"][1]["tps"] is None
        assert b.runtime.usage.input_tokens == 300
        await b.discard_runtime()
        b.store.close()
    asyncio.run(scenario())


def test_desktop_initialization_has_no_offline_model(configuration):
    async def scenario():
        b = bridge_mod.Bridge(SqliteStore(":memory:"))
        initial = await b.handle("initialize", {})
        assert all(m["id"] != "fake" for m in initial["models"])
        assert initial["state"]["model"] != "fake"
        b.store.close()
    asyncio.run(scenario())


def test_model_discovery_retains_stored_secret_and_handles_errors(configuration, monkeypatch):
    import httpx
    public = configuration.save_service(service())
    draft = public["services"][0]
    original_client = httpx.AsyncClient
    def respond(request):
        assert str(request.url) == "https://example.test/v1/models"
        assert request.headers["Authorization"] == "Bearer test-private-key"
        return httpx.Response(200, json={"data": [{"id": "discovered"}]})
    monkeypatch.setattr(bridge_mod.httpx, "AsyncClient", lambda **kwargs: original_client(transport=httpx.MockTransport(respond), **kwargs))
    async def scenario():
        b = bridge_mod.Bridge(SqliteStore(":memory:"))
        result = await b.handle("fetchServiceModels", {"service": draft})
        assert result["models"][0]["modelId"] == "discovered"
        assert "test-private-key" not in json.dumps(result)
        monkeypatch.setattr(bridge_mod.httpx, "AsyncClient", lambda **kwargs: original_client(transport=httpx.MockTransport(lambda r: httpx.Response(401, text="test-private-key")), **kwargs))
        with pytest.raises(ValueError, match="HTTP 401") as caught:
            await b.handle("fetchServiceModels", {"service": draft})
        assert "test-private-key" not in str(caught.value)
        b.store.close()
    asyncio.run(scenario())


def test_running_plugin_setting_preserves_runtime_and_reloads_next_turn(configuration, tmp_path, monkeypatch):
    from minicode import __version__
    plugin = tmp_path / ".minicode" / "plugins" / "quiet"
    plugin.mkdir(parents=True)
    (plugin / "plugin.json").write_text(json.dumps(dict(name="quiet", version="0.1.0", minicode_version=__version__, enabled=True)))
    monkeypatch.setattr(bridge_mod, "provider_for", lambda *_: FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done") ])))
    async def scenario():
        router = bridge_mod.BridgeRouter(SqliteStore(tmp_path / "plugins.sqlite"))
        b = bridge_mod.Bridge(router.store)
        router.clients["qa"] = b
        await b.create_runtime(tmp_path, "fake", None)
        runtime, background = b.runtime, b.runtime._background_manager
        release = asyncio.Event()
        b.run_task = asyncio.create_task(release.wait())
        await router.handle("setPluginEnabled", dict(clientKey="qa", workspace=str(tmp_path), name="quiet", enabled=False))
        assert b.runtime is runtime and b.busy() and b.registry_dirty
        release.set()
        await b.run_task
        await b.ensure_runtime(tmp_path, None, "fake")
        assert b.runtime is runtime and b.runtime._background_manager is background
        assert b.registry.plugin_catalog.plugins["quiet"].enabled is False
        assert b.registry_dirty is False
        await router.aclose()
    asyncio.run(scenario())


def test_service_edits_apply_to_existing_provider_on_next_request(configuration, tmp_path):
    configuration.save_service(service())
    async def scenario():
        router = bridge_mod.BridgeRouter(SqliteStore(tmp_path / "edit.sqlite"))
        b = bridge_mod.Bridge(router.store)
        router.clients["qa"] = b
        model = configuration.models()[0]["id"]
        await b.create_runtime(tmp_path, model, None)
        draft = configuration.public()["services"][0]
        draft["baseUrl"] = "https://new.example.test/v1"
        await router.handle("saveService", dict(clientKey="qa", service=draft))
        assert b.pending_model and b.runtime.provider.base_url == "https://example.test/v1"
        await b.before_round()
        assert b.runtime.provider.base_url == "https://new.example.test/v1"
        assert not b.pending_model
        await router.aclose()
    asyncio.run(scenario())
