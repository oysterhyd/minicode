"""Workspace services shared by CLI, TUI, workflows and the desktop host."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from minicode.context.compact import CompactConfig, ContextCompactor
from minicode.context.extensions import ProjectInstructions, SkillCatalog
from minicode.plugins import PluginCatalog
from minicode.tools.registry import ToolRegistry, default_registry


@dataclass(slots=True)
class WorkspaceServices:
    registry: ToolRegistry
    skills: SkillCatalog
    instructions: ProjectInstructions
    plugins: PluginCatalog


def workspace_services(workspace: Path, *, agents: list[dict] | None = None) -> WorkspaceServices:
    if agents is None:
        from minicode.configuration import HarnessConfiguration
        agents = HarnessConfiguration().agents()
    workspace = workspace.resolve()
    if not workspace.is_dir():
        raise ValueError(f"workspace does not exist: {workspace}")
    plugins = PluginCatalog(workspace)
    skills = SkillCatalog(workspace, plugin_roots=plugins.skill_roots())
    registry = default_registry(skills=skills, delegation=True, tasks=True, memory=True,
                                agent_kinds=plugins.agent_names())
    registry.plugin_catalog = plugins
    if agents is not None:
        registry.set_agents(agents, plugins.agent_names())
    for server in plugins.servers():
        registry.add_mcp_server(server)
    return WorkspaceServices(registry, skills, ProjectInstructions(workspace), plugins)


def attach_compactor(runtime, artifact_store) -> None:
    def spill(kind: str, content: str) -> str:
        if runtime.session_id is None:
            raise RuntimeError("compaction requires a persisted session")
        return artifact_store.spill(runtime.session_id, kind, content).artifact_id

    runtime.set_compactor(ContextCompactor(
        CompactConfig(max_context_tokens=max(runtime.context_window, 1)),
        spill_fn=spill,
        context_tokens_fn=lambda: runtime.context_window,
        output_tokens_fn=runtime.effective_max_output_tokens,
        estimate_scale_fn=lambda: getattr(runtime.provider, "prompt_scale", 1.0),
    ))
