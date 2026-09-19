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
from minicode.tools.base import ToolContext
from minicode.tools.registry import ToolRegistry

#: Called for every streamed piece of assistant text.
TextDeltaCallback = Callable[[str], Awaitable[None]]

#: Tool output echoed in TOOL_CALL_RESULT events is capped at this length.
_OUTPUT_PREVIEW_CHARS = 500


class AgentRuntime:
    """Stateful agent loop for one session.

    The session row is created lazily on the first :meth:`run_turn` call;
    afterwards messages, usage, rounds and events accumulate across turns.

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

        self.session_id: str | None = None  # created on first run_turn
        self._messages: list[Message] = []  # mirrors the persisted conversation
        self._usage = Usage()
        self._rounds = 0

    # -- read-only state ----------------------------------------------------

    @property
    def usage(self) -> Usage:
        """Session-cumulative token usage."""
        return self._usage

    @property
    def rounds(self) -> int:
        """Session-cumulative round count."""
        return self._rounds

    # -- turn loop -----------------------------------------------------------
    # The loop exits only through finalize paths; budget exhaustion is just
    # another finalize (MAX_ROUNDS), never a break/exception.

    async def run_turn(self, user_message: str) -> RunResult:
        """Run one user turn to completion and return the outcome.

        On the first call the session row is created and ``SESSION_START``
        is emitted. Each round streams one assistant response, persists it,
        executes its tool calls one at a time under the permission policy,
        and feeds the results back until the model produces a final answer
        or a budget is exhausted.

        Cancellation: see the class docstring — ``CANCELLED`` is persisted
        and :class:`asyncio.CancelledError` is re-raised to the caller.
        """
        turn_started = time.monotonic()

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
            await recorder.emit(
                EventType.SESSION_START,
                {
                    "workspace": str(self._workspace.resolve()),
                    "provider": self._provider_name,
                    "model": self._model,
                },
            )

        # Wall-clock budget is per turn; rounds/tokens are seeded cumulatively.
        checker = BudgetChecker(
            self._budget, start_usage=self._usage, start_rounds=self._rounds
        )

        user_msg = Message(role="user", content=[TextBlock(text=user_message)])
        self._append_message(session_id, user_msg)

        try:
            while True:
                if checker.time_exceeded():
                    return await self._finalize(
                        recorder, ExitReason.TIME_BUDGET, turn_started
                    )
                if checker.rounds_exceeded(self._rounds):
                    return await self._finalize(
                        recorder, ExitReason.MAX_ROUNDS, turn_started
                    )

                self._rounds += 1
                await recorder.emit(EventType.ROUND_START, {"round": self._rounds})

                response, failure, error = await self._stream_assistant_turn()
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

                if not response.tool_calls:
                    return await self._finalize(
                        recorder, ExitReason.COMPLETED, turn_started
                    )

                tool_results: list[ToolResultBlock] = []
                for call in response.tool_calls:  # executed one at a time, in order
                    tool_results.append(await self._execute_tool_call(recorder, call))
                if tool_results:
                    self._append_message(
                        session_id, Message(role="user", content=tool_results)
                    )
                await recorder.emit(EventType.ROUND_END, {"round": self._rounds})
        except asyncio.CancelledError:
            await self._finalize_cancelled(recorder)
            raise  # never swallow cancellation

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
            async for event in self._provider.stream(
                system=self._system_prompt,
                messages=self._messages,
                tools=self._registry.specs(),
            ):
                if isinstance(event, TextDelta):
                    if self._on_text_delta is not None:
                        await self._on_text_delta(event.text)
                else:  # ResponseDone: terminal, fully assembled response
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
        self, recorder: EventRecorder, call: ToolUseBlock
    ) -> ToolResultBlock:
        """Run one tool call under policy/approval with start/result events.

        Unknown tools, denials, missing approvals and tool crashes all become
        error ``ToolResultBlock``s fed back to the model — the loop continues.
        """
        await recorder.emit(
            EventType.TOOL_CALL_START,
            {"call_id": call.id, "name": call.name, "arguments": call.input},
        )
        outcome = await self._resolve_outcome(recorder, call)
        block = self._outcome_to_block(call, outcome)
        await recorder.emit(
            EventType.TOOL_CALL_RESULT,
            {
                "call_id": call.id,
                "name": call.name,
                "success": outcome.success,
                "exit_code": outcome.exit_code,
                "error": outcome.error,
                "output_preview": outcome.output[:_OUTPUT_PREVIEW_CHARS],
            },
        )
        return block

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

        try:
            return await tool.run(call.input, ToolContext(workspace=self._workspace))
        except Exception as exc:  # noqa: BLE001 - a tool crash must never escape the loop
            return ToolOutcome.failure(f"internal tool error: {exc}")

    @staticmethod
    def _approval_summary(name: str, arguments: dict[str, Any]) -> str:
        """Compact human-readable description of a pending tool call."""
        if name == "run_command" and isinstance(arguments.get("command"), str):
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
        build the :class:`RunResult` for this turn."""
        assert self.session_id is not None
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

    async def _finalize_cancelled(self, recorder: EventRecorder) -> None:
        """Persist ``CANCELLED`` while task cancellation is being handled.

        Guarded so finalization itself can never raise — the original
        :class:`asyncio.CancelledError` must always propagate to the caller.
        """
        if self.session_id is None:
            return
        try:
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
        )

    def _session_end_data(self, exit_reason: ExitReason) -> dict[str, Any]:
        return {
            "exit_reason": exit_reason.value,
            "rounds": self._rounds,
            "total_usage": {
                "input_tokens": self._usage.input_tokens,
                "output_tokens": self._usage.output_tokens,
                "total_tokens": self._usage.total_tokens,
            },
        }
