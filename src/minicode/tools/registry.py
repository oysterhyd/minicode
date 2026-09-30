"""Tool registry: name-based lookup and provider-facing specs."""

from __future__ import annotations


from minicode.core.models import ToolSpec
from minicode.tools.artifacts import ReadArtifactTool
from minicode.tools.base import BaseTool
from minicode.tools.base import ToolExecution
from minicode.core.models import ToolUseBlock
from minicode.tools.command import BashTool
from minicode.tools.files import EditTool, LsTool, ReadTool, WriteTool
from minicode.tools.search import GrepTool
from minicode.plugins import McpServerConfig, PluginCatalog
from minicode.tools.mcp import McpConnector


class ToolRegistry:
    """Simple name-keyed container for tool instances."""

    def __init__(self) -> None:
        self._tools: dict[str, BaseTool] = {}
        self._specs_cache: list[ToolSpec] | None = None
        self._mcp: list[McpConnector] = []
        self._mcp_tool_names: set[str] = set()
        self.plugin_catalog: PluginCatalog | None = None
        self.agent_definitions: dict[str, dict] = {}
        self.discovery_errors: list[str] = []
        self._prepared = False
        self._prepare_lock = None
        self._statuses: list[dict] = []

    def register(self, tool: BaseTool) -> None:
        """Add *tool*; duplicate names are a programming error."""
        if tool.name in self._tools:
            raise ValueError(f"tool already registered: {tool.name}")
        execution = tool.execution
        if execution.attempts < 1 or (execution.attempts > 1 and not execution.replay_safe):
            raise ValueError("tool retries require an explicit replay-safe capability")
        if execution.parallel_group not in {None, "read", "delegate"}:
            raise ValueError("unknown tool concurrency group")
        if execution.parallel_group == "read" and not execution.replay_safe:
            raise ValueError("parallel reads require a replay-safe capability")
        self._tools[tool.name] = tool
        self._specs_cache = None

    def get(self, name: str) -> BaseTool | None:
        return self._tools.get(name)

    def execution_for(self, call: ToolUseBlock) -> ToolExecution:
        tool = self.get(call.name)
        if tool is None:
            return ToolExecution()
        if call.name == "delegate":
            definition = self.agent_definitions.get(str(call.input.get("kind")))
            if definition is not None:
                names = self.names() if definition.get("inheritTools") else definition.get("tools", [])
                # Two writable delegates may share a workspace. They are
                # ordering barriers until the host provides isolated worktrees.
                if any(self.get(name) is None or not self.get(name).execution.replay_safe
                       for name in names if name != "delegate" and not name.startswith("task_")):
                    return ToolExecution(timeout_s=None)
        return tool.execution

    def set_agents(self, definitions: list[dict], plugin_names: list[str]) -> None:
        from minicode.tools.extensions import DelegateTool
        import copy
        self.agent_definitions = {a["name"]: copy.deepcopy(a) for a in definitions}
        self._tools["delegate"] = DelegateTool(
            [a["name"] for a in definitions if a["enabled"]] + plugin_names,
            include_builtins=False,
            descriptions={a["name"]: a["description"] for a in definitions if a["enabled"]},
        )
        self._specs_cache = None

    def specs(self) -> list[ToolSpec]:
        if self._specs_cache is None:
            self._specs_cache = [tool.spec() for tool in self._tools.values()]
        return [spec.model_copy(deep=True) for spec in self._specs_cache]

    def names(self) -> list[str]:
        return list(self._tools)

    def add_mcp_server(self, config: McpServerConfig) -> None:
        if self._prepared:
            raise RuntimeError("cannot add an MCP server to a prepared registry")
        if any(c.config.name == config.name for c in self._mcp):
            raise ValueError(f"MCP server already registered: {config.name}")
        self._mcp.append(McpConnector(config))

    async def prepare(self) -> list[dict[str, str | None]]:
        import asyncio
        if self._prepared:
            return [dict(status) for status in self._statuses]
        if self._prepare_lock is None:
            self._prepare_lock = asyncio.Lock()
        async with self._prepare_lock:
            if not self._prepared:
                self._statuses = await self._prepare()
                self._prepared = True
            return [dict(status) for status in self._statuses]

    async def _prepare(self) -> list[dict[str, str | None]]:
        """Connect and discover before provider schemas are assembled."""
        self.discovery_errors = []
        if self._mcp_tool_names:
            for name in self._mcp_tool_names:
                self._tools.pop(name, None)
            self._mcp_tool_names.clear()
            self._specs_cache = None
        statuses: list[dict[str, str | None]] = []
        for connector in self._mcp:
            try:
                tools = await connector.discover()
                discovered_names: set[str] = set()
                for tool in tools:
                    if tool.name in self._tools or tool.name in discovered_names:
                        raise ValueError(f"tool name collision: {tool.name}")
                    discovered_names.add(tool.name)
                for tool in tools:
                    self._tools[tool.name] = tool
                if tools:
                    self._mcp_tool_names.update(discovered_names)
                    self._specs_cache = None
                statuses.append({"server": connector.config.name, "plugin": connector.config.plugin,
                                 "protocol": connector.protocol_version,
                                 "server_version": connector.server_version, "error": None})
            except Exception as exc:  # a failed extension must not hide built-ins
                error = f"MCP {connector.config.name}: {exc}"
                try:
                    await connector.aclose()
                except Exception as cleanup_exc:
                    error += f" (cleanup: {cleanup_exc})"
                self.discovery_errors.append(error)
                statuses.append({"server": connector.config.name, "plugin": connector.config.plugin,
                                 "protocol": None, "server_version": None, "error": error})
        return statuses

    async def aclose(self) -> None:
        # The SDK's async exit stack must close in the task that entered it.
        for connector in self._mcp:
            try:
                await connector.aclose()
            except Exception as exc:
                self.discovery_errors.append(f"MCP close: {exc}")
        for name in self._mcp_tool_names:
            self._tools.pop(name, None)
        self._mcp_tool_names.clear()
        self._prepared = False
        self._prepare_lock = None
        self._specs_cache = None


def default_registry(*, skills=None, delegation: bool = False, tasks: bool = False,
                     memory: bool = False,
                     agent_kinds: list[str] | None = None) -> ToolRegistry:
    """Registry preloaded with the built-in tools.

    The first four mirror the Pi agent's core tool set (read / bash /
    edit / write); ``ls`` and ``grep`` are the read-only extras.
    """
    registry = ToolRegistry()
    for tool in (
        ReadTool(),
        BashTool(),
        EditTool(),
        WriteTool(),
        LsTool(),
        GrepTool(),
        ReadArtifactTool(),
    ):
        registry.register(tool)
    if skills is not None:
        from minicode.tools.extensions import SkillLoadTool, SkillUnloadTool, SkillResourceTool, SkillsListTool

        for tool in (SkillsListTool(skills), SkillLoadTool(), SkillUnloadTool(), SkillResourceTool(skills)):
            registry.register(tool)
    if delegation:
        from minicode.tools.extensions import DelegateTool

        registry.register(DelegateTool(agent_kinds))
    if tasks:
        from minicode.tools.tasks import TaskClaimTool, TaskCompleteTool, TaskCreateTool, TaskListTool

        for tool in (TaskCreateTool(), TaskListTool(), TaskClaimTool(), TaskCompleteTool()):
            registry.register(tool)
    if memory:
        from minicode.tools.memory import MemoryListTool

        registry.register(MemoryListTool())
    return registry
