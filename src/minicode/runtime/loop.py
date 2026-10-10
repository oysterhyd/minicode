"""The agent conversation loop.

:class:`AgentRuntime` wires a model provider, the tool registry, the
permission policy and the session store into the core agent loop:

stream one assistant turn -> persist it -> run its tool calls under policy
and approval -> feed the results back to the model -> repeat until the model
answers without tool calls, or a budget / error condition fires.

Every state change is persisted as it happens (messages, events, session
status), so a crash or cancellation never loses more than the in-flight
round. Denials, unknown tools and invalid arguments are backfilled to the
model as error ``tool_result`` blocks so it can correct itself; only the
session-level finalize paths end a turn.
"""

from __future__ import annotations

import asyncio
from contextlib import nullcontext
import json
import re
import time
import uuid
from pathlib import Path
from typing import Any, Awaitable, Callable

from minicode.core.models import (
    ApprovalHandler,
    ApprovalRequest,
    Budget,
    EventType,
    ExitReason,
    Message,
    ModelResponse,
    RunResult,
    StopReason,
    TextBlock,
    ToolOutcome,
    ToolResultBlock,
    ToolUseBlock,
    Usage,
)
from minicode.providers.base import Provider
from minicode.providers.errors import ProviderRequestError
from minicode.runtime.budget import BudgetChecker, SharedBudgetLedger
from minicode.runtime.events import EventCallback, EventRecorder
from minicode.runtime.scheduling import ToolScheduler
from minicode.runtime.state import RunPhase, RuntimeSnapshot
from minicode.runtime.prompt import build_system_prompt
from minicode.context.extensions import ProjectInstructions, SkillCatalog, Source
from minicode.security.policy import PermissionPolicy
from minicode.storage import SessionStore
from minicode.storage.ownership import SessionBusyError
from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.registry import ToolRegistry

#: Called for every streamed piece of assistant text.
TextDeltaCallback = Callable[[str], Awaitable[None]]

#: Tool output echoed in TOOL_CALL_RESULT events is capped at this length.
_OUTPUT_PREVIEW_CHARS = 500

#: Tool output limits for generic tools; individual tools page before this.
_SPILL_LIMITS = ToolLimits()

class _DeadlineExceeded(TimeoutError):
    """The turn's deadline expired, including during synchronous work."""


class _ContextLimitExceeded(RuntimeError):
    """The compacted next request still exceeds the model's hard window."""


def _format_bg_result(job: Any) -> str:
    """User-message text announcing one finished background job."""
    exit_label = "-" if job.exit_code is None else str(job.exit_code)
    status = "完成" if job.status == "completed" else f"失败（{job.status}）"
    output = job.output if job.output else "（无输出）"
    return (
        f"[后台任务 {job.job_id} 已{status}] 退出码: {exit_label}\n"
        f"命令: {job.command}\n输出:\n{output}"
    )


