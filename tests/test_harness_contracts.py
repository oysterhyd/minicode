"""Fault injection and integration coverage for production harness boundaries."""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path

import httpx
import pytest
from pydantic import BaseModel

from minicode.configuration import HarnessConfiguration
from minicode.core.models import (
    ApprovalDecision, Budget, EventType, ExitReason, Message, ModelResponse, TextBlock,
    ToolOutcome, ToolResultBlock, ToolUseBlock, Usage,
)
from minicode.providers import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn, ResponseDone, TextDelta
from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.errors import ProviderError
from minicode.runtime import AgentRuntime
from minicode.runtime.protocol import RequestServer
from minicode.runtime.scheduling import ToolScheduler
from minicode.security import AutoAllowPolicy, DefaultPolicy, PolicyBehavior
from minicode.storage import ArtifactStore, SqliteStore
from minicode.storage.ownership import SessionBusyError
from minicode.tools.base import BaseTool, READ_EXECUTION, ToolExecution
from minicode.tools.registry import ToolRegistry, default_registry


class Crash(BaseException):
    pass


def runtime(tmp_path, provider, *, registry=None, store=None, **options):
    store = store or SqliteStore(tmp_path / "harness.db")
    return AgentRuntime(provider=provider, registry=registry or default_registry(), store=store,
                        policy=options.pop("policy", AutoAllowPolicy()), workspace=tmp_path,
                        provider_name="fake", model="fake", **options), store


def test_checkpoint_rolls_back_message_result_usage_and_event(tmp_path, monkeypatch):
    with SqliteStore(tmp_path / "checkpoint.db") as store:
        sid = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        def fail(*args, **kwargs):
            raise OSError("injected persistence failure")
        monkeypatch.setattr(store, "append_event", fail)
        with pytest.raises(OSError):
            store.checkpoint(sid, EventType.ASSISTANT_MESSAGE, {},
                             message=Message(role="assistant", content=[TextBlock(text="answer")]),
                             result=ToolResultBlock(tool_use_id="call", content="known"),
                             counters={"input_tokens": 10})
        assert store.get_messages(sid) == []
        assert store.get_tool_result(sid, "call") is None
        assert store.get_events(sid) == []
        assert store.get_session(sid).input_tokens == 0


def test_crash_after_assistant_checkpoint_restores_answer_without_another_request(tmp_path):
    async def crash(event):
        if event.type is EventType.ASSISTANT_MESSAGE:
            raise Crash()
    agent, store = runtime(tmp_path, FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(text="durable answer", input_tokens=17, output_tokens=5),
    ])), on_event=crash)
    try:
        with pytest.raises(Crash):
            asyncio.run(agent.run_turn("go"))
        assert store.get_session(agent.session_id).input_tokens == 17
        class MustNotRun:
            name = "fake"
            async def stream(self, **kwargs):
                raise AssertionError("a persisted final answer must not be requested again")
                yield
        restored = AgentRuntime.resume(store=store, session_id=agent.session_id,
                                       provider=MustNotRun(), registry=default_registry(),
                                       policy=AutoAllowPolicy())
        result = asyncio.run(restored.continue_turn())
        assert result.exit_reason is ExitReason.COMPLETED
        assert result.total_usage.total_tokens == 22
        assert store.get_messages(agent.session_id)[-1].content[0].text == "durable answer"
    finally:
        store.close()


def test_known_write_result_survives_crash_before_round_message_delivery(tmp_path):
    async def crash(event):
        if event.type is EventType.TOOL_CALL_RESULT:
            raise Crash()
    script = FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={"path": "result.txt", "content": "once"})]),
        FakeTurn(text="done"),
    ])
    agent, store = runtime(tmp_path, FakeProvider(script), on_event=crash)
    try:
        with pytest.raises(Crash):
            asyncio.run(agent.run_turn("write"))
        call = next(b for m in store.get_messages(agent.session_id) for b in m.content if isinstance(b, ToolUseBlock))
        known = store.get_tool_result(agent.session_id, call.id)
        assert known and not known.is_error
        # This read-only registry cannot repeat a write. Recovery must use the outbox.
        readonly = ToolRegistry()
        restored = AgentRuntime.resume(store=store, session_id=agent.session_id,
                                       provider=FakeProvider(script), registry=readonly, policy=AutoAllowPolicy())
        result = asyncio.run(restored.continue_turn())
        assert result.exit_reason is ExitReason.COMPLETED
        blocks = [b for m in store.get_messages(agent.session_id) for b in m.content if isinstance(b, ToolResultBlock)]
        assert blocks == [known]
        assert not any(e.type is EventType.SIDE_EFFECT_UNKNOWN for e in store.get_events(agent.session_id))
        assert (tmp_path / "result.txt").read_text(encoding="utf-8") == "once"
    finally:
        store.close()


