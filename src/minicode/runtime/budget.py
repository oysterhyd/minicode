"""Budget guard for the agent loop."""

from __future__ import annotations

import time

from minicode.core.models import Budget, Usage


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
        self.deadline = time.monotonic() + budget.max_seconds

    def time_exceeded(self) -> bool:
        """True once this turn's wall-clock deadline has passed."""
        return time.monotonic() >= self.deadline

    def rounds_exceeded(self, rounds: int) -> bool:
        """True after this activation has consumed its round slice."""
        return self.budget.max_rounds > 0 and rounds - self._base_rounds >= self.budget.max_rounds

    def tokens_exceeded(self, usage: Usage) -> bool:
        """True when *usage* (session-cumulative) exceeds the token cap.

        A cap of ``0`` or less disables token accounting entirely; round and
        wall-clock slices still bound one activation.
        """
        if self.budget.max_total_tokens <= 0:
            return False
        return usage.total_tokens > self.budget.max_total_tokens
