"""Bounded, ordered tool scheduling with explicit host-owned capabilities."""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable, Callable

from minicode.core.models import ToolResultBlock, ToolUseBlock
from minicode.tools.registry import ToolRegistry


class ToolScheduler:
    def __init__(self, registry: ToolRegistry, *, parallel_reads: int = 4,
                 parallel_delegates: int = 2):
        self.registry = registry
        self.parallel_reads = parallel_reads
        self.parallel_delegates = parallel_delegates

    def batch(self, calls: list[ToolUseBlock], index: int) -> list[ToolUseBlock]:
        first = calls[index]
        group = self.registry.execution_for(first).parallel_group
        limit = (self.parallel_reads if group == "read" else
                 self.parallel_delegates if group == "delegate" else 1)
        batch: list[ToolUseBlock] = []
        keys: set[tuple[str, str]] = set()
        for call in calls[index:index + limit]:
            if self.registry.execution_for(call).parallel_group != group:
                break
            if group == "delegate":
                key = (str(call.input.get("kind")), str(call.input.get("task")))
                if key in keys:
                    break
                keys.add(key)
            batch.append(call)
        return batch or [first]

    async def execute(self, calls: list[ToolUseBlock],
                      invoke: Callable[[ToolUseBlock], Awaitable[ToolResultBlock]],
                      results: list[ToolResultBlock]) -> None:
        """Append settled results in input order, even on batch cancellation."""
        index = 0
        while index < len(calls):
            batch = self.batch(calls, index)
            if len(batch) == 1:
                results.append(await invoke(batch[0]))
            else:
                tasks = [asyncio.create_task(invoke(call), name=f"tool:{call.id}") for call in batch]
                try:
                    results.extend(await asyncio.gather(*tasks))
                except BaseException:
                    for task in tasks:
                        task.cancel()
                    await asyncio.gather(*tasks, return_exceptions=True)
                    results.extend(task.result() for task in tasks if not task.cancelled()
                                   and task.exception() is None)
                    raise
            index += len(batch)