def test_cross_process_session_ownership_rejects_second_writer(tmp_path):
    with SqliteStore(tmp_path / "ownership.db") as store:
        sid = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        program = (
            "import sys; from minicode.storage import SqliteStore; "
            "s=SqliteStore(sys.argv[1]); "
            "s.activation(sys.argv[2]).__enter__()"
        )
        with store.activation(sid):
            with pytest.raises(SessionBusyError):
                store.delete_session(sid)
            second = subprocess.run([sys.executable, "-c", program, str(tmp_path / "ownership.db"), sid],
                                    capture_output=True, text=True, timeout=10)
            assert second.returncode != 0 and "SessionBusyError" in second.stderr
        # OS ownership is released even though the helper never called __exit__.
        assert subprocess.run([sys.executable, "-c", program, str(tmp_path / "ownership.db"), sid],
                              capture_output=True, timeout=10).returncode == 0
        with store.activation(sid):
            pass


def test_concurrent_activation_and_resume_do_not_mutate_running_session(tmp_path):
    started = asyncio.Event()
    class Blocking:
        name = "fake"
        async def stream(self, **kwargs):
            started.set()
            await asyncio.Event().wait()
            yield
    agent, store = runtime(tmp_path, Blocking())
    async def scenario():
        task = asyncio.create_task(agent.run_turn("first"))
        await started.wait()
        try:
            with pytest.raises(SessionBusyError):
                await agent.run_turn("second")
            with pytest.raises(SessionBusyError):
                AgentRuntime.resume(store=store, session_id=agent.session_id, provider=Blocking(),
                                    registry=default_registry(), policy=AutoAllowPolicy())
            assert agent.snapshot().phase.value == "model"
            assert store.get_messages(agent.session_id) == [Message(role="user", content=[TextBlock(text="first")])]
        finally:
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        assert agent.snapshot().phase.value == "paused" and not agent.snapshot().running
    try:
        asyncio.run(scenario())
    finally:
        store.close()


@pytest.mark.parametrize("calls", [
    [ToolUseBlock(id="same", name="write", input={}), ToolUseBlock(id="same", name="write", input={})],
    [ToolUseBlock(id="", name="write", input={})],
    [ToolUseBlock(id="new", name="", input={})],
])
def test_malformed_tool_ids_never_authorize_execution(tmp_path, calls):
    class Broken:
        name = "fake"
        async def stream(self, **kwargs):
            yield ResponseDone(ModelResponse(blocks=calls))
    agent, store = runtime(tmp_path, Broken())
    try:
        result = asyncio.run(agent.run_turn("go"))
        assert result.exit_reason is ExitReason.PROVIDER_ERROR
        assert not any(e.type is EventType.TOOL_CALL_START for e in store.get_events(result.session_id))
    finally:
        store.close()


def test_reused_tool_id_in_later_round_is_rejected(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(id="shared", name="write", arguments={"path": "a", "content": "first"})]),
        FakeTurn(tool_calls=[FakeToolCall(id="shared", name="write", arguments={"path": "a", "content": "second"})]),
    ]))
    agent, store = runtime(tmp_path, provider)
    try:
        result = asyncio.run(agent.run_turn("go"))
        assert result.exit_reason is ExitReason.PROVIDER_ERROR
        assert (tmp_path / "a").read_text(encoding="utf-8") == "first"
    finally:
        store.close()


