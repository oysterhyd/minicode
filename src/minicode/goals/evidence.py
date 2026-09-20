"""Evidence ledger: bind passing goal reports to workspace fingerprints.

Acceptance evidence is only trustworthy for the exact code state that produced
it. Every recorded report carries the workspace content fingerprint captured at
``GoalChecker.run()`` time; :meth:`EvidenceLedger.valid_pass` returns True only
when a fully-passing report exists for exactly the fingerprint the caller asks
about. Once the code changes the fingerprint changes, the old evidence is stale
and a fresh acceptance run is required.
"""

from __future__ import annotations

from pydantic import BaseModel

from minicode.goals.checker import GoalReport
from minicode.goals.spec import ItemKind


class GoalEvidence(BaseModel):
    """Per-item evidence distilled from a passing goal report."""

    item_id: str
    kind: ItemKind
    exit_code: int | None = None
    fingerprint: str
    ran_at: str
    detail: str


class EvidenceLedger:
    """Accumulates evidence across acceptance runs, keeping full history."""

    def __init__(self) -> None:
        # One entry per recorded report: (fingerprint, fully_passed), in order.
        self._reports: list[tuple[str, bool]] = []
        # One evidence batch per recorded report (empty for non-passing ones).
        self._batches: list[list[GoalEvidence]] = []

    def record(self, report: GoalReport) -> list[GoalEvidence]:
        """Record *report* and return the evidence created from it.

        Only fully-passing reports produce evidence (one entry per item);
        failing or partially-failing reports are kept in history but yield an
        empty list, so evidence can never back a run that did not fully pass.
        """
        batch: list[GoalEvidence] = []
        if report.passed:
            batch = [
                GoalEvidence(
                    item_id=result.item_id,
                    kind=result.kind,
                    exit_code=result.exit_code,
                    fingerprint=report.fingerprint,
                    ran_at=report.ran_at,
                    detail=result.detail,
                )
                for result in report.items
                if result.passed
            ]
        self._reports.append((report.fingerprint, report.passed))
        self._batches.append(batch)
        return list(batch)

    def valid_pass(self, fingerprint: str) -> bool:
        """True iff a fully-passing report was recorded for this fingerprint."""
        return any(fp == fingerprint and passed for fp, passed in self._reports)

    def latest(self) -> list[GoalEvidence] | None:
        """Evidence batch of the most recent record; None if nothing recorded."""
        if not self._batches:
            return None
        return list(self._batches[-1])