class AgentRuntime:
    """Stateful agent loop for one session.

    The session row is created lazily on the first :meth:`run_turn` call;
    afterwards messages, usage, rounds and events accumulate across turns.
    An existing session can be continued with :meth:`resume`, which restores
    the persisted conversation and settles tool calls whose outcome was
    lost to an interruption before handing control back to the loop.

    P1 hooks (all optional, duck-typed so the runtime stays decoupled):

    - ``compactor`` — context compaction (``needs_compaction`` / ``compact``);
      runs before each provider call and rewrites the persisted conversation.
    - ``goal_checker`` — acceptance gate (``spec``, ``protected_snapshot``,
      ``run() -> report``, ``format_failure_report``); a model answer without
      tool calls only ends the session when acceptance passes.
    - ``evidence_ledger`` — binds passing evidence to a workspace fingerprint
      (``record(report)`` / ``valid_pass(fingerprint)``); code changes after
      evidence was recorded invalidate it.
    - ``background_manager`` — background command jobs (``poll_completed``,
      ``cancel_all``); finished jobs are delivered as user messages.
    - ``artifact_store`` — full tool outputs spilled to disk when they exceed
      the tool limits, retained for session reports.

    Cancellation contract: if the task running :meth:`run_turn` is
    cancelled, the runtime finalizes the session with
    ``ExitReason.CANCELLED`` (store status updated and ``SESSION_END``
    emitted; finalization itself is guarded so it cannot raise) and then
    re-raises :class:`asyncio.CancelledError`. Callers see cancellation
    propagate normally while the persisted state is already consistent.
    """

    def __init__(
        self,
        *,
        provider: Provider,
        registry: ToolRegistry,
        store: SessionStore,
        policy: PermissionPolicy,
        workspace: Path,
        provider_name: str,
        model: str,
        budget: Budget | None = None,
        approval_handler: ApprovalHandler | None = None,
        on_text_delta: TextDeltaCallback | None = None,
        on_event: EventCallback | None = None,
        system_prompt: str | None = None,
        compactor: Any | None = None,
        goal_checker: Any | None = None,
        evidence_ledger: Any | None = None,
        background_manager: Any | None = None,
        artifact_store: Any | None = None,
        project_instructions: ProjectInstructions | None = None,
        skills: SkillCatalog | None = None,
        allow_delegation: bool = True,
        shared_budget: SharedBudgetLedger | None = None,
    ) -> None:
        self._provider = provider
        self._registry = registry
        self._store = store
        # The board shares the session database; its tools only appear when
        # the host explicitly registers them. Reopening a session keeps tasks.
        from minicode.storage.sqlite_store import SqliteStore
        from minicode.tasks.taskstore import TaskStore
        from minicode.context.memory import ProjectMemoryStore
        self._task_store = TaskStore(store) if isinstance(store, SqliteStore) else None
        self._project_memory = ProjectMemoryStore(store) if isinstance(store, SqliteStore) else None
        self._policy = policy
        self._workspace = workspace
        self._provider_name = provider_name
        self._model = model
        self._budget = budget if budget is not None else Budget()
        self._approval_handler = approval_handler
        self._on_text_delta = on_text_delta
        self._on_event = on_event
        self._base_system_prompt = (
            system_prompt
            if system_prompt is not None
            else build_system_prompt(str(workspace.resolve()), registry.names())
        )
        self._auto_system_prompt = system_prompt is None
        self._system_prompt = self._base_system_prompt
        self._compactor = compactor
        self._goal_checker = goal_checker
        self._evidence_ledger = evidence_ledger
        self._background_manager = background_manager
        self._artifact_store = artifact_store
        self._project_instructions = project_instructions
        self._skills = skills
        self._active_skills: dict[str, str] = {}
        self._active_skill_sources: dict[str, Source] = {}
        self._allow_delegation = allow_delegation
        self._shared_budget = shared_budget
        self._budget_ledger: SharedBudgetLedger | None = shared_budget
        self._child_sessions: dict[tuple[str, str], AgentRuntime] = {}
        self._pending_instruction_scopes: set[Path] = set()
        self._refresh_system_prompt()

        self.session_id: str | None = None  # created on first run_turn
        self._messages: list[Message] = []  # mirrors the persisted conversation
        self._usage = Usage()
        self._last_model_usage: Usage | None = None
        self._rounds = 0
        self._goal_attempts = 0
        self._own_pass_fingerprint: str | None = None  # fallback without a ledger
        self._protected_captured = False
        self._pending_dangling: list[ToolUseBlock] = []  # set by resume()
        self._deadline: float | None = None
        self._announced_jobs: set[str] = set()
        self._lost_jobs: list[dict[str, Any]] = []
        self._approval_lock = asyncio.Lock()
        self._task_pending = False
        self._last_tool_signature: str | None = None
        self._same_tool_rounds = 0
        self._last_exit_reason: ExitReason | None = None
        self._provider_in_flight = False
        self._running = False
        self._activation_guard = None
        self.before_round: Callable[[], Awaitable[None]] | None = None
        self._phase = RunPhase.IDLE
        self._run_id: str | None = None

    # -- read-only state ----------------------------------------------------

    @property
    def usage(self) -> Usage:
        """Session-cumulative token usage."""
        return self._usage.model_copy(deep=True)

    @property
    def last_model_usage(self) -> Usage | None:
        """Usage for the most recent model response, if one exists."""
        return self._last_model_usage.model_copy(deep=True) if self._last_model_usage else None

    @property
    def task_pending(self) -> bool:
        """Whether the latest task has a durable continuation point."""
        return self._task_pending

    @property
    def message_count(self) -> int:
        """Persisted conversation length, used to detect time-slice progress."""
        return len(self._messages)

    @property
    def rounds(self) -> int:
        """Session-cumulative round count."""
        return self._rounds

    @property
    def model(self) -> str:
        """The active provider-facing model id."""
        return self._model

    @property
    def provider(self) -> Provider:
        """The active provider adapter (swappable via :meth:`set_model`)."""
        return self._provider

    @property
    def provider_name(self) -> str:
        return self._provider_name

    @property
    def workspace(self) -> Path:
        return self._workspace

    @property
    def context_window(self) -> int:
        """Prompt-token capacity of the active model (catalog lookup)."""
        from minicode.core.catalog import lookup_model

        return getattr(self._provider, "context_window", lookup_model(self._model).context_window)

    def prompt_budget_tokens(self) -> int:
        """Compaction trigger, reserving 16,384 tokens for the next answer."""
        return max(0, self.context_window - 16_384)

    def effective_max_output_tokens(self) -> int:
        """Response budget actually sent by the active provider.

        Provider construction, compaction and UI context reporting all read
        this same value, avoiding a catalog/request mismatch after startup or
        a live model switch.
        """
        configured = getattr(self._provider, "max_tokens", None)
        if isinstance(configured, int) and configured > 0:
            return configured
        from minicode.core.catalog import lookup_model

        return lookup_model(self._model).max_output_tokens or 2000

    def context_tokens_used(self) -> int:
        """Estimated prompt size of the current context (no output reserve).

        Uses the same conservative estimator as compaction, so the status
        bar's context-window load matches when compaction would trigger.
        """
        return sum(self.context_token_breakdown().values())

    def context_token_breakdown(self) -> dict[str, int]:
        """Estimated system, tool and conversation shares of the current prompt."""
        from minicode.context.estimate import estimate_messages_tokens

        specs = self._registry.specs()
        system = estimate_messages_tokens(self._system_prompt, [], reserve_output_tokens=0)
        tools = estimate_messages_tokens(None, [], specs, reserve_output_tokens=0)
        messages = estimate_messages_tokens(None, self._messages, reserve_output_tokens=0)
        scale = getattr(self._provider, "prompt_scale", 1.0)
        total = int((system + tools + messages) * scale)
        scaled_system = int(system * scale)
        scaled_tools = int(tools * scale)
        return {"system": scaled_system, "tools": scaled_tools,
                "messages": total - scaled_system - scaled_tools}

    def _refresh_system_prompt(self) -> None:
        sections = [self._base_system_prompt]
        if self._project_instructions is not None and self._project_instructions.loaded:
            sections.append(self._project_instructions.prompt_section())
        if self._skills is not None and self._skills.skills:
            sections.append("可用技能目录（正文按需加载）：\n" + self._skills.listing(6000))
        sections.extend(
            f"已激活技能 {name}；来源：{source.label()}\n{source.content}"
            for name, source in self._active_skill_sources.items()
        )
        self._system_prompt = "\n\n".join(sections)

    def activate_skill(self, name: str) -> str:
        """Load a skill body once per content version for this session."""
        if self._skills is None:
            raise ValueError("no skills are configured")
        source = self._skills.load(name)
        if name in self._active_skills and self._active_skills[name] != source.digest:
            raise ValueError(f"skill changed during this session; start a new session: {name}")
        if self._active_skills.get(name) != source.digest:
            self._active_skills[name] = source.digest
            self._active_skill_sources[name] = source
            self._refresh_system_prompt()
            if self.session_id is not None:
                self._store.append_event(self.session_id, EventType.SKILL_ACTIVATED, {
                    "name": name, "path": str(source.path), "sha256": source.digest,
                })
        return f"已激活技能 {name}；来源：{source.label()}\n{source.content}"

    def deactivate_skill(self, name: str) -> str:
        if name not in self._active_skills:
            raise ValueError(f"skill is not active: {name}")
        self._active_skills.pop(name)
        self._active_skill_sources.pop(name)
        self._refresh_system_prompt()
        if self.session_id is not None:
            self._store.append_event(self.session_id, EventType.SKILL_DEACTIVATED, {"name": name})
        return f"已停用技能 {name}"

    # -- live configuration ---------------------------------------------------

    @property
    def budget(self) -> Budget:
        return self._budget.model_copy(deep=True)

    @property
    def active_skills(self) -> dict[str, str]:
        return dict(self._active_skills)

    @property
    def goal_checker(self):
        return self._goal_checker

    @property
    def evidence_ledger(self):
        return self._evidence_ledger

    @property
    def compactor(self):
        return self._compactor

    @property
    def messages(self) -> list[Message]:
        return [message.model_copy(deep=True) for message in self._messages]

    @property
    def system_prompt(self) -> str:
        return self._system_prompt

    def snapshot(self) -> RuntimeSnapshot:
        return RuntimeSnapshot(
            session_id=self.session_id, run_id=self._run_id, phase=self._phase,
            running=self._running, task_pending=self._task_pending,
            provider=self._provider_name, model=self._model, rounds=self._rounds,
            usage=self._usage.model_copy(deep=True), budget=self.budget,
            context_window=self.context_window, context_breakdown=self.context_token_breakdown(),
            active_skills=self.active_skills,
        )

    def set_budget(self, budget: Budget) -> None:
        if self._running:
            raise SessionBusyError("budget changes apply at the next activation")
        updated = budget.model_copy(deep=True)
        if self.session_id is not None:
            self._store.append_event(self.session_id, EventType.BUDGET_CHANGED,
                                     {"budget": updated.model_dump()})
        self._budget = updated

    def set_compactor(self, compactor) -> None:
        if self._running:
            raise SessionBusyError("cannot replace compactor during an activation")
        self._compactor = compactor

    def refresh_tools(self) -> None:
        if self._provider_in_flight:
            raise SessionBusyError("tool schemas change at request boundaries")
        if self._auto_system_prompt:
            self._base_system_prompt = build_system_prompt(str(self._workspace.resolve()), self._registry.names())
        self._refresh_system_prompt()

    async def configure_extensions(self, registry: ToolRegistry, skills: SkillCatalog) -> None:
        if self._running:
            raise SessionBusyError("cannot replace extensions during an activation")
        await self._registry.aclose()
        self._registry = registry
        self._skills = skills
        for name in list(self._active_skills):
            if name not in skills.skills:
                self.deactivate_skill(name)
        self.refresh_tools()

    def replace_context(self, messages: list[Message]) -> None:
        if self._running:
            raise SessionBusyError("cannot replace context during an activation")
        if self.session_id is None:
            raise ValueError("session has not started")
        if self._find_dangling_calls(messages):
            raise ValueError("replacement context contains unfinished tool calls")
        copied = [message.model_copy(deep=True) for message in messages]
        self._store.replace_messages(self.session_id, copied)
        self._messages = copied

    async def compact_context(self) -> bool:
        if self._running:
            raise SessionBusyError("cannot compact context during an activation")
        if self.session_id is None or self._compactor is None:
            raise ValueError("context compaction is unavailable")
        recorder = EventRecorder(self._store, self.session_id, self._on_event)
        before = self.context_tokens_used()
        await self._compact_if_needed(recorder, self.session_id)
        return self.context_tokens_used() != before

    async def aclose(self) -> None:
        """Release session resources before the host's event loop ends."""
        if self._running:
            raise SessionBusyError("cancel and await the activation before closing it")
        try:
            await self._cancel_background_jobs()
        finally:
            try:
                await self._registry.aclose()
            finally:
                close = getattr(self._provider, "aclose", None)
                if close is not None:
                    await close()

    def set_model(
        self, *, provider: Provider, provider_name: str, model: str
    ) -> None:
        """Swap the active provider/model mid-session (the ``/model`` command).

        The conversation, usage and session id are kept; only the adapter
        and its model id change, so subsequent rounds are billed and routed
        by the new model. The persisted session row is updated so
        ``sessions list`` reflects reality.
        """
        self._validate_provider(provider, provider_name, model)
        if self._provider_in_flight:
            raise SessionBusyError("model changes apply at request boundaries")
        if self.session_id is not None:
            self._store.update_session(self.session_id, provider=provider_name, model=model)
        self._provider = provider
        self._provider_name = provider_name
        self._model = model

    @staticmethod
    def _validate_provider(provider: Provider, provider_name: str, model: str) -> None:
        if provider.name != provider_name:
            raise ValueError("provider instance does not match provider_name")
        if getattr(provider, "configuration_id", getattr(provider, "model", model)) != model:
            raise ValueError("provider instance does not match model")

    def _check_deadline(self) -> None:
        if self._deadline is not None and time.monotonic() >= self._deadline:
            raise _DeadlineExceeded("turn deadline exceeded")

    # -- turn loop -----------------------------------------------------------
    # The loop exits through a completed result or a durable pause checkpoint.

    async def continue_turn(self) -> RunResult:
        """Continue a paused task without adding a duplicate user message."""
        if self._last_exit_reason is ExitReason.GOAL_NOT_MET:
            self._goal_attempts = 0
        return await self.run_turn(None)

    async def run_turn(self, user_message: str | None) -> RunResult:
        """Activate the session once; rejected concurrent callers change nothing."""
        if self._running:
            raise SessionBusyError("this runtime already has an active turn")
        self._running = True
        self._run_id = uuid.uuid4().hex
        self._phase = RunPhase.PREPARING
        try:
            return await self._run_turn(user_message)
        finally:
            if self._activation_guard is not None:
                self._activation_guard.__exit__(None, None, None)
                self._activation_guard = None
            self._running = False
            self._provider_in_flight = False
            self._run_id = None
            self._phase = RunPhase.PAUSED if self._task_pending else RunPhase.IDLE

    async def _run_turn(self, user_message: str | None) -> RunResult:
        """Run one activation of a user task and return its durable outcome.

        On the first call the session row is created and ``SESSION_START``
        is emitted (a configured protected-path snapshot is captured here,
        before the model can change anything). Each round injects finished
        background-job results, compacts the context when it exceeds the
        budget, streams one assistant response, persists it, executes its
        tool calls under the permission policy, and feeds the
        results back until the model produces a final answer — which, with
        an acceptance gate configured, only ends the session once the goal
        checks pass — or a resource slice checkpoints the task. Adjacent built-in reads may
        run in bounded parallel batches; writes and unknown tools remain
        ordering barriers.

        Cancellation: see the class docstring — ``CANCELLED`` is persisted
        and :class:`asyncio.CancelledError` is re-raised to the caller.
        """
        turn_started = time.monotonic()
        # CLI chat creates a fresh event loop for each user turn.
        self._approval_lock = asyncio.Lock()

        if user_message is None and (self.session_id is None or not self._task_pending):
            raise ValueError("no paused task to continue")

        first_turn = self.session_id is None
        if first_turn:
            self.session_id = self._store.create_session(
                workspace=str(self._workspace.resolve()),
                provider=self._provider_name,
                model=self._model,
            )
        session_id = self.session_id
        assert session_id is not None  # set directly above on the first turn
        ownership = getattr(self._store, "activation", None)
        guard = ownership(session_id) if ownership else nullcontext()
        guard.__enter__()
        self._activation_guard = guard
        recorder = EventRecorder(self._store, session_id, self._on_event)
        user_recorded = False
        if self._shared_budget is None:
            self._budget_ledger = SharedBudgetLedger(
                usage=self._usage,
                deadline=(turn_started + self._budget.max_seconds
                          if self._budget.max_seconds > 0 else None),
            )
        else:
            self._budget_ledger = self._shared_budget
        self._deadline = self._budget_ledger.deadline
        deadline_scope = None
        try:
            if first_turn:
                self._bind_goal_session()
            deadline_scope = asyncio.timeout(
                max(0, self._deadline - time.monotonic()) if self._deadline is not None else None
            )
            async with deadline_scope:
                if first_turn:
                    start_data = {
                        "workspace": str(self._workspace.resolve()),
                        "provider": self._provider_name,
                        "model": self._model,
                        "budget": self._budget.model_dump(),
                    }
                    if self._project_instructions is not None:
                        start_data["project_instructions"] = self._project_instructions.sources()
                    if self._active_skills:
                        start_data["active_skills"] = dict(self._active_skills)
                    resume_identity = getattr(self._provider, "resume_identity", None)
                    if callable(resume_identity):
                        start_data["provider_resume_identity"] = resume_identity()
                    await recorder.emit(
                        EventType.SESSION_START,
                        start_data,
                    )
                discovered = await self._registry.prepare()
                if discovered:
                    if self._auto_system_prompt:
                        self._base_system_prompt = build_system_prompt(
                            str(self._workspace.resolve()), self._registry.names()
                        )
                        self._refresh_system_prompt()
                    for status in discovered:
                        await recorder.emit(EventType.MCP_DISCOVERY, status)
                # A timeout can leave a dangling call in this same process.
                self._pending_dangling = self._find_dangling_calls(self._messages)
                # Settle tool calls interrupted before their result was persisted.
                await self._settle_recovery(recorder)

                # Round/time slices checkpoint work; the optional token guard
                # still accounts for the entire session.
                checker = BudgetChecker(
                    self._budget, start_usage=self._usage, start_rounds=self._rounds
                )

                if user_message is not None:
                    user_msg = Message(role="user", content=[TextBlock(text=user_message)])
                    self._append_message(session_id, user_msg)
                    self._task_pending = True
                    self._goal_attempts = 0
                    self._last_tool_signature = None
                    self._same_tool_rounds = 0
                    user_recorded = True
                self._store.update_session(session_id, status="running", exit_reason=None)

                if user_message is None and self._has_unfinalized_answer():
                    gated = await self._goal_gate(recorder, turn_started)
                    if gated is not None:
                        return gated

                context_rejections = 0
                truncations = 0
                while True:
                    before_round = getattr(self, "before_round", None)
                    if before_round is not None:
                        await before_round()
                    self._check_deadline()
                    if checker.tokens_exceeded(self._budget_ledger.usage):
                        return await self._finalize(
                            recorder, ExitReason.TOKEN_BUDGET, turn_started,
                            error="已达到显式 Token 费用上限；提高预算后可继续当前任务。",
                        )
                    if self._budget.max_rounds > 0 and self._budget_ledger.rounds >= self._budget.max_rounds:
                        return await self._finalize(
                            recorder, ExitReason.MAX_ROUNDS, turn_started
                        )

                    # Background jobs finished since the last round are delivered
                    # before the next model call so the model sees their output.
                    await self._deliver_finished_jobs(recorder, session_id)

                    # Layered compaction before the provider call keeps the
                    # context inside budget; the compacted view is persisted.
                    try:
                        await self._compact_if_needed(recorder, session_id)
                    except _ContextLimitExceeded as exc:
                        return await self._finalize(
                            recorder,
                            ExitReason.CONTEXT_LIMIT,
                            turn_started,
                            error=str(exc),
                        )

                    if not self._budget_ledger.reserve_round(self._budget.max_rounds):
                        return await self._finalize(recorder, ExitReason.MAX_ROUNDS, turn_started)
                    self._rounds += 1
                    await recorder.emit(EventType.ROUND_START, {"round": self._rounds})
                    self._store.update_session(session_id, rounds=self._rounds)

                    self._check_deadline()
                    self._pending_instruction_scopes.clear()
                    self._provider_in_flight = True
                    self._phase = RunPhase.MODEL
                    response, failure, error, provider_exc = await self._stream_assistant_turn()
                    self._provider_in_flight = False
                    self._check_deadline()
                    if response is None:
                        assert failure is not None  # always set when response is None
                        if (isinstance(provider_exc, ProviderRequestError)
                                and self._is_context_error(str(provider_exc))
                                and context_rejections < 3
                                and self._offload_largest_text(session_id)):
                            context_rejections += 1
                            continue
                        return await self._finalize(
                            recorder, failure, turn_started, error=error
                        )
                    context_rejections = 0

                    assistant = Message(role="assistant", content=list(response.blocks))
                    next_usage = self._usage + response.usage
                    # There is no await between committing and mirroring the
                    # transition. Observers always see the complete checkpoint.
                    data = {
                        "text": response.text,
                        "tool_calls": [call.name for call in response.tool_calls],
                        "usage": {**response.usage.model_dump(),
                                  "total_tokens": response.usage.total_tokens},
                        "stop_reason": response.stop_reason.value,
                        "request_seconds": getattr(self, "_request_seconds", 0.0),
                        "generation_seconds": getattr(self, "_generation_seconds", None),
                        "first_token_seconds": getattr(self, "_first_token_seconds", None),
                    }
                    commit = getattr(self._store, "checkpoint", None)
                    counters = self._counter_values(next_usage)
                    if commit:
                        event = commit(session_id, EventType.ASSISTANT_MESSAGE, data,
                                       message=assistant, counters=counters)
                        self._messages.append(assistant)
                    else:
                        self._append_message(session_id, assistant)
                        self._store.update_session(session_id, **counters)
                        event = self._store.append_event(session_id, EventType.ASSISTANT_MESSAGE, data)
                    self._usage = next_usage
                    self._last_model_usage = response.usage
                    self._budget_ledger.add_usage(response.usage)
                    await recorder.publish(event)

                    # Token budget fires before any tool of this round runs: side
                    # effects must not start once the budget is already blown.
                    if checker.tokens_exceeded(self._budget_ledger.usage):
                        self._backfill_unexecuted_calls(
                            session_id, response.tool_calls, "未执行：显式 Token 费用上限已达到。"
                        )
                        return await self._finalize(
                            recorder, ExitReason.TOKEN_BUDGET, turn_started,
                            error="已达到显式 Token 费用上限；提高预算后可继续当前任务。",
                        )

                    if response.stop_reason is StopReason.MAX_TOKENS:
                        self._backfill_unexecuted_calls(
                            session_id, response.tool_calls,
                            "未执行：模型响应被截断，请重新生成完整调用。",
                        )
                        self._append_message(session_id, Message(role="user", content=[TextBlock(
                            text="上一条模型回复被输出长度截断。请简短续写未完成部分；"
                                 "不要假设被截断的工具调用已经执行。"
                        )]))
                        truncations += 1
                        if truncations >= 3:
                            return await self._finalize(
                                recorder, ExitReason.MAX_TOKENS, turn_started,
                                error="连续三次回复被截断；任务已暂停，可调整模型或输出预算后继续。",
                            )
                        await recorder.emit(EventType.ROUND_END, {"round": self._rounds})
                        continue
                    truncations = 0

                    if not response.tool_calls:
                        gated = await self._goal_gate(recorder, turn_started)
                        if gated is not None:
                            return gated
                        # Acceptance failed but fix attempts remain: the failure
                        # report was appended as a user message; keep looping.
                        await recorder.emit(EventType.ROUND_END, {"round": self._rounds})
                        continue

                    tool_results: list[ToolResultBlock] = []
                    self._phase = RunPhase.TOOLS
                    try:
                        scheduler = ToolScheduler(
                            self._registry,
                            parallel_delegates=1 if self._budget.max_total_tokens > 0 else 2,
                        )
                        await scheduler.execute(
                            response.tool_calls,
                            lambda call: self._execute_tool_call(recorder, call, session_id),
                            tool_results,
                        )
                    finally:
                        if tool_results:
                            self._append_message(
                                session_id, Message(role="user", content=tool_results)
                            )
                    await recorder.emit(EventType.ROUND_END, {"round": self._rounds})
                    if self._tool_cycle_stalled(response.tool_calls, tool_results):
                        self._append_message(session_id, Message(role="user", content=[TextBlock(
                            text="连续多轮工具调用与结果完全相同，任务已暂停以避免无效循环。"
                                 "继续时请重新评估目标与可用证据。"
                        )]))
                        return await self._finalize(
                            recorder, ExitReason.STALLED, turn_started,
                            error="重复的工具调用没有产生新证据。",
                        )
        except TimeoutError as exc:
            if (not isinstance(exc, _DeadlineExceeded)
                    and (deadline_scope is None or not deadline_scope.expired())):
                return await self._finalize(recorder, ExitReason.INTERNAL_ERROR, turn_started,
                                            error=f"unexpected host timeout: {exc}")
            if self._provider_in_flight:
                # The gateway may have billed a request without delivering
                # usage. A reported zero would be misleading.
                self._usage = self._usage.model_copy(update={"available": False})
                self._provider_in_flight = False
                self._persist_counters()
            if user_message is not None and not user_recorded:
                self._append_message(session_id, Message(role="user", content=[TextBlock(text=user_message)]))
                self._task_pending = True
            return await self._finalize(recorder, ExitReason.TIME_BUDGET, turn_started)
        except asyncio.CancelledError:
            if user_message is not None and not user_recorded:
                self._append_message(session_id, Message(role="user", content=[TextBlock(text=user_message)]))
                self._task_pending = True
            if self._provider_in_flight:
                self._usage = self._usage.model_copy(update={"available": False})
                self._provider_in_flight = False
                self._persist_counters()
            await self._finalize_cancelled(recorder)
            raise  # never swallow cancellation
        except Exception as exc:  # noqa: BLE001 - checkpoint unexpected failures
            if user_message is not None and not user_recorded:
                self._append_message(session_id, Message(role="user", content=[TextBlock(text=user_message)]))
                self._task_pending = True
            return await self._finalize(
                recorder, ExitReason.INTERNAL_ERROR, turn_started, error=str(exc)
            )
        finally:
            self._deadline = None

    # -- resuming ------------------------------------------------------------

    def _has_unfinalized_answer(self) -> bool:
        if not self._messages or self._messages[-1].role != "assistant":
            return False
        for event in reversed(self._store.get_events(self.session_id)):
            if event.type is EventType.SESSION_END:
                return False
            if event.type is EventType.ASSISTANT_MESSAGE:
                return (not event.data.get("tool_calls")
                        and event.data.get("stop_reason") == StopReason.END_TURN.value)
        return False

    @classmethod
    def resume(cls, *, store: SessionStore, session_id: str, **options) -> "AgentRuntime":
        ownership = getattr(store, "activation", None)
        with ownership(session_id) if ownership else nullcontext():
            from minicode.runtime.recovery import restore_runtime
            return restore_runtime(cls, store=store, session_id=session_id, **options)

    @staticmethod
    def _find_dangling_calls(messages: list[Message]) -> list[ToolUseBlock]:
        """Tool calls from persisted messages that never received a result —
        the interruption window between execution and persistence."""
        requested: list[ToolUseBlock] = []
        answered: set[str] = set()
        for message in messages:
            for block in message.content:
                if isinstance(block, ToolUseBlock):
                    requested.append(block)
                elif isinstance(block, ToolResultBlock):
                    answered.add(block.tool_use_id)
        return [call for call in requested if call.id not in answered]

    async def _settle_recovery(self, recorder: EventRecorder) -> None:
        """Settle the dangling calls found at resume time (plan.md §6.3).

        A persisted result is never re-run. Read-only calls are re-executed
        under a fresh event record; calls with uncertain side effects
        (writes, shell) are marked ``unknown`` and the model is told to
        verify current state instead of assuming anything.
        """
        for data in self._lost_jobs:
            await recorder.emit(EventType.BACKGROUND_JOB_LOST, {**data, "status": "lost"})
            self._append_message(self.session_id, Message(role="user", content=[TextBlock(
                text=f"后台任务 {data['job_id']} 在恢复会话时失联，结果未知，请核实。"
            )]))
        self._lost_jobs = []
        pending = self._pending_dangling
        self._pending_dangling = []
        if not pending:
            return

        history = (self._store.get_events(self.session_id)
                   if any(call.name == "skill_unload" for call in pending) else [])
        results: list[ToolResultBlock] = []
        for call in pending:
            lookup = getattr(self._store, "get_tool_result", None)
            settled = lookup(self.session_id, call.id) if lookup else None
            if settled is not None:
                results.append(settled)
                continue
            if call.name == "skill_unload":
                started = max((event.seq for event in history
                               if event.type is EventType.TOOL_CALL_START
                               and event.data.get("call_id") == call.id), default=-1)
                next_call = min((event.seq for event in history
                                 if event.type is EventType.TOOL_CALL_START
                                 and event.seq > started), default=float("inf"))
                if started >= 0 and any(started < event.seq < next_call
                       and event.type is EventType.SKILL_DEACTIVATED
                       and event.data.get("name") == call.input.get("name")
                       for event in history):
                    # The state transition was durable before the tool result.
                    # Replaying the unload would fail because it is already off.
                    output = f"已停用技能 {call.input['name']}"
                    await recorder.emit(EventType.TOOL_CALL_RESULT, {
                        "call_id": call.id, "name": call.name, "success": True,
                        "output_preview": output, "output_detail": output,
                        "recovered": True,
                    })
                    results.append(ToolResultBlock(
                        tool_use_id=call.id, content=output, is_error=False,
                    ))
                    continue
            tool = self._registry.get(call.name)
            if tool is not None and self._registry.execution_for(call).replay_safe:
                # Re-check the *current* policy; permissions may have changed
                # since the original call was interrupted.
                results.append(await self._execute_tool_call(
                    recorder, call, self.session_id, recovered=True
                ))
            else:
                await recorder.emit(
                    EventType.SIDE_EFFECT_UNKNOWN,
                    {"call_id": call.id, "name": call.name},
                )
                results.append(
                    ToolResultBlock(
                        tool_use_id=call.id,
                        content=(
                            f"副作用状态未知：进程在「{call.name}」执行后、结果落库前被中断。"
                            "不要假设它已执行或未执行；请先用只读工具核实当前状态再继续。"
                        ),
                        is_error=False,
                    )
                )
        assert self.session_id is not None
        self._append_message(self.session_id, Message(role="user", content=results))

    # -- streaming -----------------------------------------------------------

    async def _stream_assistant_turn(
        self,
    ) -> tuple[ModelResponse | None, ExitReason | None, str | None, Exception | None]:
        """Stream one assistant turn from the provider.

        Returns ``(response, None, None)`` on success, or
        ``(None, exit_reason, error)`` when the provider failed so the caller
        can finalize the session with that exit reason. The provider contract
        makes ``ResponseDone`` the terminal event carrying the fully
        assembled response, so the streamed text is taken from there.
        ``asyncio.CancelledError`` is deliberately not caught here.
        """
        from minicode.runtime.streaming import stream_response
        recorder = EventRecorder(self._store, self.session_id, self._on_event)
        lookup = getattr(self._store, "get_tool_result", None)
        result = await stream_response(
            self._provider, system=self._system_prompt, messages=self._messages,
            tools=self._registry.specs(), on_delta=self._on_text_delta,
            on_retry=recorder.emit,
            settled=lambda call_id: bool(lookup(self.session_id, call_id)) if lookup else False,
        )
        self._request_seconds = result.duration_s
        self._generation_seconds = result.generation_seconds
        self._first_token_seconds = result.first_token_seconds
        if result.usage_uncertain:
            self._usage = self._usage.model_copy(update={"available": False})
        return result.response, result.failure, str(result.error) if result.error else None, result.error

    # -- tool execution -------------------------------------------------------

    async def _execute_tool_call(
        self, recorder: EventRecorder, call: ToolUseBlock, session_id: str,
        *, recovered: bool = False,
    ) -> ToolResultBlock:
        """Run one tool call under policy/approval with start/result events.

        Unknown tools, denials, missing approvals and tool crashes all become
        error ``ToolResultBlock``s fed back to the model — the loop continues.
        Oversized outputs are spilled to the artifact store (the model gets a
        preview plus an archive reference).
        """
        self._check_deadline()
        started = time.monotonic()
        start_data = {"call_id": call.id, "name": call.name, "arguments": call.input}
        source = getattr(self._registry.get(call.name), "source", None)
        if source is not None:
            start_data["source"] = source
        if recovered:
            start_data["recovered"] = True
        await recorder.emit(EventType.TOOL_CALL_START, start_data)
        outcome = await self._resolve_outcome(recorder, call)
        if outcome.job_id is not None:
            self._announced_jobs.add(outcome.job_id)
            await recorder.emit(
                EventType.BACKGROUND_JOB_STARTED,
                {"job_id": outcome.job_id, "command": call.input.get("command", "")},
            )

        presented = self._maybe_spill(session_id, outcome)
        block = self._outcome_to_block(call, presented)
        result_data = {
                "call_id": call.id,
                "name": call.name,
                "success": outcome.success,
                "exit_code": outcome.exit_code,
                "error": outcome.error,
                "output_preview": presented.output[:_OUTPUT_PREVIEW_CHARS],
                "output_detail": presented.output if "[artifact:" not in presented.output else presented.output[:4000],
                "artifact_id": (match.group(1) if (
                    match := re.search(r"\[artifact:([A-Za-z0-9_-]+)\]", presented.output)
                ) else None),
                "truncated": "已截断" in presented.output or "truncated" in presented.output,
                "duration_s": round(time.monotonic() - started, 3),
            }
        if recovered:
            result_data["recovered"] = True
        if source is not None:
            result_data["source"] = source
        await recorder.checkpoint(EventType.TOOL_CALL_RESULT, result_data, result=block)
        return block

    def _maybe_spill(
        self, session_id: str, outcome: ToolOutcome
    ) -> ToolOutcome:
        """Archive the unabridged output before producing a model preview.

        Failure logs follow the same path as successful output.  A tool may
        have already placed a bounded preview in ``output``; ``full_output``
        is the authoritative source in that case.
        """
        original = outcome.full_output if outcome.full_output is not None else outcome.output
        if (self._artifact_store is None or (
            outcome.full_output is None
            and len(original.encode("utf-8")) <= _SPILL_LIMITS.max_output_chars
        )):
            return outcome.model_copy(update={"full_output": None})
        ref = self._artifact_store.spill(
            session_id, "tool_output", original
        )
        preview = outcome.output
        if len(preview.encode("utf-8")) > _SPILL_LIMITS.max_output_chars:
            from minicode.tools.base import utf8_prefix
            preview = utf8_prefix(preview, _SPILL_LIMITS.max_output_chars - 200)
        note = (
            f"\n...[完整输出已归档为 [artifact:{ref.artifact_id}]；"
            "用 read_artifact 分页续读]"
        )
        return outcome.model_copy(update={"output": preview + note, "full_output": None})

    async def _resolve_outcome(
        self, recorder: EventRecorder, call: ToolUseBlock
    ) -> ToolOutcome:
        """Resolve one tool call to an outcome: unknown-tool check, then the
        permission gate (DENY / ASK+approval / ALLOW), then execution."""
        from minicode.runtime.execution import execute_tool
        return await execute_tool(
            call, registry=self._registry, policy=self._policy,
            approval_handler=self._approval_handler, approval_lock=self._approval_lock,
            recorder=recorder, preflight=lambda call, args: self._tool_preflight(call, args, recorder),
            context=lambda: self._tool_context(call, recorder),
            check_deadline=self._check_deadline, approval_summary=self._approval_summary,
            waiting=lambda active: setattr(self, "_phase", RunPhase.APPROVAL if active else RunPhase.TOOLS),
        )

    async def _tool_preflight(self, call: ToolUseBlock, validated_input: dict,
                              recorder: EventRecorder) -> ToolOutcome | None:
        if self._project_instructions is not None and call.name in {"read", "ls", "grep", "edit", "write"}:
            target = validated_input.get("path")
            if isinstance(target, str):
                try:
                    target_path = (self._workspace / target).resolve()
                    if any(target_path.is_relative_to(scope) for scope in self._pending_instruction_scopes):
                        return ToolOutcome.failure("该目录的新 AGENTS.md 指令刚被加载；请按新指令重新调用工具。")
                    discovered = self._project_instructions.discover(target)
                except (ValueError, OSError, UnicodeError) as exc:
                    return ToolOutcome.failure(str(exc))
                if discovered:
                    self._pending_instruction_scopes.update(source.path.parent for source in discovered)
                    self._refresh_system_prompt()
                    for source in discovered:
                        await recorder.emit(EventType.PROJECT_INSTRUCTIONS, {
                            "path": str(source.path), "sha256": source.digest,
                        })
                    return ToolOutcome.failure("已加载该目录的 AGENTS.md 指令。请按新指令重新调用工具。")

        return None

    def _tool_context(self, call: ToolUseBlock, recorder: EventRecorder) -> ToolContext:
        return ToolContext(
            workspace=self._workspace,
            artifact_store=self._artifact_store,
            background_manager=self._background_manager,
            session_id=self.session_id,
            activate_skill=self.activate_skill if self._skills is not None else None,
            deactivate_skill=self.deactivate_skill if self._skills is not None else None,
            delegate=self._delegate if self._allow_delegation else None,
            task_store=self._task_store,
            project_memory=self._project_memory,
            on_output=(lambda output: recorder.emit(EventType.TOOL_OUTPUT, {
                "call_id": call.id, "name": call.name, "output_preview": output[-1000:],
            })),
        )

    async def _delegate(self, kind: str, task: str) -> ToolOutcome:
        from minicode.runtime.delegation import delegate
        return await delegate(self, kind, task)

    @staticmethod
    def _approval_summary(name: str, arguments: dict[str, Any]) -> str:
        """Compact human-readable description of a pending tool call."""
        if name == "bash" and isinstance(arguments.get("command"), str):
            return str(arguments["command"])
        return f"{name}: {json.dumps(arguments, ensure_ascii=False)[:200]}"

    @staticmethod
    def _outcome_to_block(call: ToolUseBlock, outcome: ToolOutcome) -> ToolResultBlock:
        """Wrap a tool outcome into the ``tool_result`` block sent to the model."""
        if outcome.success:
            content = outcome.output
        else:
            content = outcome.error or ""
            if outcome.output:
                content = f"{content}\n{outcome.output}"
        return ToolResultBlock(
            tool_use_id=call.id, content=content, is_error=not outcome.success
        )

    # -- P1: goal gate, compaction, background delivery ----------------------

    def _capture_protected_paths(self) -> None:
        """Snapshot protected paths before the model can change anything, so
        acceptance can later prove they stayed untouched."""
        snapshot = getattr(self._goal_checker, "protected_snapshot", None)
        if snapshot is None or self._protected_captured:
            return
        snapshot.capture()
        self._protected_captured = True

    def _bind_goal_session(self, *, resuming: bool = False) -> None:
        """Bind acceptance and its immutable protected baseline to this session."""
        from minicode.goals import AcceptanceSpec, EvidenceLedger, GoalChecker, ProtectedSnapshot

        assert self.session_id is not None
        saved = self._store.get_goal_state(self.session_id)
        checker = self._goal_checker
        if saved is not None:
            spec = AcceptanceSpec.model_validate(saved["spec"])
            if isinstance(checker, GoalChecker):
                protected = {item.path for item in spec.items if item.type == "protected"}
                requested = {item.path for item in checker.spec.items if item.type == "protected"}
                if not protected <= requested:
                    raise ValueError("resume cannot remove the session's protected paths")
                spec = checker.spec
            checker = GoalChecker(spec, self._workspace)
        if not isinstance(checker, GoalChecker):
            self._capture_protected_paths()
            return
        snapshot = ProtectedSnapshot(
            self._workspace, [item.path for item in checker.spec.items if item.type == "protected"]
        )
        if saved is not None:
            snapshot.restore(saved["baseline"])
        elif resuming:
            # Legacy sessions have no trustworthy initial baseline.
            snapshot.restore({"digests": {}, "unresolvable": {}})
        else:
            snapshot.capture()
        self._goal_checker = GoalChecker(checker.spec, self._workspace, snapshot)
        self._evidence_ledger = EvidenceLedger()
        self._protected_captured = True
        self._store.save_goal_state(
            self.session_id, {"spec": checker.spec.model_dump(), "baseline": snapshot.export()}
        )

    async def _goal_gate(
        self, recorder: EventRecorder, turn_started: float
    ) -> RunResult | None:
        """Acceptance gate for a model answer without tool calls.

        Returns the finalized :class:`RunResult` when the session may end
        (no gate configured, evidence still valid, or checks passed), or
        ``None`` when the failure report was appended and the loop should
        continue. Evidence is bound to the workspace fingerprint: code that
        changed after a passing check invalidates it and forces a re-check.
        """
        if self._goal_checker is None:
            return await self._finalize(recorder, ExitReason.COMPLETED, turn_started)

        # Fast path: passing evidence bound to the *current* workspace state
        # is still valid — no re-check needed. Any code change since the
        # evidence was recorded changes the fingerprint and forces a re-run.
        fingerprint = await self._current_workspace_fingerprint()
        if fingerprint is not None and self._evidence_valid(fingerprint):
            return await self._finalize(recorder, ExitReason.COMPLETED, turn_started)

        from minicode.goals import GoalChecker

        report = await (
            self._goal_checker.run(deadline=self._deadline)
            if isinstance(self._goal_checker, GoalChecker) else self._goal_checker.run()
        )
        self._check_deadline()
        await recorder.emit(
            EventType.GOAL_CHECK,
            {
                "passed": report.passed,
                "fingerprint": report.fingerprint,
                "attempt": self._goal_attempts + 1,
                "items": [
                    {
                        "item_id": item.item_id,
                        "kind": item.kind,
                        "passed": item.passed,
                        "exit_code": item.exit_code,
                        "detail": item.detail,
                    }
                    for item in report.items
                ],
            },
        )
        if report.passed:
            self._record_goal_evidence(report)
            return await self._finalize(recorder, ExitReason.COMPLETED, turn_started)

        max_attempts = getattr(self._goal_checker.spec, "max_fix_attempts", 3)
        self._goal_attempts += 1
        failure_report = self._goal_checker.format_failure_report(report)
        assert self.session_id is not None
        self._append_message(
            self.session_id,
            Message(role="user", content=[TextBlock(text=failure_report)]),
        )
        if self._goal_attempts > max_attempts:
            return await self._finalize(recorder, ExitReason.GOAL_NOT_MET, turn_started)
        return None

    def _record_goal_evidence(self, report: Any) -> None:
        """Bind the passing report to its workspace fingerprint (ledger when
        provided, internal fallback otherwise)."""
        if self._evidence_ledger is not None:
            self._evidence_ledger.record(report)
        self._own_pass_fingerprint = report.fingerprint

    async def _current_workspace_fingerprint(self) -> str | None:
        """Fingerprint of the workspace right now (lazy import so the runtime
        core stays decoupled from the goals package)."""
        from minicode.goals.checker import workspace_fingerprint

        try:
            return await asyncio.to_thread(workspace_fingerprint, self._workspace)
        except OSError:
            return None

    def _evidence_valid(self, fingerprint: str) -> bool:
        if self._evidence_ledger is not None:
            return self._evidence_ledger.valid_pass(fingerprint)
        return self._own_pass_fingerprint == fingerprint

    async def _compact_if_needed(self, recorder: EventRecorder, session_id: str) -> None:
        """Run layered compaction when the estimated context exceeds budget
        and persist the compacted view (originals live in artifacts)."""
        specs = self._registry.specs()
        from minicode.context.compact import ContextCompactor

        archive_ready = not (
            isinstance(self._compactor, ContextCompactor)
            and getattr(self._compactor, "_spill_fn", None) is None
        )
        if self._compactor is not None and archive_ready and self._compactor.needs_compaction(
            self._system_prompt, self._messages, specs
        ):
            result = self._compactor.compact(
                self._system_prompt, self._messages, specs
            )
            if result.changed:
                self._messages = result.messages
                self._store.replace_messages(session_id, self._messages)
                stats = result.stats
                await recorder.emit(
                    EventType.CONTEXT_COMPACTED,
                    {
                        "tokens_before": stats.tokens_before,
                        "tokens_after": stats.tokens_after,
                        "archived_units": stats.archived_units,
                        "shrunk_results": stats.shrunk_results,
                        "summarized_units": stats.summarized_units,
                    },
                )
        offloaded = 0
        while not self._fits_context(specs) and offloaded < 32:
            if not self._offload_largest_text(session_id):
                break
            offloaded += 1
        if offloaded:
            await recorder.emit(EventType.CONTEXT_COMPACTED, {"offloaded_blocks": offloaded})
        if not self._fits_context(specs):
            raise _ContextLimitExceeded(
                "上下文仍超过模型窗口；原始记录已保留。请切换更大上下文模型，"
                "或减少固定工具/系统提示内容后继续。"
            )

    def _fits_context(self, specs: list[Any]) -> bool:
        from minicode.context.estimate import estimate_messages_tokens

        estimate_fits = estimate_messages_tokens(
            self._system_prompt, self._messages, specs,
            reserve_output_tokens=0,
        ) * getattr(self._provider, "prompt_scale", 1.0) + 1024 <= self.context_window
        fits_hard_limit = getattr(self._compactor, "fits_hard_limit", None)
        return estimate_fits and (
            not callable(fits_hard_limit)
            or fits_hard_limit(self._system_prompt, self._messages, specs)
        )

    @staticmethod
    def _is_context_error(message: str) -> bool:
        lowered = message.lower()
        return any(marker in lowered for marker in (
            "context length", "context window", "context_length",
            "too many tokens", "prompt is too long", "token limit",
        ))

    def _offload_largest_text(self, session_id: str) -> bool:
        """Replace one oversized text block with a pageable artifact pointer.

        This is a last resort when deterministic compaction cannot fit a
        request. It never discards the original input or alters tool calls.
        """
        if self._artifact_store is None:
            return False
        candidates = [
            (len(block.text), message_index, block_index, block.text)
            for message_index, message in enumerate(self._messages)
            for block_index, block in enumerate(message.content)
            if isinstance(block, TextBlock)
            and len(block.text) > 1000
            and not block.text.startswith("原始内容过长，完整文本见")
        ]
        if not candidates:
            return False
        _, message_index, block_index, original = max(candidates)
        try:
            ref = self._artifact_store.spill(session_id, "context_input", original)
        except OSError:
            return False
        preview = original[:500]
        tail = original[-150:] if len(original) > 650 else ""
        replacement = (
            f"原始内容过长，完整文本见 [artifact:{ref.artifact_id}]。"
            "请先用 read_artifact 分页读取所需部分，再继续任务；不要仅凭以下预览作决定。\n"
            f"开头预览：\n{preview}\n末尾预览：\n{tail}"
        )
        updated = list(self._messages)
        blocks = list(updated[message_index].content)
        blocks[block_index] = TextBlock(text=replacement)
        updated[message_index] = Message(role=updated[message_index].role, content=blocks)
        self._store.replace_messages(session_id, updated)
        self._messages = updated
        return True

    async def _deliver_finished_jobs(
        self, recorder: EventRecorder, session_id: str
    ) -> None:
        """Hand finished background jobs to the model as user messages.

        Completion is keyed by the job id and each job is delivered exactly
        once (``poll_completed`` drains); the original tool call keeps its
        single ``started`` result, so no tool id ever gets a second result.
        """
        if self._background_manager is None:
            return
        for job in getattr(self._background_manager, "jobs", lambda: [])():
            if job.job_id not in self._announced_jobs:
                self._announced_jobs.add(job.job_id)
                await recorder.emit(EventType.BACKGROUND_JOB_STARTED, {
                    "job_id": job.job_id, "command": job.command,
                })
        peek = getattr(self._background_manager, "peek_completed", None)
        jobs = peek() if peek else self._background_manager.poll_completed()
        for job in jobs:
            artifact_id = None
            full_output = getattr(job, "full_output", None)
            if full_output is not None and self._artifact_store is not None:
                ref = self._artifact_store.spill(session_id, "command_output", full_output)
                artifact_id = ref.artifact_id
                job.output += f"\n[artifact:{artifact_id}] · 用 read_artifact 分页续读"
                job.full_output = None
            message = Message(role="user", content=[TextBlock(text=_format_bg_result(job))])
            event = recorder.commit(
                EventType.BACKGROUND_JOB_LOST if job.status == "lost" else EventType.BACKGROUND_JOB_COMPLETED,
                {
                    "job_id": job.job_id,
                    "command": job.command,
                    "status": job.status,
                    "exit_code": job.exit_code,
                    "output_preview": job.output[:_OUTPUT_PREVIEW_CHARS],
                    "artifact_id": artifact_id,
                    "truncated": artifact_id is not None,
                    "delivered": True,
                },
                message=message,
            )
            self._messages.append(message)
            acknowledge = getattr(self._background_manager, "ack_completed", None)
            if acknowledge is not None:
                acknowledge(job.job_id)
            await recorder.publish(event)

    # -- persistence helpers ---------------------------------------------------

    def _append_message(self, session_id: str, message: Message) -> None:
        """Mirror a message into memory and the store."""
        self._store.append_message(session_id, message)
        self._messages.append(message)

    def _counter_values(self, usage: Usage | None = None) -> dict[str, Any]:
        usage = usage if usage is not None else self._usage
        return dict(rounds=self._rounds, input_tokens=usage.input_tokens,
                    output_tokens=usage.output_tokens, cache_read_tokens=usage.cache_read_tokens,
                    cache_write_tokens=usage.cache_write_tokens, usage_available=usage.available)

    def _persist_counters(self) -> None:
        assert self.session_id is not None
        self._store.update_session(self.session_id, **self._counter_values())

    def _backfill_unexecuted_calls(
        self, session_id: str, calls: list[ToolUseBlock], reason: str
    ) -> None:
        if calls:
            self._append_message(session_id, Message(role="user", content=[
                ToolResultBlock(tool_use_id=call.id, is_error=True, content=reason)
                for call in calls
            ]))

    def _tool_cycle_stalled(
        self, calls: list[ToolUseBlock], results: list[ToolResultBlock]
    ) -> bool:
        signature = json.dumps(
            [
                (call.name, call.input, result.content, result.is_error)
                for call, result in zip(calls, results)
            ], ensure_ascii=False, sort_keys=True,
        )
        if signature == self._last_tool_signature:
            self._same_tool_rounds += 1
        else:
            self._same_tool_rounds = 1
            self._last_tool_signature = signature
        return self._same_tool_rounds >= 4

    async def _finalize(
        self,
        recorder: EventRecorder,
        exit_reason: ExitReason,
        turn_started: float,
        *,
        error: str | None = None,
    ) -> RunResult:
        """Single exit path: persist session state, emit ``SESSION_END`` and
        build the :class:`RunResult` for this turn. Any still-running
        background jobs are cancelled so the session never leaks processes."""
        assert self.session_id is not None
        self._phase = RunPhase.FINISHING
        if exit_reason not in (ExitReason.MAX_ROUNDS, ExitReason.TIME_BUDGET):
            await self._cancel_background_jobs()
        await self._deliver_finished_jobs(recorder, self.session_id)
        duration_s = time.monotonic() - turn_started
        await self._finish_checkpoint(recorder, exit_reason, error=error)
        return RunResult(
            session_id=self.session_id,
            exit_reason=exit_reason,
            rounds=self._rounds,
            total_usage=self._usage,
            duration_s=duration_s,
            error=error,
        )

    async def _cancel_background_jobs(self) -> None:
        """Finish background teardown before propagating another cancellation."""
        if self._background_manager is None:
            return
        cleanup = asyncio.create_task(self._background_manager.cancel_all())
        try:
            await asyncio.shield(cleanup)
        except asyncio.CancelledError:
            # Repeated Ctrl+C must not interrupt the actual process cleanup.
            while not cleanup.done():
                try:
                    await asyncio.shield(cleanup)
                except asyncio.CancelledError:
                    continue
            cleanup.result()
            raise

    async def _finalize_cancelled(self, recorder: EventRecorder) -> None:
        """Persist ``CANCELLED`` while task cancellation is being handled.

        Guarded so finalization itself can never raise — the original
        :class:`asyncio.CancelledError` must always propagate to the caller.
        """
        cleanup = asyncio.create_task(self._persist_cancelled(recorder))
        while not cleanup.done():
            try:
                await asyncio.shield(cleanup)
            except asyncio.CancelledError:
                continue
        cleanup.result()

    async def _persist_cancelled(self, recorder: EventRecorder) -> None:
        if self.session_id is None:
            return
        await self._cancel_background_jobs()
        try:
            await self._deliver_finished_jobs(recorder, self.session_id)
            await self._finish_checkpoint(recorder, ExitReason.CANCELLED)
        except Exception:  # noqa: BLE001 - finalization must not mask cancellation
            pass

    async def _finish_checkpoint(self, recorder: EventRecorder, exit_reason: ExitReason,
                                 *, error: str | None = None) -> None:
        pending = exit_reason is not ExitReason.COMPLETED
        status = "paused" if pending else "completed"
        data = {**self._session_end_data(exit_reason), "status": status, "run_id": self._run_id}
        if error is not None:
            data["error"] = error
        event = recorder.commit(EventType.SESSION_END, data, counters={
            **self._counter_values(), "status": status, "exit_reason": exit_reason.value,
        })
        self._last_exit_reason = exit_reason
        self._task_pending = pending
        await recorder.publish(event)

    def _session_end_data(self, exit_reason: ExitReason) -> dict[str, Any]:
        return {
            "exit_reason": exit_reason.value,
            "status": "paused" if self._task_pending else "completed",
            "rounds": self._rounds,
            "total_usage": {**self._usage.model_dump(),
                            "total_tokens": self._usage.total_tokens},
        }