def test_partial_stream_error_is_not_retried(tmp_path):
    class Partial:
        name = "fake"
        calls = 0
        async def stream(self, **kwargs):
            self.calls += 1
            yield TextDelta("partial")
            raise ProviderError("transport failed")
    provider = Partial()
    agent, store = runtime(tmp_path, provider)
    try:
        result = asyncio.run(agent.run_turn("go"))
        assert result.exit_reason is ExitReason.PROVIDER_ERROR and provider.calls == 1
        assert not result.total_usage.available
    finally:
        store.close()


def test_gateway_eof_without_terminal_marker_is_not_completion(tmp_path):
    provider = CommandCodeProvider(model="test", api_key="key", base_url="https://test.invalid/v1",
        transport=httpx.MockTransport(lambda request: httpx.Response(200, content=
            b'data: {"choices":[{"delta":{"content":"unfinished"}}]}\n\n')))
    async def scenario():
        try:
            with pytest.raises(ProviderError, match="completion marker"):
                async for _ in provider.stream(system="", messages=[], tools=[]):
                    pass
        finally:
            await provider.aclose()
    asyncio.run(scenario())


def test_desktop_approval_requires_a_boolean(tmp_path, monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[1] / "desktop"))
    import bridge
    with SqliteStore(tmp_path / "approval.db") as store:
        handler = bridge.Bridge(store)
        async def scenario():
            pending = asyncio.get_running_loop().create_future()
            handler.approvals["approval"] = pending
            with pytest.raises(ValueError, match="boolean"):
                await handler.handle("resolveApproval", {"approvalId": "approval", "granted": "false"})
            assert not pending.done()
            assert await handler.handle("resolveApproval", {"approvalId": "approval", "granted": False})
            assert pending.result() is False
        asyncio.run(scenario())


def test_tool_execution_does_not_trust_read_name_or_approval_observer(tmp_path):
    class Args(BaseModel):
        nested: dict
    class Tool(BaseTool):
        name = "read"
        description = "A custom implementation with no read capability claim"
        args_model = Args
        async def execute(self, args, ctx):
            return ToolOutcome(output=args.nested["value"])
    registry = ToolRegistry()
    registry.register(Tool())
    calls = [ToolUseBlock(id=str(i), name="read", input={"nested": {"value": "original"}}) for i in range(2)]
    assert len(ToolScheduler(registry).batch(calls, 0)) == 1
    async def approve(request):
        request.arguments["nested"]["value"] = "mutated"
        return ApprovalDecision(granted=True)
    agent, store = runtime(tmp_path, FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="read", arguments=calls[0].input)]), FakeTurn(text="done"),
    ])), registry=registry, policy=DefaultPolicy(rules={"read": PolicyBehavior.ASK}), approval_handler=approve)
    try:
        result = asyncio.run(agent.run_turn("go"))
        blocks = [b for m in store.get_messages(result.session_id) for b in m.content if isinstance(b, ToolResultBlock)]
        assert blocks[0].content == "original"
    finally:
        store.close()


def test_writable_delegates_are_serial_and_not_replay_safe():
    registry = default_registry(delegation=True)
    registry.set_agents([
        {"name": "reader", "enabled": True, "description": "read", "tools": ["read"], "instructions": "read"},
        {"name": "writer", "enabled": True, "description": "write", "tools": ["write"], "instructions": "write"},
    ], [])
    readonly = ToolUseBlock(id="r", name="delegate", input={"kind": "reader", "task": "read"})
    writable = ToolUseBlock(id="w", name="delegate", input={"kind": "writer", "task": "write"})
    assert registry.execution_for(readonly).replay_safe
    assert not registry.execution_for(writable).replay_safe
    assert len(ToolScheduler(registry).batch([writable, writable], 0)) == 1
    assert len(ToolScheduler(registry).batch([readonly, writable], 0)) == 1


