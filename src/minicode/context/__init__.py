"""Context management (P1): token estimation and layered compaction.

Public API:
- :func:`estimate_tokens` / :func:`estimate_messages_tokens` — conservative
  size estimates (characters/3 heuristic, never exact tokenizer counts).
- :class:`ContextCompactor` with :class:`CompactConfig` / :class:`CompactResult`
  — deterministic archive → shrink → structured-summary compaction that keeps
  tool_use/tool_result pairs intact.
"""

from __future__ import annotations

from minicode.context.compact import (
    CompactConfig,
    CompactResult,
    CompactStats,
    ContextCompactor,
)
from minicode.context.estimate import estimate_messages_tokens, estimate_tokens

__all__ = [
    "CompactConfig",
    "CompactResult",
    "CompactStats",
    "ContextCompactor",
    "estimate_messages_tokens",
    "estimate_tokens",
]
