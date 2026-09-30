"""Validated tool invocation through policy, serialized approval and limits."""

from __future__ import annotations

import asyncio
import copy
from collections.abc import Awaitable, Callable

from minicode.core.models import ApprovalRequest, EventType, ToolOutcome, ToolUseBlock
from minicode.runtime.events import EventRecorder
from minicode.security.policy import PolicyBehavior
from minicode.tools.base import ToolContext


async def execute_tool(
    call: ToolUseBlock, *, registry, policy, approval_handler,
    approval_lock: asyncio.Lock, recorder: EventRecorder,
    preflight: Callable[[ToolUseBlock, dict], Awaitable[ToolOutcome | None]],
    context: Callable[[], ToolContext], check_deadline: Callable[[], None],
    approval_summary: Callable[[str, dict], str],
    waiting: Callable[[bool], None],
) -> ToolOutcome:
    tool = registry.get(call.name)
    if tool is None:
        return ToolOutcome.failure(f"unknown tool: {call.name}")
    try:
        arguments = tool.validate_args(call.input)
    except Exception as error:
        return ToolOutcome.failure(f"invalid arguments for {call.name}: {error}")
    blocked = await preflight(call, arguments)
    if blocked is not None:
        return blocked
    decision = await policy.check(call.name, arguments)
    if decision.behavior is PolicyBehavior.DENY:
        return ToolOutcome.failure("permission denied by policy" +
                                   (f": {decision.reason}" if decision.reason else ""))
    if decision.behavior is PolicyBehavior.ASK:
        if approval_handler is None:
            return ToolOutcome.failure("approval required but no approval handler is configured")
        async with approval_lock:
            summary = approval_summary(call.name, arguments)
            waiting(True)
            try:
                await recorder.emit(EventType.APPROVAL_REQUEST, {
                    "call_id": call.id, "tool_name": call.name, "summary": summary,
                })
                approved = await approval_handler(ApprovalRequest(
                    tool_name=call.name, arguments=copy.deepcopy(arguments), summary=summary,
                ))
                await recorder.emit(EventType.APPROVAL_DECISION, {
                    "call_id": call.id, "tool_name": call.name,
                    "granted": approved.granted, "reason": approved.reason,
                })
            finally:
                waiting(False)
            if not approved.granted:
                return ToolOutcome.failure("approval denied" +
                                           (f": {approved.reason}" if approved.reason else ""))
    for attempt in range(tool.execution.attempts):
        check_deadline()
        try:
            async with asyncio.timeout(tool.execution.timeout_s):
                outcome = await tool.run(arguments, context())
            if not isinstance(outcome, ToolOutcome):
                return ToolOutcome.failure("tool returned an invalid outcome")
            return outcome
        except TimeoutError:
            return ToolOutcome.failure(
                "tool timed out; its side effects may be incomplete, inspect state before retrying"
            )
        except Exception as error:
            if attempt + 1 == tool.execution.attempts:
                return ToolOutcome.failure(f"internal tool error: {error}")
            await asyncio.sleep(0.1)
    return ToolOutcome.failure("tool execution policy has no attempts")
