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
from minicode.providers.errors import ProviderError
from minicode.runtime.budget import BudgetChecker
from minicode.runtime.events import EventCallback, EventRecorder
from minicode.runtime.prompt import build_system_prompt
from minicode.security.policy import PermissionPolicy, PolicyBehavior
from minicode.storage import SessionStore
from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.registry import ToolRegistry

#: Called for every streamed piece of assistant text.
TextDeltaCallback = Callable[[str], Awaitable[None]]

#: Tool output echoed in TOOL_CALL_RESULT events is capped at this length.
_OUTPUT_PREVIEW_CHARS = 500

#: Spill thresholds (same defaults as ToolLimits; kept here so the loop does
#: not reach into per-tool limits configured elsewhere).
_SPILL_LIMITS = ToolLimits()

#: Tool names that are safe to re-execute while resuming an interrupted
#: session: they only read, so re-running them cannot duplicate side effects.
_READ_ONLY_TOOLS = frozenset({"read", "ls", "grep", "read_artifact"})
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
        self._system_prompt = (
            system_prompt
            if system_prompt is not None
            else build_system_prompt(str(workspace.resolve()), registry.names())
        )
        self._compactor = compactor
        self._goal_checker = goal_checker
        self._evidence_ledger = evidence_ledger
        self._background_manager = background_manager
        self._artifact_store = artifact_store

        self.session_id: str | None = None  # created on first run_turn
        self._messages: list[Message] = []  # mirrors the persisted conversation
        self._usage = Usage()
        self._rounds = 0
        self._goal_attempts = 0
        self._own_pass_fingerprint: str | None = None  # fallback without a ledger
        self._protected_captured = False
        self._pending_dangling: list[ToolUseBlock] = []  # set by resume()
        self._deadline: float | None = None
        self._announced_jobs: set[str] = set()
        self._lost_jobs: list[dict[str, Any]] = []
        self._approval_lock = asyncio.Lock()

    # -- read-only state ----------------------------------------------------

    @property
    def usage(self) -> Usage:
        """Session-cumulative token usage."""
        return self._usage

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
        """How much prompt the active model can take *and still answer*.

        ``context_window`` minus the model's maximum response length: the
        window has to hold both, so compacting only against the window would
        let a long answer push the request past it. This is the number
        compaction should trigger on — not the session's token budget, which
        counts cumulative spend rather than context size.
        """
        return max(0, self.context_window - self.effective_max_output_tokens())

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

        return estimate_messages_tokens(
            self._system_prompt,
            self._messages,
            self._registry.specs(),
            reserve_output_tokens=0,
        )

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
    # The loop exits only through finalize paths; budget exhaustion is just
    # another finalize (MAX_ROUNDS), never a break/exception.

    async def run_turn(self, user_message: str) -> RunResult:
        """Run one user turn to completion and return the outcome.

        On the first call the session row is created and ``SESSION_START``
        is emitted (a configured protected-path snapshot is captured here,
        before the model can change anything). Each round injects finished
        background-job results, compacts the context when it exceeds the
        budget, streams one assistant response, persists it, executes its
        tool calls one at a time under the permission policy, and feeds the
        results back until the model produces a final answer — which, with
        an acceptance gate configured, only ends the session once the goal
        checks pass — or a budget is exhausted. Adjacent built-in reads may
        run in bounded parallel batches; writes and unknown tools remain
        ordering barriers.

        Cancellation: see the class docstring — ``CANCELLED`` is persisted
        and :class:`asyncio.CancelledError` is re-raised to the caller.
        """
        turn_started = time.monotonic()
        # CLI chat creates a fresh event loop for each user turn.
        self._approval_lock = asyncio.Lock()

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
        if first_turn:
            self._bind_goal_session()
        user_recorded = False
        self._deadline = turn_started + self._budget.max_seconds
        try:
            async with asyncio.timeout(max(0, self._deadline - time.monotonic())):
                if first_turn:
                    await recorder.emit(
                        EventType.SESSION_START,
                        {
                            "workspace": str(self._workspace.resolve()),
                            "provider": self._provider_name,
                            "model": self._model,
                        },
                    )
                # Settle tool calls interrupted before their result was persisted.
                await self._settle_recovery(recorder)

                # Wall-clock budget is per turn; rounds/tokens are seeded cumulatively.
                checker = BudgetChecker(
                    self._budget, start_usage=self._usage, start_rounds=self._rounds
                )

                user_msg = Message(role="user", content=[TextBlock(text=user_message)])
                self._append_message(session_id, user_msg)
                user_recorded = True

                while True:
                    self._check_deadline()
                    if checker.rounds_exceeded(self._rounds):
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

                    self._rounds += 1
                    await recorder.emit(EventType.ROUND_START, {"round": self._rounds})

                    self._check_deadline()
                    response, failure, error = await self._stream_assistant_turn()
                    self._check_deadline()
                    if response is None:
                        assert failure is not None  # always set when response is None
                        return await self._finalize(
                            recorder, failure, turn_started, error=error
                        )

                    self._append_message(
                        session_id, Message(role="assistant", content=list(response.blocks))
                    )
                    self._usage = self._usage + response.usage
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

                    # Token budget fires before any tool of this round runs: side
                    # effects must not start once the budget is already blown.
                    if checker.tokens_exceeded(self._usage):
                        return await self._finalize(
                            recorder, ExitReason.TOKEN_BUDGET, turn_started
                        )

                    if response.stop_reason is StopReason.MAX_TOKENS:
                        if response.tool_calls:
                            self._append_message(session_id, Message(role="user", content=[
                                ToolResultBlock(tool_use_id=call.id, is_error=True,
                                                content="未执行：模型响应被截断，请重新生成完整调用。")
                                for call in response.tool_calls
                            ]))
                        return await self._finalize(recorder, ExitReason.MAX_TOKENS, turn_started)

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
        except TimeoutError:
            if not user_recorded:
                self._append_message(session_id, Message(role="user", content=[TextBlock(text=user_message)]))
            return await self._finalize(recorder, ExitReason.TIME_BUDGET, turn_started)
        except asyncio.CancelledError:
            await self._finalize_cancelled(recorder)
            raise  # never swallow cancellation
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
        resolved_workspace = (Path(workspace) if workspace else Path(summary.workspace)).resolve()
        if not resolved_workspace.is_dir():
            raise ValueError(
                f"workspace no longer exists: {resolved_workspace} (session {session_id})"
            )

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
            budget=budget
            if budget is not None
            else Budget(
                # A resumed session gets headroom in rounds and no token cap:
                # its stored totals are already large by definition, and
                # capping against them would kill the continuation on the
                # first round.
                max_rounds=max(summary.rounds + 10, 10),
            ),
            approval_handler=approval_handler,
            on_text_delta=on_text_delta,
            on_event=on_event,
            system_prompt=system_prompt,
            compactor=compactor,
            goal_checker=goal_checker,
            evidence_ledger=evidence_ledger,
            background_manager=background_manager,
            artifact_store=artifact_store,
        )
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
        runtime._pending_dangling = cls._find_dangling_calls(runtime._messages)
        jobs: dict[str, dict[str, Any]] = {}
        for event in store.get_events(session_id):
            if event.type is EventType.BACKGROUND_JOB_STARTED:
                jobs[event.data["job_id"]] = event.data
            elif event.type in (EventType.BACKGROUND_JOB_COMPLETED, EventType.BACKGROUND_JOB_LOST):
                jobs.pop(event.data["job_id"], None)
        runtime._lost_jobs = list(jobs.values())
        runtime._bind_goal_session(resuming=True)
        store.update_session(
            session_id, workspace=str(resolved_workspace),
            provider=resolved_provider, model=resolved_model,
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
            tool = (
                self._registry.get(call.name) if call.name in _READ_ONLY_TOOLS else None
            )
            if tool is not None:
                # Read-only calls are safe to re-execute, and the read-only
                # set is fixed, so no policy re-check is needed.
                await recorder.emit(
                    EventType.TOOL_CALL_START,
                    {
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.input,
                        "recovered": True,
                    },
                )
                outcome = self._maybe_spill(
                    self.session_id, await self._run_tool(call, tool)
                )
                await recorder.emit(
                    EventType.TOOL_CALL_RESULT,
                    {
                        "call_id": call.id,
                        "name": call.name,
                        "success": outcome.success,
                        "exit_code": outcome.exit_code,
                        "error": outcome.error,
                        "output_preview": outcome.output[:_OUTPUT_PREVIEW_CHARS],
                        "recovered": True,
                    },
                )
                results.append(self._outcome_to_block(call, outcome))
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
    ) -> tuple[ModelResponse | None, ExitReason | None, str | None]:
        """Stream one assistant turn from the provider.

        Returns ``(response, None, None)`` on success, or
        ``(None, exit_reason, error)`` when the provider failed so the caller
        can finalize the session with that exit reason. The provider contract
        makes ``ResponseDone`` the terminal event carrying the fully
        assembled response, so the streamed text is taken from there.
        ``asyncio.CancelledError`` is deliberately not caught here.
        """
        try:
            async with aclosing(self._provider.stream(
                system=self._system_prompt,
                messages=self._messages,
                tools=self._registry.specs(),
            )) as stream:
                async for event in stream:
                    if isinstance(event, TextDelta):
                        if self._on_text_delta is not None:
                            await self._on_text_delta(event.text)
                    else:
                        return event.response, None, None
        except ProviderError as exc:
            return None, ExitReason.PROVIDER_ERROR, str(exc)
        except Exception as exc:  # noqa: BLE001 - any provider bug must not kill the loop
            return None, ExitReason.INTERNAL_ERROR, str(exc)
        return (
            None,
            ExitReason.INTERNAL_ERROR,
            "provider stream ended without a final response",
        )

    # -- tool execution -------------------------------------------------------

    async def _execute_tool_call(
        self, recorder: EventRecorder, call: ToolUseBlock, session_id: str
    ) -> ToolResultBlock:
        """Run one tool call under policy/approval with start/result events.

        Unknown tools, denials, missing approvals and tool crashes all become
        error ``ToolResultBlock``s fed back to the model — the loop continues.
        Oversized outputs are spilled to the artifact store (the model gets a
        preview plus an archive reference).
        """
        started = time.monotonic()
        await recorder.emit(
            EventType.TOOL_CALL_START,
            {"call_id": call.id, "name": call.name, "arguments": call.input},
        )
        outcome = await self._resolve_outcome(recorder, call)
        if outcome.job_id is not None:
            self._announced_jobs.add(outcome.job_id)
            await recorder.emit(
                EventType.BACKGROUND_JOB_STARTED,
                {"job_id": outcome.job_id, "command": call.input.get("command", "")},
            )

        presented = self._maybe_spill(session_id, outcome)
        block = self._outcome_to_block(call, presented)
        await recorder.emit(
            EventType.TOOL_CALL_RESULT,
            {
                "call_id": call.id,
                "name": call.name,
                "success": outcome.success,
                "exit_code": outcome.exit_code,
                "error": outcome.error,
                "output_preview": presented.output[:_OUTPUT_PREVIEW_CHARS],
                "output_detail": presented.output[:4000],
                "duration_s": round(time.monotonic() - started, 3),
            },
        )
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
        if self._artifact_store is None or len(original) <= _SPILL_LIMITS.spill_threshold_chars:
            return outcome.model_copy(update={"full_output": None})
        ref = self._artifact_store.spill(
            session_id, "tool_output", original
        )
        preview = original[: _SPILL_LIMITS.spill_preview_chars]
        note = (
            f"\n...[输出共 {len(original)} 字符，已转存为 artifact "
            f"[artifact:{ref.artifact_id}]；完整内容保存在会话归档中]"
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

        return await self._run_tool(call, tool)

    async def _run_tool(self, call: ToolUseBlock, tool: Any) -> ToolOutcome:
        """Execute one approved tool call with the shared services attached."""
        self._check_deadline()
        try:
            return await tool.run(
                call.input,
                ToolContext(
                    workspace=self._workspace,
                    artifact_store=self._artifact_store,
                    background_manager=self._background_manager,
                    session_id=self.session_id,
                ),
            )
        except Exception as exc:  # noqa: BLE001 - a tool crash must never escape the loop
            return ToolOutcome.failure(f"internal tool error: {exc}")

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
        fingerprint = self._current_workspace_fingerprint()
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
        if self._goal_attempts > max_attempts:
            return await self._finalize(recorder, ExitReason.GOAL_NOT_MET, turn_started)
        failure_report = self._goal_checker.format_failure_report(report)
        assert self.session_id is not None
        self._append_message(
            self.session_id,
            Message(role="user", content=[TextBlock(text=failure_report)]),
        )
        return None

    def _record_goal_evidence(self, report: Any) -> None:
        """Bind the passing report to its workspace fingerprint (ledger when
        provided, internal fallback otherwise)."""
        if self._evidence_ledger is not None:
            self._evidence_ledger.record(report)
        self._own_pass_fingerprint = report.fingerprint

    def _current_workspace_fingerprint(self) -> str | None:
        """Fingerprint of the workspace right now (lazy import so the runtime
        core stays decoupled from the goals package)."""
        from minicode.goals.checker import workspace_fingerprint

        try:
            return workspace_fingerprint(self._workspace)
        except OSError:
            return None

    def _evidence_valid(self, fingerprint: str) -> bool:
        if self._evidence_ledger is not None:
            return self._evidence_ledger.valid_pass(fingerprint)
        return self._own_pass_fingerprint == fingerprint

    async def _compact_if_needed(self, recorder: EventRecorder, session_id: str) -> None:
        """Run layered compaction when the estimated context exceeds budget
        and persist the compacted view (originals live in artifacts)."""
        if self._compactor is None:
            return
        specs = self._registry.specs()
        if self._compactor.needs_compaction(self._system_prompt, self._messages, specs):
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
        fits_hard_limit = getattr(self._compactor, "fits_hard_limit", None)
        if callable(fits_hard_limit) and not fits_hard_limit(
            self._system_prompt, self._messages, specs
        ):
            raise _ContextLimitExceeded(
                "压缩后上下文仍超过模型窗口；请缩小输入、切换更大上下文模型，"
                "或通过 read_artifact 按需取回已归档内容。"
            )

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
            await recorder.emit(
                EventType.BACKGROUND_JOB_LOST if job.status == "lost" else EventType.BACKGROUND_JOB_COMPLETED,
                {
                    "job_id": job.job_id,
                    "command": job.command,
                    "status": job.status,
                    "exit_code": job.exit_code,
                    "output_preview": job.output[:_OUTPUT_PREVIEW_CHARS],
                },
            )
            self._append_message(
                session_id,
                Message(role="user", content=[TextBlock(text=_format_bg_result(job))]),
            )

    # -- persistence helpers ---------------------------------------------------

    def _append_message(self, session_id: str, message: Message) -> None:
        """Mirror a message into memory and the store."""
        self._messages.append(message)
        self._store.append_message(session_id, message)

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
        if exit_reason is not ExitReason.TIME_BUDGET:
            self._check_deadline()
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
        self._store.update_session(
            self.session_id,
            status=exit_reason.value,
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