def test_concurrent_protocol_reserves_capacity_for_controls():
    replies = []
    ready, release = asyncio.Event(), asyncio.Event()
    async def handle(method, params):
        if method == "slow":
            ready.set()
            await release.wait()
        return method
    async def scenario():
        server = RequestServer(handle, replies.append, limit=1)
        server.submit(json.dumps({"id": 1, "method": "slow"}))
        await ready.wait()
        server.submit(json.dumps({"id": 2, "method": "getState"}))
        server.submit(json.dumps({"id": 3, "method": "resolveApproval"}))
        await asyncio.sleep(0)
        assert next(r for r in replies if r.get("id") == 2)["error"] == "request queue is full"
        assert next(r for r in replies if r.get("id") == 3)["result"] == "resolveApproval"
        await server.aclose()
        assert len([r for r in replies if r.get("id") == 1]) == 1
    asyncio.run(scenario())


def test_mutating_adapter_does_not_change_persisted_context_or_schemas(tmp_path):
    class Mutating:
        name = "fake"
        async def stream(self, **kwargs):
            kwargs["messages"][0].content[0].text = "tampered"
            kwargs["tools"][0].input_schema.clear()
            yield ResponseDone(ModelResponse(blocks=[TextBlock(text="done")]))
    registry = default_registry()
    original = registry.specs()
    agent, store = runtime(tmp_path, Mutating(), registry=registry)
    try:
        result = asyncio.run(agent.run_turn("original"))
        assert store.get_messages(result.session_id)[0].content[0].text == "original"
        assert registry.specs() == original
    finally:
        store.close()


def test_custom_tool_timeout_returns_failure_and_releases_turn(tmp_path):
    class Args(BaseModel):
        pass
    class Slow(BaseTool):
        name = "slow"
        args_model = Args
        description = "slow"
        execution = ToolExecution(timeout_s=0.02)
        async def execute(self, args, ctx):
            await asyncio.Event().wait()
    registry = ToolRegistry()
    registry.register(Slow())
    agent, store = runtime(tmp_path, FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="slow", arguments={})]), FakeTurn(text="done"),
    ])), registry=registry)
    try:
        result = asyncio.run(agent.run_turn("go"))
        blocks = [b for m in store.get_messages(result.session_id) for b in m.content if isinstance(b, ToolResultBlock)]
        assert result.exit_reason is ExitReason.COMPLETED
        assert blocks[0].is_error and "side effects may be incomplete" in blocks[0].content
    finally:
        store.close()


def test_regex_fallback_interrupts_catastrophic_match(tmp_path, monkeypatch):
    import minicode.tools.search as search
    from minicode.tools.base import ToolContext
    monkeypatch.setattr(search.shutil, "which", lambda _: None)
    monkeypatch.setattr(search, "_RG_TIMEOUT_S", 0.02)
    (tmp_path / "pathological.txt").write_text("a" * 80_000 + "!", encoding="utf-8")
    result = asyncio.run(search.GrepTool().run({"pattern": "(a+)+$"}, ToolContext(workspace=tmp_path)))
    assert not result.success and "timed out" in result.error


def test_cli_keeps_background_command_alive_between_activations(tmp_path, monkeypatch):
    import minicode.cli as cli
    from rich.console import Console
    snippet = "import time; time.sleep(.2); print('background complete')"
    command = f'& "{sys.executable}" -c "{snippet}"' if os.name == "nt" else f'"{sys.executable}" -c "{snippet}"'
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="bash", arguments={"command": command, "background": True})]),
        FakeTurn(text="done"),
    ]))
    with SqliteStore(tmp_path / "cli.db") as store:
        setup = cli._Setup(tmp_path, provider, "fake", "fake", Budget(max_rounds=1))
        console = Console()
        services = cli._build_services(setup, store, console, True)
        repl = cli._ChatRepl(setup=setup, store=store, console=console, services=services)
        count = 0
        async def input_while_jobs_run(*args):
            nonlocal count
            count += 1
            if count == 1:
                return "start"
            if count == 2:
                async def finished():
                    while not services.background_manager.jobs() or services.background_manager.jobs()[0].status == "running":
                        await asyncio.sleep(.02)
                await asyncio.wait_for(finished(), 5)
                assert services.background_manager.jobs()[0].status == "completed"
                return "/continue"
            return "exit"
        monkeypatch.setattr(cli, "console_input", input_while_jobs_run)
        repl.loop()
        assert store.get_session(repl.runtime.session_id).status == "completed"
        assert any("background complete" in b.text for m in store.get_messages(repl.runtime.session_id)
                   for b in m.content if isinstance(b, TextBlock))


