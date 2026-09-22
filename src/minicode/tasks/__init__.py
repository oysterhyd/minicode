"""Task subsystem for minicode (P1): background jobs and the task dependency store.

- :mod:`minicode.tasks.background` — :class:`BackgroundManager` tracks shell
  commands started with ``bash(background=True)`` and hands finished
  jobs back exactly once.
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
from minicode.tasks.taskstore import TaskRow, TaskStatus, TaskStore

__all__ = [
    "BackgroundJob",
    "BackgroundManager",
    "CompleteCallback",
    "JobStatus",
    "TaskRow",
    "TaskStatus",
    "TaskStore",
    "format_result",
]
