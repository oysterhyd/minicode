"""Budget guard for the agent loop."""

from __future__ import annotations

import time
from dataclasses import dataclass

from minicode.core.models import Budget, Usage


@dataclass
class SharedBudgetLedger:
    """One activation's counters, shared by parent and concurrent children."""

    usage: Usage
    rounds: int = 0
    deadline: float | None = None

    def add_usage(self, usage: Usage) -> None:
        # Model completions run on one event loop; this update has no await.
        self.usage = self.usage + usage

    def reserve_round(self, limit: int) -> bool:
        if limit > 0 and self.rounds >= limit:
            return False
        self.rounds += 1
        return True


class BudgetChecker:
    """Per-activation budget guard over session-cumulative counters.

    Wall-clock and round limits bound one activation of a task. They create
    a durable checkpoint rather than a permanent session limit. The optional
    token cap is cumulative because it is an explicit spending guard.
    """

    def __init__(self, budget: Budget, *, start_usage: Usage, start_rounds: int) -> None:
        self.budget = budget
        self._base_usage = start_usage
        self._base_rounds = start_rounds
        self.deadline = (time.monotonic() + budget.max_seconds
                         if budget.max_seconds > 0 else None)

    def time_exceeded(self) -> bool:
        """True once this turn's wall-clock deadline has passed."""
        return self.deadline is not None and time.monotonic() >= self.deadline

    def rounds_exceeded(self, rounds: int) -> bool:
        """True after this activation has consumed its round slice."""
        return self.budget.max_rounds > 0 and rounds - self._base_rounds >= self.budget.max_rounds

    def tokens_exceeded(self, usage: Usage) -> bool:
        """True when *usage* (session-cumulative) exceeds the token cap.

        A cap of ``0`` or less disables token accounting entirely. Round and
        wall-clock slices apply only when explicitly configured.
        """
        if self.budget.max_total_tokens <= 0:
            return False
        return usage.total_tokens > self.budget.max_total_tokens
