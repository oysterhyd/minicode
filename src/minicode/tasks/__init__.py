"""Task subsystem for minicode (P1): background jobs, read-only subagents
and the task dependency store.

- :mod:`minicode.tasks.background` — :class:`BackgroundManager` tracks shell
  commands started with ``run_command(background=True)`` and hands finished
  jobs back exactly once.
- :mod:`minicode.tasks.subagent` — :class:`SubagentRunner` runs read-only
  child agents that answer in structured JSON.
- :mod:`minicode.tasks.taskstore` — :class:`TaskStore` persists a
  dependency-ordered task board in the shared session database.
"""

from __future__ import annotations

from minicode.tasks.background import (
    BackgroundJob,
    BackgroundManager,
    CompleteCallback,
    JobStatus,
    format_result,
)
from minicode.tasks.subagent import (
    SUBAGENT_SYSTEM_PROMPT,
    SubagentResult,
    SubagentRunner,
    validate_refs,
)
from minicode.tasks.taskstore import TaskRow, TaskStatus, TaskStore

__all__ = [
    "SUBAGENT_SYSTEM_PROMPT",
    "BackgroundJob",
    "BackgroundManager",
    "CompleteCallback",
    "JobStatus",
    "SubagentResult",
    "SubagentRunner",
    "TaskRow",
    "TaskStatus",
    "TaskStore",
    "format_result",
    "validate_refs",
]