def test_custom_model_routing_and_agent_catalog_are_shared_by_cli(tmp_path, monkeypatch):
    import minicode.cli as cli
    import minicode.providers.factory as factory
    import minicode.configuration as configuration_module
    configuration = HarnessConfiguration(tmp_path / "shared.json")
    configuration.write({"services": [], "agents": [], "disabledAgents": [], "defaultModel": ""})
    configuration.save_service({"name": "custom", "baseUrl": "https://service.test/v1", "apiStyle": "openai",
        "apiKey": "private", "enabled": True,
        "models": [{"modelId": "custom-model", "name": "Custom", "contextWindow": 100000,
                    "maxOutputTokens": 4000, "supportsEffort": False}]})
    model = configuration.models()[0]["id"]
    configuration.save_agent({"name": "custom-agent", "label": "Custom", "description": "inspect",
        "instructions": "Inspect requested files.", "tools": ["read"], "inheritTools": False})
    monkeypatch.setattr(configuration_module, "HarnessConfiguration", lambda: configuration)
    monkeypatch.setattr(factory, "HarnessConfiguration", lambda: configuration)
    provider, name, selected = cli._build_provider(cli.ProviderChoice.auto, model, None)
    assert selected == model and name == "commandcode"
    assert provider.model == "custom-model" and provider.context_window == 100000
    assert provider.base_url == "https://service.test/v1" and provider.max_tokens == 4000
    resumed_provider, _, resumed_model = cli._build_provider(cli.ProviderChoice.commandcode, model, None)
    assert resumed_model == model and resumed_provider.model == "custom-model"
    from minicode.runtime.services import workspace_services
    bundle = workspace_services(tmp_path)
    assert "custom-agent" in bundle.registry.agent_definitions


def test_resuming_multiple_child_slices_does_not_double_count_tokens(tmp_path):
    with SqliteStore(tmp_path / "children.db") as store:
        sid = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        child = store.create_session(workspace=str(tmp_path), provider="fake", model="fake")
        store.append_message(sid, Message(role="user", content=[TextBlock(text="inspect")]))
        store.append_event(sid, EventType.SESSION_START, {})
        store.append_event(sid, EventType.ASSISTANT_MESSAGE, {"usage": Usage(input_tokens=10).model_dump()})
        store.append_event(sid, EventType.SUBAGENT_START, {"child_session_id": child})
        store.append_event(child, EventType.ASSISTANT_MESSAGE, {"usage": Usage(input_tokens=20).model_dump()})
        store.append_event(child, EventType.ASSISTANT_MESSAGE, {"usage": Usage(input_tokens=30).model_dump()})
        store.update_session(child, input_tokens=50)
        # Older transcripts recorded cumulative child usage each slice.
        store.append_event(sid, EventType.SUBAGENT_RESULT, {"child_session_id": child, "usage": Usage(input_tokens=20).model_dump()})
        store.append_event(sid, EventType.SUBAGENT_RESULT, {"child_session_id": child, "usage": Usage(input_tokens=50).model_dump()})
        restored = AgentRuntime.resume(store=store, session_id=sid, provider=FakeProvider(),
                                       registry=default_registry(), policy=AutoAllowPolicy())
        assert restored.usage.input_tokens == 60


