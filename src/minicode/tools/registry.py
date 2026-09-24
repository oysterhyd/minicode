"""Tool registry: name-based lookup and provider-facing specs."""

from __future__ import annotations


from minicode.core.models import ToolSpec
from minicode.tools.artifacts import ReadArtifactTool
from minicode.tools.base import BaseTool
from minicode.tools.command import BashTool
from minicode.tools.files import EditTool, LsTool, ReadTool, WriteTool
from minicode.tools.search import GrepTool
from minicode.plugins import McpServerConfig, PluginCatalog
from minicode.tools.mcp import McpConnector


class ToolRegistry:
    """Simple name-keyed container for tool instances."""

    def __init__(self) -> None:
        self._tools: dict[str, BaseTool] = {}
        self._mcp: list[McpConnector] = []
        self.plugin_catalog: PluginCatalog | None = None
        self.discovery_errors: list[str] = []

    def register(self, tool: BaseTool) -> None:
        """Add *tool*; duplicate names are a programming error."""
        if tool.name in self._tools:
            raise ValueError(f"tool already registered: {tool.name}")
        self._tools[tool.name] = tool

    def get(self, name: str) -> BaseTool | None:
        return self._tools.get(name)

    def specs(self) -> list[ToolSpec]:
        return [tool.spec() for tool in self._tools.values()]

    def names(self) -> list[str]:
        return list(self._tools)

    def add_mcp_server(self, config: McpServerConfig) -> None:
        self._mcp.append(McpConnector(config))

    async def prepare(self) -> list[dict[str, str | None]]:
        """Connect and discover before provider schemas are assembled."""
        self.discovery_errors = []
        statuses: list[dict[str, str | None]] = []
        for connector in self._mcp:
            try:
                tools = await connector.discover()
                for tool in tools:
                    existing = self.get(tool.name)
                    if existing is not None and (not hasattr(existing, "connector")
                                                 or existing.connector is not connector):
                        raise ValueError(f"tool name collision: {tool.name}")
                    self._tools[tool.name] = tool
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
