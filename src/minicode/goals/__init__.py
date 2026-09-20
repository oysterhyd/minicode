"""Goal acceptance (P1): spec loading, checking, and evidence binding.

Public API:
    AcceptanceItem, AcceptanceSpec   -- strict YAML acceptance config (spec.py)
    workspace_fingerprint            -- content fingerprint of a workspace
    ProtectedSnapshot                -- tamper detection for protected paths
    GoalItemResult, GoalReport       -- check outcome contracts
    GoalChecker                      -- runs acceptance items
    GoalEvidence, EvidenceLedger     -- fingerprint-bound evidence history
"""

from minicode.goals.checker import (
    GoalChecker,
    GoalItemResult,
    GoalReport,
    ProtectedSnapshot,
    workspace_fingerprint,
)
from minicode.goals.evidence import EvidenceLedger, GoalEvidence
from minicode.goals.spec import AcceptanceItem, AcceptanceSpec, ItemKind

__all__ = [
    "AcceptanceItem",
    "AcceptanceSpec",
    "EvidenceLedger",
    "GoalChecker",
    "GoalEvidence",
    "GoalItemResult",
    "GoalReport",
    "ItemKind",
    "ProtectedSnapshot",
    "workspace_fingerprint",
]