def test_desktop_slow_settings_request_does_not_block_cancel(tmp_path, monkeypatch):
    desktop = Path(__file__).resolve().parents[1] / "desktop"
    monkeypatch.syspath_prepend(str(desktop))
    import bridge
    started, slow_started, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    class Blocking:
        name = "fake"
        async def stream(self, **kwargs):
            started.set()
            await asyncio.Event().wait()
            yield
    original = bridge.Bridge.handle
    async def handle(self, method, params):
        if method == "fetchServiceModels":
            slow_started.set()
            await release.wait()
            return {"models": []}
        return await original(self, method, params)
    monkeypatch.setattr(bridge.Bridge, "handle", handle)
    monkeypatch.setattr(bridge, "provider_for", lambda *args: Blocking())
    monkeypatch.setattr(bridge, "emit", lambda *args: None)
    async def scenario():
        router = bridge.BridgeRouter(SqliteStore(tmp_path / "desktop.db"))
        try:
            await router.handle("sendPrompt", {"clientKey": "test", "workspace": str(tmp_path),
                                              "text": "go", "model": "fake"})
            await started.wait()
            slow = asyncio.create_task(router.handle("fetchServiceModels", {"clientKey": "test", "service": {}}))
            await slow_started.wait()
            assert await asyncio.wait_for(router.handle("cancelTurn", {"clientKey": "test"}), 1)
            await asyncio.wait_for(router.clients["test"].run_task, 1)
            assert not slow.done()
            assert router.clients["test"].runtime.snapshot().phase.value == "paused"
            release.set()
            await slow
        finally:
            await router.aclose()
    asyncio.run(scenario())


def test_failed_adapter_cannot_mutate_next_retry_request(tmp_path):
    class Mutating:
        name = "fake"
        calls = 0
        async def stream(self, **kwargs):
            self.calls += 1
            assert kwargs["messages"][0].content[0].text == "original"
            if self.calls == 1:
                kwargs["messages"][0].content[0].text = "changed"
                raise ProviderError("temporary error")
            yield ResponseDone(ModelResponse(blocks=[TextBlock(text="done")]))
    provider = Mutating()
    agent, store = runtime(tmp_path, provider)
    try:
        result = asyncio.run(agent.run_turn("original"))
        assert result.exit_reason is ExitReason.COMPLETED and provider.calls == 2
    finally:
        store.close()


def test_tool_schema_aliases_are_the_same_parameters_approved_and_executed(tmp_path):
    from pydantic import Field
    from minicode.tools.base import ToolContext
    class Args(BaseModel):
        path: str = Field(alias="file-path")
    class Tool(BaseTool):
        name = "aliased"
        description = "aliased"
        args_model = Args
        async def execute(self, args, ctx):
            return ToolOutcome(output=args.path)
    tool = Tool()
    assert "file-path" in tool.spec().input_schema["properties"]
    assert tool.validate_args({"file-path": "a.txt"}) == {"file-path": "a.txt"}
    assert asyncio.run(tool.run({"file-path": "a.txt"}, ToolContext(workspace=tmp_path))).output == "a.txt"


def test_inherited_skill_tools_work_in_child_with_separate_activation_state(tmp_path):
    from minicode.context.extensions import SkillCatalog
    directory = tmp_path / ".minicode" / "skills" / "inspect"
    directory.mkdir(parents=True)
    (directory / "SKILL.md").write_text("---\nname: inspect\ndescription: inspect source\n---\nInspect carefully.\n", encoding="utf-8")
    catalog = SkillCatalog(tmp_path, user_root=tmp_path / "empty")
    registry = default_registry(skills=catalog, delegation=True)
    registry.set_agents([{"name": "reader", "description": "inspect", "instructions": "inspect",
                         "tools": [], "inheritTools": True, "enabled": True}], [])
    script = FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="delegate", arguments={"kind": "reader", "task": "inspect"})]),
        FakeTurn(tool_calls=[FakeToolCall(name="skill_load", arguments={"name": "inspect"})]),
        FakeTurn(text=json.dumps({"summary": "done", "findings": [], "evidence_refs": [], "unresolved": []})),
        FakeTurn(text="parent done"),
    ])
    agent, store = runtime(tmp_path, FakeProvider(script), registry=registry, skills=catalog)
    try:
        result = asyncio.run(agent.run_turn("inspect"))
        assert result.exit_reason is ExitReason.COMPLETED
        child_id = next(e.data["child_session_id"] for e in store.get_events(result.session_id)
                        if e.type is EventType.SUBAGENT_START)
        assert any(e.type is EventType.SKILL_ACTIVATED for e in store.get_events(child_id))
        assert agent.active_skills == {}
    finally:
        store.close()
