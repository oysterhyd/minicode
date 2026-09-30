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
        self._owner: asyncio.Task | None = None
        self._queue: asyncio.Queue | None = None
        self._closing = False

    async def _request(self, operation: str, *arguments):
        """All SDK contexts enter and exit in one long-lived owning task."""
        if self._closing:
            raise RuntimeError("MCP connection is closing")
        if self._owner is None or self._owner.done():
            self._queue = asyncio.Queue(maxsize=32)
            self._owner = asyncio.create_task(self._serve(), name=f"mcp:{self.config.name}")
        future = asyncio.get_running_loop().create_future()
        assert self._queue is not None
        self._queue.put_nowait((operation, arguments, future))
        try:
            return await asyncio.shield(future)
        except asyncio.CancelledError:
            future.cancel()
            # Cancellation terminates the affected connection, never replays
            # an external call, and waits for the server process to be reaped.
            await self.aclose()
            raise

    async def _serve(self) -> None:
        current = None
        assert self._queue is not None
        queue = self._queue
        try:
            while True:
                operation, arguments, current = await queue.get()
                if current.cancelled():
                    continue
                try:
                    result = (await self._discover() if operation == "discover"
                              else await self._call(*arguments))
                except Exception as exc:
                    if not current.done():
                        current.set_exception(exc)
                else:
                    if not current.done():
                        current.set_result(result)
                current = None
        except asyncio.CancelledError:
            pass
        finally:
            try:
                await self._disconnect()
            finally:
                pending = [current] if current is not None else []
                while not queue.empty():
                    pending.append(queue.get_nowait()[2])
                for future in pending:
                    if not future.done():
                        future.set_exception(RuntimeError("MCP connection closed during request"))

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
        return await self._request("discover")

    async def _discover(self) -> list["McpTool"]:
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
            return await self._request("call", name, arguments)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            return ToolOutcome.failure(f"MCP server {self.config.name} disconnected or timed out: {exc}")

    async def _call(self, name: str, arguments: dict[str, Any]) -> ToolOutcome:
        try:
            self.config.verify()
            client = await self._connect()
            async with asyncio.timeout(self.config.timeout_s):
                result = await client.call_tool(name, arguments)
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # connection failures are reported to the model
            await self._disconnect()
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
        owner = self._owner
        if owner is None:
            return
        if not self._closing and not owner.done():
            self._closing = True
            owner.cancel()
        try:
            # A second Stop cannot strand the task that owns SDK cancel scopes.
            while not owner.done():
                try:
                    await asyncio.shield(owner)
                except asyncio.CancelledError:
                    continue
            owner.result()
        finally:
            if self._owner is owner:
                self._owner = None
                self._closing = False

    async def _disconnect(self) -> None:
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

    def validate_args(self, raw_args: dict[str, Any]) -> dict[str, Any]:
        Draft202012Validator(self.schema).validate(raw_args)
        return dict(raw_args)

    async def run(self, raw_args: dict[str, Any], ctx: ToolContext) -> ToolOutcome:
        try:
            Draft202012Validator(self.schema).validate(raw_args)
        except (SchemaError, ValidationError) as exc:
            return ToolOutcome.failure(f"invalid arguments for {self.name}: {exc.message}")
        return await self.connector.call(self.remote_name, raw_args)

    async def execute(self, args: Any, ctx: ToolContext) -> ToolOutcome:
        raise NotImplementedError("McpTool validates the discovered JSON Schema in run")
