"""Restore a durable session without replaying unknown side effects."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from minicode.core.models import (
    ApprovalHandler, Budget, EventType, ExitReason, TextBlock, ToolUseBlock, Usage,
)
from minicode.providers.base import Provider
from minicode.runtime.events import EventCallback
from minicode.runtime.budget import SharedBudgetLedger
from minicode.context.extensions import ProjectInstructions, SkillCatalog
from minicode.security.policy import PermissionPolicy
from minicode.storage import SessionStore
from minicode.tools.registry import ToolRegistry

TextDeltaCallback = Any


def restore_runtime(
    cls,
    *,
    store: SessionStore,
    session_id: str,
    provider: Provider,
    registry: ToolRegistry,
    policy: PermissionPolicy,
    workspace: Path | None = None,
    provider_name: str | None = None,
    model: str | None = None,
    budget: Budget | None = None,
    approval_handler: ApprovalHandler | None = None,
    on_text_delta: TextDeltaCallback | None = None,
    on_event: EventCallback | None = None,
    system_prompt: str | None = None,
    compactor: Any | None = None,
    goal_checker: Any | None = None,
    evidence_ledger: Any | None = None,
    background_manager: Any | None = None,
    artifact_store: Any | None = None,
    project_instructions: ProjectInstructions | None = None,
    skills: SkillCatalog | None = None,
    allow_delegation: bool = True,
    shared_budget: SharedBudgetLedger | None = None,
) -> "AgentRuntime":
    """Continue a persisted session in a fresh process.

    Loads the stored conversation, usage and round count, restores the
    workspace/model from the session row (overridable) and marks tool
    calls whose outcome was lost to an interruption; they are settled at
    the start of the next :meth:`run_turn` (see :meth:`_settle_recovery`).
    Raises ``ValueError`` for an unknown session or a missing workspace.
    """
    summary = store.get_session(session_id)
    if summary is None:
        raise ValueError(f"unknown session: {session_id}")
    events = store.get_events(session_id)
    start_event = next((event for event in events
                        if event.type is EventType.SESSION_START), None)
    budget_event = next((event for event in reversed(events)
                         if event.type is EventType.BUDGET_CHANGED), start_event)
    saved_budget = budget_event.data.get("budget") if budget_event else None
    effective_budget = budget if budget is not None else Budget()
    if isinstance(saved_budget, dict):
        try:
            saved_cap = int(saved_budget.get("max_total_tokens", 0))
        except (TypeError, ValueError):
            saved_cap = 0
        # A default zero in a resumed frontend must not silently remove
        # an explicit spending cap. A negative cap is an explicit opt-out.
        if saved_cap != 0 and effective_budget.max_total_tokens == 0:
            effective_budget = effective_budget.model_copy(
                update={"max_total_tokens": saved_cap}
            )
    resolved_workspace = (Path(workspace) if workspace else Path(summary.workspace)).resolve()
    if not resolved_workspace.is_dir():
        raise ValueError(
            f"workspace no longer exists: {resolved_workspace} (session {session_id})"
        )
    if project_instructions is None and (
        (start_event and start_event.data.get("project_instructions"))
        or any(e.type is EventType.PROJECT_INSTRUCTIONS for e in events)
    ):
        project_instructions = ProjectInstructions(resolved_workspace)
    if skills is None and (
        (start_event and start_event.data.get("active_skills"))
        or any(e.type is EventType.SKILL_ACTIVATED for e in events)
    ):
        skills = SkillCatalog(resolved_workspace)

    resolved_provider = provider_name or provider.name
    resolved_model = model or getattr(provider, "configuration_id", getattr(provider, "model", summary.model))
    cls._validate_provider(provider, resolved_provider, resolved_model)
    runtime = cls(
        provider=provider,
        registry=registry,
        store=store,
        policy=policy,
        workspace=resolved_workspace,
        provider_name=resolved_provider,
        model=resolved_model,
        budget=effective_budget,
        approval_handler=approval_handler,
        on_text_delta=on_text_delta,
        on_event=on_event,
        system_prompt=system_prompt,
        compactor=compactor,
        goal_checker=goal_checker,
        evidence_ledger=evidence_ledger,
        background_manager=background_manager,
        artifact_store=artifact_store,
        project_instructions=project_instructions,
        skills=skills,
        allow_delegation=allow_delegation,
        shared_budget=shared_budget,
    )
    if project_instructions is not None:
        expected_sources = list(start_event.data.get("project_instructions", [])) if start_event else []
        expected_sources.extend(event.data for event in events
                                if event.type is EventType.PROJECT_INSTRUCTIONS)
        for saved in expected_sources:
            path = Path(saved["path"])
            if path != resolved_workspace / "AGENTS.md":
                relative = path.relative_to(resolved_workspace)
                project_instructions.discover(str(relative.parent / "_scope_probe"))
            source = project_instructions.loaded.get(path)
            if source is None or source.digest != saved["sha256"]:
                raise ValueError(f"project instructions changed since session start: {path}")
        runtime._refresh_system_prompt()
    active_skills = dict(start_event.data.get("active_skills", {})) if start_event else {}
    for event in events:
        if event.type is EventType.SKILL_ACTIVATED:
            active_skills[str(event.data["name"])] = str(event.data["sha256"])
        elif event.type is EventType.SKILL_DEACTIVATED:
            active_skills.pop(str(event.data["name"]), None)
    for name, digest in active_skills.items():
        if skills is None or skills.load(name).digest != digest:
            raise ValueError(f"activated skill changed since session start: {name}")
        runtime.activate_skill(name)
    runtime.session_id = session_id
    runtime._messages = store.get_messages(session_id)
    runtime._usage = Usage(
        input_tokens=summary.input_tokens,
        output_tokens=summary.output_tokens,
        cache_read_tokens=summary.cache_read_tokens,
        cache_write_tokens=summary.cache_write_tokens,
        available=summary.usage_available,
    )
    runtime._rounds = summary.rounds
    runtime._task_pending = bool(runtime._messages) and summary.status != ExitReason.COMPLETED.value
    try:
        runtime._last_exit_reason = ExitReason(summary.exit_reason) if summary.exit_reason else None
    except ValueError:
        runtime._last_exit_reason = None
    runtime._pending_dangling = cls._find_dangling_calls(runtime._messages)
    jobs: dict[str, dict[str, Any]] = {}
    event_usage = Usage()
    event_rounds = summary.rounds
    completed_responses = 0
    for event in events:
        if event.type is EventType.BACKGROUND_JOB_STARTED:
            jobs[event.data["job_id"]] = event.data
        elif event.type in (EventType.BACKGROUND_JOB_COMPLETED, EventType.BACKGROUND_JOB_LOST):
            jobs.pop(event.data["job_id"], None)
        elif event.type is EventType.ROUND_START:
            event_rounds = max(event_rounds, int(event.data.get("round", 0)))
        elif event.type is EventType.ASSISTANT_MESSAGE:
            completed_responses += 1
            data = event.data.get("usage")
            if isinstance(data, dict):
                response_usage = Usage(
                    input_tokens=int(data.get("input_tokens", 0)),
                    output_tokens=int(data.get("output_tokens", 0)),
                    cache_read_tokens=int(data.get("cache_read_tokens", 0)),
                    cache_write_tokens=int(data.get("cache_write_tokens", 0)),
                    available=bool(data.get("available", False)),
                )
                event_usage += response_usage
                runtime._last_model_usage = response_usage
    child_ids = {
        str(event.data["child_session_id"])
        for event in events if event.type is EventType.SUBAGENT_START
        and event.data.get("child_session_id")
    }
    settled_children = {
        str(event.data["child_session_id"])
        for event in events if event.type is EventType.SUBAGENT_RESULT
        and event.data.get("child_session_id")
    }
    for child_id in child_ids:
        child_events = store.get_events(child_id)
        child_responses = [event for event in child_events
                           if event.type is EventType.ASSISTANT_MESSAGE]
        completed_responses += len(child_responses)
        child_usage = Usage()
        for child_event in child_responses:
            data = child_event.data.get("usage")
            if isinstance(data, dict):
                child_usage += Usage(**data)
        child_summary = store.get_session(child_id)
        if child_summary is not None:
            # Do not sum cumulative SUBAGENT_RESULT records across slices.
            # Count every child exactly once, retaining uncertain billing.
            child_usage = Usage(
                input_tokens=max(child_usage.input_tokens, child_summary.input_tokens),
                output_tokens=max(child_usage.output_tokens, child_summary.output_tokens),
                cache_read_tokens=max(child_usage.cache_read_tokens, child_summary.cache_read_tokens),
                cache_write_tokens=max(child_usage.cache_write_tokens, child_summary.cache_write_tokens),
                available=child_usage.available and child_summary.usage_available,
            )
        event_usage += child_usage
    runtime._rounds = event_rounds
    restore_progress = getattr(provider, "restore_progress", None)
    resume_identity = getattr(provider, "resume_identity", None)
    saved_identity = (start_event.data.get("provider_resume_identity")
                      if start_event else None)
    if (callable(restore_progress) and callable(resume_identity)
            and saved_identity is not None
            and resume_identity() == saved_identity):
        ids = [
            block.id
            for message in runtime._messages for block in message.content
            if isinstance(block, ToolUseBlock)
        ]
        ids.extend(
            str(event.data.get("call_id", ""))
            for event in events if event.type is EventType.TOOL_CALL_START
        )
        for child_id in child_ids:
            ids.extend(
                str(event.data.get("call_id", ""))
                for event in store.get_events(child_id)
                if event.type is EventType.TOOL_CALL_START
            )
        fake_call_counter = max((
            int(call_id.removeprefix("fake_tool_"))
            for call_id in ids
            if call_id.startswith("fake_tool_")
            and call_id.removeprefix("fake_tool_").isdigit()
        ), default=0)
        restore_progress(completed_responses, fake_call_counter)
    if event_usage.total_tokens > runtime._usage.total_tokens:
        runtime._usage = event_usage
    runtime._lost_jobs = list(jobs.values())
    runtime._bind_goal_session(resuming=True)
    store.update_session(
        session_id, workspace=str(resolved_workspace),
        provider=resolved_provider, model=resolved_model,
        rounds=runtime._rounds,
        input_tokens=runtime._usage.input_tokens,
        output_tokens=runtime._usage.output_tokens,
        cache_read_tokens=runtime._usage.cache_read_tokens,
        cache_write_tokens=runtime._usage.cache_write_tokens,
        usage_available=runtime._usage.available,
    )
    # Explicit recovery overrides become the latest durable configuration too.
    if effective_budget.model_dump() != saved_budget:
        runtime.set_budget(effective_budget)
    return runtime
