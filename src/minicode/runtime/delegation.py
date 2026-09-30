"""One-level child agents with shared budgets and durable recovery."""

from __future__ import annotations

import json

from minicode.core.models import EventType, ExitReason, RunResult, TextBlock, ToolOutcome
from minicode.runtime.events import EventRecorder
from minicode.runtime.prompt import build_system_prompt
from minicode.context.extensions import ProjectInstructions
from minicode.tools.registry import ToolRegistry


async def delegate(self, kind: str, task: str) -> ToolOutcome:
    from minicode.runtime.loop import AgentRuntime
    from minicode.context.compact import CompactConfig, ContextCompactor
    from minicode.security.policy import DefaultPolicy
    from minicode.tools.artifacts import ReadArtifactTool
    from minicode.tools.files import LsTool, ReadTool
    from minicode.tools.search import GrepTool

    agent_source = None
    agent_instructions = ""
    definition = self._registry.agent_definitions.get(kind)
    if definition is not None and not definition["enabled"]:
        return ToolOutcome.failure(f"subagent is disabled: {kind}")
    if definition is not None:
        agent_instructions = definition["instructions"]
        agent_source = {"agent": kind, "origin": "builtin" if definition.get("builtin") else "user"}
    elif kind not in {"explore", "review"}:
        catalog = self._registry.plugin_catalog
        if catalog is None:
            return ToolOutcome.failure(f"unknown read-only subagent kind: {kind}")
        try:
            agent_instructions, agent_source = catalog.agent_instructions(kind)
        except (ValueError, OSError, UnicodeError) as exc:
            return ToolOutcome.failure(str(exc))

    if self.session_id is None:
        return ToolOutcome.failure("parent session has not started")
    assert self._budget_ledger is not None
    if (self._budget.max_total_tokens > 0
            and self._budget_ledger.usage.total_tokens >= self._budget.max_total_tokens):
        return ToolOutcome.failure("parent token budget is exhausted")

    child_registry = ToolRegistry()
    if definition is not None:
        names = (self._registry.names() if definition.get("inheritTools") else definition["tools"])
        for name in names:
            if name == "delegate" or name.startswith("task_"):
                continue
            tool = self._registry.get(name)
            if tool is not None:
                child_registry.register(tool)
    else:
        for tool in (ReadTool(), LsTool(), GrepTool(), ReadArtifactTool()):
            child_registry.register(tool)
    async def on_child_event(event: Any) -> None:
        if event.type is EventType.SESSION_START:
            await EventRecorder(self._store, self.session_id, self._on_event).emit(
                EventType.SUBAGENT_START,
                {"kind": kind, "task": task, "child_session_id": child.session_id,
                 "source": agent_source},
            )

    key = (kind, task)
    child = self._child_sessions.get(key)
    requirements = "\n".join(
        block.text for message in self._messages[-12:] if message.role == "user"
        for block in message.content if isinstance(block, TextBlock)
    )
    child_prompt = (
        build_system_prompt(str(self._workspace.resolve()), child_registry.names())
        + ("\n\n你是子助手。只使用配置的工具，遵守父会话权限。" if definition is not None else "\n\n你是只读子助手。只能调查与审查，不得执行 shell 或修改文件。")
        + "最终只输出 JSON 对象，键为 summary（字符串）、findings（字符串列表）、"
        "evidence_refs（你实际读取过的工作区相对路径列表）、unresolved（字符串列表）。"
        "不得编造证据。\n父任务要求：\n" + requirements
        + ("\n插件子助手指令：\n" + agent_instructions if agent_instructions else "")
    )
    if child is None:
        paused_id = None
        for event in reversed(self._store.get_events(self.session_id)):
            if (event.type in {EventType.SUBAGENT_RESULT, EventType.SUBAGENT_START}
                    and event.data.get("kind") == kind
                    and event.data.get("task") == task):
                if event.data.get("exit_reason") in {
                    ExitReason.MAX_ROUNDS.value, ExitReason.TIME_BUDGET.value,
                    ExitReason.TOKEN_BUDGET.value,
                } or event.type is EventType.SUBAGENT_START:
                    paused_id = event.data.get("child_session_id")
                break
        common = dict(
            provider=self._provider, registry=child_registry, store=self._store,
            policy=self._policy if definition is not None else DefaultPolicy(), workspace=self._workspace,
            approval_handler=self._approval_handler if definition is not None else None,
            provider_name=self._provider_name, model=self._model,
            budget=self._budget, artifact_store=self._artifact_store,
            project_instructions=ProjectInstructions(self._workspace),
            skills=self._skills,
            allow_delegation=False, on_event=on_child_event,
            shared_budget=self._budget_ledger, system_prompt=child_prompt,
        )
        child = (AgentRuntime.resume(session_id=paused_id, **common)
                 if isinstance(paused_id, str) else AgentRuntime(**common))
        self._child_sessions[key] = child
    else:
        child.set_model(provider=self._provider, provider_name=self._provider_name, model=self._model)
        child._registry = child_registry
        child._base_system_prompt = child_prompt
        child._refresh_system_prompt()
        child._shared_budget = self._budget_ledger
        child._budget_ledger = self._budget_ledger
    if self._artifact_store is not None:
        child._compactor = ContextCompactor(
            CompactConfig(max_context_tokens=max(child.context_window, 1)),
            spill_fn=lambda archive_kind, content: self._artifact_store.spill(
                child.session_id, archive_kind, content
            ).artifact_id,
            context_tokens_fn=lambda: child.context_window,
            output_tokens_fn=child.effective_max_output_tokens,
            estimate_scale_fn=lambda: getattr(child.provider, "prompt_scale", 1.0),
        )
    child_result: RunResult | None = None
    child_usage_before = child.usage
    try:
        if child.session_id is not None and not child.task_pending:
            child_result = RunResult(session_id=child.session_id, exit_reason=ExitReason.COMPLETED,
                                     rounds=child.rounds, total_usage=child.usage)
        else:
            child_result = (await child.continue_turn() if child.session_id is not None
                            else await child.run_turn(f"{kind}：{task}"))
    finally:
        delta = child.usage.model_copy(update={
            "input_tokens": child.usage.input_tokens - child_usage_before.input_tokens,
            "output_tokens": child.usage.output_tokens - child_usage_before.output_tokens,
            "cache_read_tokens": child.usage.cache_read_tokens - child_usage_before.cache_read_tokens,
            "cache_write_tokens": child.usage.cache_write_tokens - child_usage_before.cache_write_tokens,
        })
        self._usage = self._usage + delta
        self._persist_counters()
    if child_result.exit_reason is ExitReason.COMPLETED:
        self._child_sessions.pop(key, None)
    child_id = child.session_id
    assert child_id is not None
    events = self._store.get_events(child_id)
    successful_calls = {
        str(event.data.get("call_id")) for event in events
        if event.type is EventType.TOOL_CALL_RESULT and event.data.get("success")
    }
    observed = {
        str(event.data["arguments"]["path"])
        for event in events if event.type is EventType.TOOL_CALL_START
        and str(event.data.get("call_id")) in successful_calls
        and event.data.get("name") in {"read", "ls", "grep"}
        and isinstance(event.data.get("arguments", {}).get("path"), str)
    }
    final_text = ""
    for message in reversed(child._messages):
        if message.role == "assistant":
            final_text = "".join(b.text for b in message.content if isinstance(b, TextBlock))
            break
    structured = False
    try:
        payload = json.loads(final_text)
        if not isinstance(payload, dict):
            raise ValueError("child response is not an object")
        if (not isinstance(payload.get("summary"), str)
                or any(not isinstance(payload.get(key), list)
                       for key in ("findings", "evidence_refs", "unresolved"))):
            raise ValueError("child response has an invalid schema")
        summary = str(payload.get("summary", ""))
        findings = [str(item) for item in payload.get("findings", [])]
        unresolved = [str(item) for item in payload.get("unresolved", [])]
        requested_refs = payload.get("evidence_refs", [])
        if not isinstance(requested_refs, list):
            requested_refs = []
        structured = True
    except (ValueError, TypeError, AttributeError):
        summary, findings, requested_refs = final_text, [], []
        unresolved = ["子助手未返回有效的结构化结果。"]
    verified: list[str] = []
    for ref in requested_refs:
        if not isinstance(ref, str):
            continue
        path = ref.split(":", 1)[0]
        resolved = (self._workspace / path).resolve()
        if path in observed and resolved.is_relative_to(self._workspace.resolve()) and resolved.exists():
            verified.append(ref)
        else:
            unresolved.append(f"未验证的证据引用：{ref}")
    record = {
        "kind": kind, "task": task, "child_session_id": child_id,
        "exit_reason": child_result.exit_reason.value if child_result else "unknown",
        "structured": structured,
        "usage": delta.model_dump(), "usage_scope": "activation", "evidence_refs": verified,
        "source": agent_source,
    }
    await EventRecorder(self._store, self.session_id, self._on_event).emit(
        EventType.SUBAGENT_RESULT, record
    )
    status = record["exit_reason"] if structured else "invalid_result"
    result = {"summary": summary, "findings": findings,
              "evidence_refs": verified, "unresolved": unresolved,
              "child_session_id": child_id, "status": status}
    completed = (child_result is not None and child_result.exit_reason is ExitReason.COMPLETED
                 and structured)
    if not completed:
        result["unresolved"].append(f"子任务状态：{status}")
    return ToolOutcome(
        success=completed, output=json.dumps(result, ensure_ascii=False),
        error=None if completed else f"子任务未完成：{status}",
    )

