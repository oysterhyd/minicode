"""Stable host-facing snapshots of a live agent activation."""

from __future__ import annotations

from enum import Enum

from pydantic import BaseModel

from minicode.core.models import Budget, Usage


class RunPhase(str, Enum):
    IDLE = "idle"
    PREPARING = "preparing"
    MODEL = "model"
    TOOLS = "tools"
    APPROVAL = "approval"
    FINISHING = "finishing"
    PAUSED = "paused"


class RuntimeSnapshot(BaseModel):
    session_id: str | None
    run_id: str | None
    phase: RunPhase
    running: bool
    task_pending: bool
    provider: str
    model: str
    rounds: int
    usage: Usage
    budget: Budget
    context_window: int
    context_breakdown: dict[str, int]
    active_skills: dict[str, str]
