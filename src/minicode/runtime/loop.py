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
from contextlib import aclosing
import json
import re
import time
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
from minicode.providers.base import Provider, TextDelta
from minicode.providers.errors import ProviderAuthError, ProviderError, ProviderRequestError
from minicode.runtime.budget import BudgetChecker, SharedBudgetLedger
from minicode.runtime.events import EventCallback, EventRecorder
from minicode.runtime.prompt import build_system_prompt
from minicode.context.extensions import ProjectInstructions, SkillCatalog, Source
from minicode.security.policy import PermissionPolicy, PolicyBehavior
from minicode.storage import SessionStore
from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.registry import ToolRegistry

#: Called for every streamed piece of assistant text.
TextDeltaCallback = Callable[[str], Awaitable[None]]

#: Tool output echoed in TOOL_CALL_RESULT events is capped at this length.
_OUTPUT_PREVIEW_CHARS = 500

#: Tool output limits for generic tools; individual tools page before this.
_SPILL_LIMITS = ToolLimits()

#: Tool names that are safe to re-execute while resuming an interrupted
#: session: they only read, so re-running them cannot duplicate side effects.
_READ_ONLY_TOOLS = frozenset({"read", "ls", "grep", "read_artifact", "skills_list", "skill_load", "skill_unload", "skill_resource"})
_REPLAY_SAFE_TOOLS = _READ_ONLY_TOOLS | {"delegate"}
_MAX_PARALLEL_READS = 4


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

    # -- read-only state ----------------------------------------------------

    @property
    def usage(self) -> Usage:
        """Session-cumulative token usage."""
        return self._usage

    @property
    def last_model_usage(self) -> Usage | None:
        """Usage for the most recent model response, if one exists."""
        return self._last_model_usage

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

        return lookup_model(self._model).context_window

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
        from minicode.context.estimate import estimate_messages_tokens

        return int(estimate_messages_tokens(
            self._system_prompt,
            self._messages,
            self._registry.specs(),
            reserve_output_tokens=0,
        ) * getattr(self._provider, "prompt_scale", 1.0))

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
        if self.session_id is not None:
            self._store.update_session(self.session_id, provider=provider_name, model=model)
        self._provider = provider
        self._provider_name = provider_name
        self._model = model

    @staticmethod
    def _validate_provider(provider: Provider, provider_name: str, model: str) -> None:
        if provider.name != provider_name:
            raise ValueError("provider instance does not match provider_name")
        if getattr(provider, "model", model) != model:
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
        try:
            if first_turn:
                self._bind_goal_session()
            async with asyncio.timeout(
                max(0, self._deadline - time.monotonic()) if self._deadline is not None else None
            ):
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

                context_rejections = 0
                truncations = 0
                while True:
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
                    response, failure, error, provider_exc = await self._stream_assistant_turn()
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

                    self._append_message(
                        session_id, Message(role="assistant", content=list(response.blocks))
                    )
                    self._usage = self._usage + response.usage
                    self._last_model_usage = response.usage
                    self._budget_ledger.add_usage(response.usage)
                    await recorder.emit(
                        EventType.ASSISTANT_MESSAGE,
                        {
                            "text": response.text,
                            "tool_calls": [call.name for call in response.tool_calls],
                            "usage": {
                                "input_tokens": response.usage.input_tokens,
                                "output_tokens": response.usage.output_tokens,
                                "cache_read_tokens": response.usage.cache_read_tokens,
                                "cache_write_tokens": response.usage.cache_write_tokens,
                                "available": response.usage.available,
                                "total_tokens": response.usage.total_tokens,
                            },
                            "stop_reason": response.stop_reason.value,
                        },
                    )
                    self._persist_counters()

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
                    try:
                        calls = response.tool_calls
                        index = 0
                        while index < len(calls):
                            self._check_deadline()
                            if (calls[index].name == "delegate"
                                    and self._budget.max_total_tokens <= 0):
                                # Only independent child tasks overlap. With an
                                # explicit token cap, serial execution avoids
                                # two in-flight requests spending one balance.
                                batch: list[ToolUseBlock] = []
                                keys: set[tuple[str, str]] = set()
                                while (index + len(batch) < len(calls)
                                       and calls[index + len(batch)].name == "delegate"
                                       and len(batch) < 2):
                                    candidate = calls[index + len(batch)]
                                    key = (str(candidate.input.get("kind")),
                                           str(candidate.input.get("task")))
                                    if key in keys:
                                        break
                                    keys.add(key)
                                    batch.append(candidate)
                                tasks = [asyncio.create_task(
                                    self._execute_tool_call(recorder, call, session_id)
                                ) for call in batch]
                                try:
                                    tool_results.extend(await asyncio.gather(*tasks))
                                except BaseException:
                                    for task in tasks:
                                        task.cancel()
                                    await asyncio.gather(*tasks, return_exceptions=True)
                                    tool_results.extend(task.result() for task in tasks if
                                                        task.done() and not task.cancelled() and
                                                        task.exception() is None)
                                    raise
                                index += len(batch)
                                continue
                            if calls[index].name not in _READ_ONLY_TOOLS:
                                tool_results.append(
                                    await self._execute_tool_call(recorder, calls[index], session_id)
                                )
                                index += 1
                                continue
                            end = index
                            while end < len(calls) and calls[end].name in _READ_ONLY_TOOLS:
                                end += 1
                            # A write/unknown call is a barrier. Only adjacent
                            # built-in reads may overlap, and the provider sees
                            # their results in its original call order.
                            for start in range(index, end, _MAX_PARALLEL_READS):
                                batch = calls[start:min(start + _MAX_PARALLEL_READS, end)]
                                tasks = [
                                    asyncio.create_task(self._execute_tool_call(recorder, call, session_id))
                                    for call in batch
                                ]
                                try:
                                    tool_results.extend(await asyncio.gather(*tasks))
                                except BaseException:
                                    for task in tasks:
                                        task.cancel()
                                    await asyncio.gather(*tasks, return_exceptions=True)
                                    # Completed results must be durable even when
                                    # cancellation interrupts the batch.
                                    tool_results.extend(task.result() for task in tasks if
                                                        task.done() and not task.cancelled() and
                                                        task.exception() is None)
                                    raise
                                self._check_deadline()
                            index = end
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
        except TimeoutError:
            if user_message is not None and not user_recorded:
                self._append_message(session_id, Message(role="user", content=[TextBlock(text=user_message)]))
                self._task_pending = True
            return await self._finalize(recorder, ExitReason.TIME_BUDGET, turn_started)
        except asyncio.CancelledError:
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

    @classmethod
    def resume(
        cls,
        *,
        store: SessionStore,
        session_id: str,
        provider: Provider,
        registry: ToolRegistry,
        policy: PermissionPolicy,
        workspace: Path | None = None,
        provider_name: str | None = None,
        model: str | None = None,
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
    ) -> "AgentRuntime":
        """Continue a persisted session in a fresh process.

        Loads the stored conversation, usage and round count, restores the
        workspace/model from the session row (overridable) and marks tool
        calls whose outcome was lost to an interruption; they are settled at
        the start of the next :meth:`run_turn` (see :meth:`_settle_recovery`).
        Raises ``ValueError`` for an unknown session or a missing workspace.
        """
        summary = store.get_session(session_id)
        if summary is None:
            raise ValueError(f"unknown session: {session_id}")
        events = store.get_events(session_id)
        start_event = next((event for event in events
                            if event.type is EventType.SESSION_START), None)
        saved_budget = start_event.data.get("budget") if start_event else None
        effective_budget = budget if budget is not None else Budget()
        if isinstance(saved_budget, dict):
            try:
                saved_cap = int(saved_budget.get("max_total_tokens", 0))
            except (TypeError, ValueError):
                saved_cap = 0
            # A default zero in a resumed frontend must not silently remove
            # an explicit spending cap. A negative cap is an explicit opt-out.
            if saved_cap > 0 and effective_budget.max_total_tokens == 0:
                effective_budget = effective_budget.model_copy(
                    update={"max_total_tokens": saved_cap}
                )
        resolved_workspace = (Path(workspace) if workspace else Path(summary.workspace)).resolve()
        if not resolved_workspace.is_dir():
            raise ValueError(
                f"workspace no longer exists: {resolved_workspace} (session {session_id})"
            )
        if project_instructions is None and (
            (start_event and start_event.data.get("project_instructions"))
            or any(e.type is EventType.PROJECT_INSTRUCTIONS for e in events)
        ):
            project_instructions = ProjectInstructions(resolved_workspace)
        if skills is None and (
            (start_event and start_event.data.get("active_skills"))
            or any(e.type is EventType.SKILL_ACTIVATED for e in events)
        ):
            skills = SkillCatalog(resolved_workspace)

        resolved_provider = provider_name or provider.name
        resolved_model = model or getattr(provider, "model", summary.model)
        cls._validate_provider(provider, resolved_provider, resolved_model)
        runtime = cls(
            provider=provider,
            registry=registry,
            store=store,
            policy=policy,
            workspace=resolved_workspace,
            provider_name=resolved_provider,
            model=resolved_model,
            budget=effective_budget,
            approval_handler=approval_handler,
            on_text_delta=on_text_delta,
            on_event=on_event,
            system_prompt=system_prompt,
            compactor=compactor,
            goal_checker=goal_checker,
            evidence_ledger=evidence_ledger,
            background_manager=background_manager,
            artifact_store=artifact_store,
            project_instructions=project_instructions,
            skills=skills,
            allow_delegation=allow_delegation,
            shared_budget=shared_budget,
        )
        if project_instructions is not None:
            expected_sources = list(start_event.data.get("project_instructions", [])) if start_event else []
            expected_sources.extend(event.data for event in events
                                    if event.type is EventType.PROJECT_INSTRUCTIONS)
            for saved in expected_sources:
                path = Path(saved["path"])
                if path != resolved_workspace / "AGENTS.md":
                    relative = path.relative_to(resolved_workspace)
                    project_instructions.discover(str(relative.parent / "_scope_probe"))
                source = project_instructions.loaded.get(path)
                if source is None or source.digest != saved["sha256"]:
                    raise ValueError(f"project instructions changed since session start: {path}")
            runtime._refresh_system_prompt()
        active_skills = dict(start_event.data.get("active_skills", {})) if start_event else {}
        for event in events:
            if event.type is EventType.SKILL_ACTIVATED:
                active_skills[str(event.data["name"])] = str(event.data["sha256"])
            elif event.type is EventType.SKILL_DEACTIVATED:
                active_skills.pop(str(event.data["name"]), None)
        for name, digest in active_skills.items():
            if skills is None or skills.load(name).digest != digest:
                raise ValueError(f"activated skill changed since session start: {name}")
            runtime.activate_skill(name)
        runtime.session_id = session_id
        runtime._messages = store.get_messages(session_id)
        runtime._usage = Usage(
            input_tokens=summary.input_tokens,
            output_tokens=summary.output_tokens,
            cache_read_tokens=summary.cache_read_tokens,
            cache_write_tokens=summary.cache_write_tokens,
            available=summary.usage_available,
        )
        runtime._rounds = summary.rounds
        runtime._task_pending = bool(runtime._messages) and summary.status != ExitReason.COMPLETED.value
        try:
            runtime._last_exit_reason = ExitReason(summary.exit_reason) if summary.exit_reason else None
        except ValueError:
            runtime._last_exit_reason = None
        runtime._pending_dangling = cls._find_dangling_calls(runtime._messages)
        jobs: dict[str, dict[str, Any]] = {}
        event_usage = Usage()
        event_rounds = summary.rounds
        completed_responses = 0
        for event in events:
            if event.type is EventType.BACKGROUND_JOB_STARTED:
                jobs[event.data["job_id"]] = event.data
            elif event.type in (EventType.BACKGROUND_JOB_COMPLETED, EventType.BACKGROUND_JOB_LOST):
                jobs.pop(event.data["job_id"], None)
            elif event.type is EventType.ROUND_START:
                event_rounds = max(event_rounds, int(event.data.get("round", 0)))
            elif event.type is EventType.ASSISTANT_MESSAGE:
                completed_responses += 1
                data = event.data.get("usage")
                if isinstance(data, dict):
                    response_usage = Usage(
                        input_tokens=int(data.get("input_tokens", 0)),
                        output_tokens=int(data.get("output_tokens", 0)),
                        cache_read_tokens=int(data.get("cache_read_tokens", 0)),
                        cache_write_tokens=int(data.get("cache_write_tokens", 0)),
                        available=bool(data.get("available", False)),
                    )
                    event_usage += response_usage
                    runtime._last_model_usage = response_usage
            elif event.type is EventType.SUBAGENT_RESULT:
                data = event.data.get("usage")
                if isinstance(data, dict):
                    event_usage += Usage(**data)
        child_ids = {
            str(event.data["child_session_id"])
            for event in events if event.type is EventType.SUBAGENT_START
            and event.data.get("child_session_id")
        }
        settled_children = {
            str(event.data["child_session_id"])
            for event in events if event.type is EventType.SUBAGENT_RESULT
            and event.data.get("child_session_id")
        }
        for child_id in child_ids:
            child_events = store.get_events(child_id)
            child_responses = [event for event in child_events
                               if event.type is EventType.ASSISTANT_MESSAGE]
            completed_responses += len(child_responses)
            if child_id not in settled_children:
                for child_event in child_responses:
                    data = child_event.data.get("usage")
                    if isinstance(data, dict):
                        event_usage += Usage(**data)
        runtime._rounds = event_rounds
        restore_progress = getattr(provider, "restore_progress", None)
        resume_identity = getattr(provider, "resume_identity", None)
        saved_identity = (start_event.data.get("provider_resume_identity")
                          if start_event else None)
        if (callable(restore_progress) and callable(resume_identity)
                and saved_identity is not None
                and resume_identity() == saved_identity):
            ids = [
                block.id
                for message in runtime._messages for block in message.content
                if isinstance(block, ToolUseBlock)
            ]
            ids.extend(
                str(event.data.get("call_id", ""))
                for event in events if event.type is EventType.TOOL_CALL_START
            )
            for child_id in child_ids:
                ids.extend(
                    str(event.data.get("call_id", ""))
                    for event in store.get_events(child_id)
                    if event.type is EventType.TOOL_CALL_START
                )
            fake_call_counter = max((
                int(call_id.removeprefix("fake_tool_"))
                for call_id in ids
                if call_id.startswith("fake_tool_")
                and call_id.removeprefix("fake_tool_").isdigit()
            ), default=0)
            restore_progress(completed_responses, fake_call_counter)
        if event_usage.total_tokens > runtime._usage.total_tokens:
            runtime._usage = event_usage
        runtime._lost_jobs = list(jobs.values())
        runtime._bind_goal_session(resuming=True)
        store.update_session(
            session_id, workspace=str(resolved_workspace),
            provider=resolved_provider, model=resolved_model,
            rounds=runtime._rounds,
            input_tokens=runtime._usage.input_tokens,
            output_tokens=runtime._usage.output_tokens,
            cache_read_tokens=runtime._usage.cache_read_tokens,
            cache_write_tokens=runtime._usage.cache_write_tokens,
            usage_available=runtime._usage.available,
        )
        return runtime

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

        results: list[ToolResultBlock] = []
        for call in pending:
            if call.name in _REPLAY_SAFE_TOOLS:
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
        for attempt in range(3):
            saw_text = False
            try:
                async with aclosing(self._provider.stream(
                    system=self._system_prompt,
                    messages=self._messages,
                    tools=self._registry.specs(),
                )) as stream:
                    async for event in stream:
                        if isinstance(event, TextDelta):
                            saw_text = True
                            if self._on_text_delta is not None:
                                await self._on_text_delta(event.text)
                        else:
                            return event.response, None, None, None
                raise ProviderError("provider stream ended without a final response")
            except (ProviderAuthError, ProviderRequestError) as exc:
                self._usage = self._usage.model_copy(update={"available": False})
                return None, ExitReason.PROVIDER_ERROR, str(exc), exc
            except ProviderError as exc:
                self._usage = self._usage.model_copy(update={"available": False})
                if saw_text or attempt == 2:
                    return None, ExitReason.PROVIDER_ERROR, str(exc), exc
                await asyncio.sleep(min(2.0, 0.25 * (2 ** attempt)))
            except Exception as exc:  # noqa: BLE001 - provider bugs are resumable
                self._usage = self._usage.model_copy(update={"available": False})
                return None, ExitReason.INTERNAL_ERROR, str(exc), exc
        raise AssertionError("retry loop must return")

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
        await recorder.emit(EventType.TOOL_CALL_RESULT, result_data)
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
        tool = self._registry.get(call.name)
        if tool is None:
            return ToolOutcome.failure(f"unknown tool: {call.name}")

        if self._project_instructions is not None and call.name in {"read", "ls", "grep", "edit", "write"}:
            target = call.input.get("path")
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

        decision = await self._policy.check(call.name, call.input)
        if decision.behavior is PolicyBehavior.DENY:
            reason = f": {decision.reason}" if decision.reason else ""
            return ToolOutcome.failure(f"permission denied by policy{reason}")

        if decision.behavior is PolicyBehavior.ASK:
            if self._approval_handler is None:
                return ToolOutcome.failure(
                    "approval required but no approval handler is configured"
                )
            # An overridden policy may ask even for a nominally read-only
            # tool. Keep its dialogs sequential while the approved reads may
            # still overlap afterwards.
            async with self._approval_lock:
                summary = self._approval_summary(call.name, call.input)
                await recorder.emit(
                    EventType.APPROVAL_REQUEST,
                    {"call_id": call.id, "tool_name": call.name, "summary": summary},
                )
                approved = await self._approval_handler(
                    ApprovalRequest(tool_name=call.name, arguments=call.input, summary=summary)
                )
                await recorder.emit(
                    EventType.APPROVAL_DECISION,
                    {
                        "call_id": call.id,
                        "tool_name": call.name,
                        "granted": approved.granted,
                        "reason": approved.reason,
                    },
                )
            if not approved.granted:
                reason = f": {approved.reason}" if approved.reason else ""
                return ToolOutcome.failure(f"approval denied{reason}")

        return await self._run_tool(call, tool, recorder)

    async def _run_tool(self, call: ToolUseBlock, tool: Any,
                        recorder: EventRecorder) -> ToolOutcome:
        """Execute one approved tool call with the shared services attached."""
        self._check_deadline()
        context = ToolContext(
            workspace=self._workspace,
            artifact_store=self._artifact_store,
            background_manager=self._background_manager,
            session_id=self.session_id,
            activate_skill=self.activate_skill if self._skills is not None else None,
            deactivate_skill=self.deactivate_skill if self._skills is not None else None,
            delegate=self._delegate if self._allow_delegation else None,
            on_output=(lambda output: recorder.emit(EventType.TOOL_OUTPUT, {
                "call_id": call.id, "name": call.name, "output_preview": output[-1000:],
            })),
        )
        for attempt in range(2):
            try:
                return await tool.run(call.input, context)
            except Exception as exc:  # noqa: BLE001 - tool crash becomes a result
                if call.name not in _READ_ONLY_TOOLS or attempt == 1:
                    return ToolOutcome.failure(f"internal tool error: {exc}")
                await asyncio.sleep(0.1)
        raise AssertionError("tool retry loop must return")

    async def _delegate(self, kind: str, task: str) -> ToolOutcome:
        """Run a read-only child under the parent's model and shared budget."""
        from minicode.context.compact import CompactConfig, ContextCompactor
        from minicode.security.policy import DefaultPolicy
        from minicode.tools.artifacts import ReadArtifactTool
        from minicode.tools.files import LsTool, ReadTool
        from minicode.tools.search import GrepTool

        agent_source = None
        agent_instructions = ""
        if kind not in {"explore", "review"}:
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
            + "\n\n你是只读子助手。只能调查与审查，不得执行 shell 或修改文件。"
            "最终只输出 JSON 对象，键为 summary（字符串）、findings（字符串列表）、"
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
                policy=DefaultPolicy(), workspace=self._workspace,
                provider_name=self._provider_name, model=self._model,
                budget=self._budget, artifact_store=self._artifact_store,
                project_instructions=ProjectInstructions(self._workspace),
                allow_delegation=False, on_event=on_child_event,
                shared_budget=self._budget_ledger, system_prompt=child_prompt,
            )
            child = (AgentRuntime.resume(session_id=paused_id, **common)
                     if isinstance(paused_id, str) else AgentRuntime(**common))
            self._child_sessions[key] = child
        else:
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
            "usage": child.usage.model_dump(), "evidence_refs": verified,
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
        for job in self._background_manager.poll_completed():
            artifact_id = None
            full_output = getattr(job, "full_output", None)
            if full_output is not None and self._artifact_store is not None:
                ref = self._artifact_store.spill(session_id, "command_output", full_output)
                artifact_id = ref.artifact_id
                job.output += f"\n[artifact:{artifact_id}] · 用 read_artifact 分页续读"
                job.full_output = None
            await recorder.emit(
                EventType.BACKGROUND_JOB_LOST if job.status == "lost" else EventType.BACKGROUND_JOB_COMPLETED,
                {
                    "job_id": job.job_id,
                    "command": job.command,
                    "status": job.status,
                    "exit_code": job.exit_code,
                    "output_preview": job.output[:_OUTPUT_PREVIEW_CHARS],
                    "artifact_id": artifact_id,
                    "truncated": artifact_id is not None,
                },
            )
            self._append_message(
                session_id,
                Message(role="user", content=[TextBlock(text=_format_bg_result(job))]),
            )

    # -- persistence helpers ---------------------------------------------------

    def _append_message(self, session_id: str, message: Message) -> None:
        """Mirror a message into memory and the store."""
        self._store.append_message(session_id, message)
        self._messages.append(message)

    def _persist_counters(self) -> None:
        assert self.session_id is not None
        self._store.update_session(
            self.session_id,
            rounds=self._rounds,
            input_tokens=self._usage.input_tokens,
            output_tokens=self._usage.output_tokens,
            cache_read_tokens=self._usage.cache_read_tokens,
            cache_write_tokens=self._usage.cache_write_tokens,
            usage_available=self._usage.available,
        )

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
        if exit_reason not in (ExitReason.MAX_ROUNDS, ExitReason.TIME_BUDGET):
            await self._cancel_background_jobs()
        await self._deliver_finished_jobs(recorder, self.session_id)
        duration_s = time.monotonic() - turn_started
        self._persist_session(exit_reason)
        data = self._session_end_data(exit_reason)
        if error is not None:
            data["error"] = error
        await recorder.emit(EventType.SESSION_END, data)
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
            self._persist_session(ExitReason.CANCELLED)
            await recorder.emit(
                EventType.SESSION_END, self._session_end_data(ExitReason.CANCELLED)
            )
        except Exception:  # noqa: BLE001 - finalization must not mask cancellation
            pass

    def _persist_session(self, exit_reason: ExitReason) -> None:
        assert self.session_id is not None
        self._last_exit_reason = exit_reason
        self._task_pending = exit_reason is not ExitReason.COMPLETED
        self._store.update_session(
            self.session_id,
            status="paused" if self._task_pending else "completed",
            exit_reason=exit_reason.value,
            rounds=self._rounds,
            input_tokens=self._usage.input_tokens,
            output_tokens=self._usage.output_tokens,
            cache_read_tokens=self._usage.cache_read_tokens,
            cache_write_tokens=self._usage.cache_write_tokens,
            usage_available=self._usage.available,
        )

    def _session_end_data(self, exit_reason: ExitReason) -> dict[str, Any]:
        return {
            "exit_reason": exit_reason.value,
            "status": "paused" if self._task_pending else "completed",
            "rounds": self._rounds,
            "total_usage": {
                "input_tokens": self._usage.input_tokens,
                "output_tokens": self._usage.output_tokens,
                "cache_read_tokens": self._usage.cache_read_tokens,
                "cache_write_tokens": self._usage.cache_write_tokens,
                "available": self._usage.available,
                "total_tokens": self._usage.total_tokens,
            },
        }
