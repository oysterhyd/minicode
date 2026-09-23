"""Official-SDK stdio MCP bridge into minicode's normal tool pipeline."""

from __future__ import annotations

import asyncio
import json
import re
from contextlib import AsyncExitStack
from typing import Any

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError, ValidationError
from mcp import Client, StdioServerParameters

from minicode.core.models import ToolOutcome, ToolSpec
from minicode.plugins import McpServerConfig
from minicode.tools.base import BaseTool, ToolContext


_TOOL_NAME = re.compile(r"^[A-Za-z0-9_-]+$")
_MAX_DISCOVERY_PAGES = 100
_MAX_TOOLS = 256


class McpConnector:
    def __init__(self, config: McpServerConfig):
        self.config = config
        self._stack: AsyncExitStack | None = None
        self._client: Client | None = None
        self.protocol_version: str | None = None
        self.server_version: str | None = None

    async def _connect(self) -> Client:
        if self._client is not None:
            return self._client
        stack = AsyncExitStack()
        try:
            parameters = StdioServerParameters(
                command=self.config.command, args=list(self.config.args), cwd=self.config.cwd,
            )
            async with asyncio.timeout(self.config.timeout_s):
                client = await stack.enter_async_context(Client(
                    parameters, read_timeout_seconds=self.config.timeout_s,
                ))
        except BaseException:
            await stack.aclose()
            raise
        self._stack = stack
        self._client = client
        self.protocol_version = str(client.protocol_version)
        info = client.server_info
        self.server_version = str(info.version) if info is not None else None
        return client

    async def discover(self) -> list["McpTool"]:
        self.config.verify()
        client = await self._connect()
        cursor: str | None = None
        seen: set[str] = set()
        names: set[str] = set()
        result: list[McpTool] = []
        for _ in range(_MAX_DISCOVERY_PAGES):
            async with asyncio.timeout(self.config.timeout_s):
                page = await client.list_tools(cursor=cursor)
            for tool in page.tools:
                full_name = f"mcp__{self.config.name}__{tool.name}"
                if (not _TOOL_NAME.fullmatch(tool.name) or len(full_name) > 64
                        or tool.name in names):
                    raise ValueError(f"invalid or duplicate MCP tool name: {tool.name}")
                names.add(tool.name)
                if len(names) > _MAX_TOOLS:
                    raise ValueError(f"MCP server {self.config.name} exposed too many tools")
                schema = tool.input_schema
                if not isinstance(schema, dict):
                    raise ValueError(f"invalid schema for MCP tool {tool.name}")
                Draft202012Validator.check_schema(schema)
                result.append(McpTool(self, tool.name, tool.description or "", schema))
            cursor = page.next_cursor
            if cursor is None:
                return result
            if cursor in seen:
                raise ValueError(f"MCP server {self.config.name} repeated a discovery cursor")
            seen.add(cursor)
        raise ValueError(f"MCP server {self.config.name} exceeded discovery page limit")

    async def call(self, name: str, arguments: dict[str, Any]) -> ToolOutcome:
        try:
            self.config.verify()
            client = await self._connect()
            async with asyncio.timeout(self.config.timeout_s):
                result = await client.call_tool(name, arguments)
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # connection failures are reported to the model
            await self.aclose()
            return ToolOutcome.failure(f"MCP server {self.config.name} disconnected or timed out: {exc}")
        parts: list[str] = []
        for block in result.content:
            if block.type == "text":
                parts.append(block.text)
            elif block.type == "resource_link":
                parts.append(f"[resource: {block.uri}]")
            else:
                parts.append(f"[{block.type} content omitted from text output]")
        if not parts and result.structured_content is not None:
            parts.append(json.dumps(result.structured_content, ensure_ascii=False))
        output = "\n".join(parts)
        return (ToolOutcome.failure("MCP tool error", output=output) if result.is_error
                else ToolOutcome(output=output))

    async def aclose(self) -> None:
        stack, self._stack = self._stack, None
        self._client = None
        if stack is not None:
            await stack.aclose()


class McpTool(BaseTool):
    """Remote metadata never decides approval, replay safety, or concurrency."""

    def __init__(self, connector: McpConnector, remote_name: str,
                 description: str, schema: dict[str, Any]):
        self.connector = connector
        self.remote_name = remote_name
        self.name = f"mcp__{connector.config.name}__{remote_name}"
        self.description = f"[{connector.config.plugin}/{connector.config.name}] {description}"
        self.schema = schema
        self.source = {
            "kind": "mcp", "plugin": connector.config.plugin,
            "plugin_version": connector.config.plugin_version,
            "plugin_sha256": connector.config.plugin_digest,
            "server": connector.config.name,
        }

    def spec(self) -> ToolSpec:
        return ToolSpec(name=self.name, description=self.description,
                        input_schema=self.schema, requires_approval=True)

    async def run(self, raw_args: dict[str, Any], ctx: ToolContext) -> ToolOutcome:
        try:
            Draft202012Validator(self.schema).validate(raw_args)
        except (SchemaError, ValidationError) as exc:
            return ToolOutcome.failure(f"invalid arguments for {self.name}: {exc.message}")
        return await self.connector.call(self.remote_name, raw_args)

    async def execute(self, args: Any, ctx: ToolContext) -> ToolOutcome:
        raise NotImplementedError("McpTool validates the discovered JSON Schema in run")
