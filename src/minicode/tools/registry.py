"""Tool registry: name-based lookup and provider-facing specs."""

from __future__ import annotations

from minicode.core.models import ToolSpec
from minicode.tools.base import BaseTool
from minicode.tools.command import RunCommandTool
from minicode.tools.files import ApplyPatchTool, ListFilesTool, ReadFileTool
from minicode.tools.search import SearchTextTool


class ToolRegistry:
    """Simple name-keyed container for tool instances."""

    def __init__(self) -> None:
        self._tools: dict[str, BaseTool] = {}

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


def default_registry() -> ToolRegistry:
    """Registry preloaded with the P0 built-in tools."""
    registry = ToolRegistry()
    for tool in (
        ReadFileTool(),
        ListFilesTool(),
        SearchTextTool(),
        ApplyPatchTool(),
        RunCommandTool(),
    ):
        registry.register(tool)
    return registry
