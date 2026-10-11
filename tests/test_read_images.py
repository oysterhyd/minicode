"""Actual tool image results survive persistence and reach model wire requests."""
import asyncio
import base64
import json
from types import SimpleNamespace

import httpx
import pytest

from minicode.core.models import EventType, ToolResultBlock, Message, ToolUseBlock
from minicode.providers.commandcode import CommandCodeProvider, _build_payload
from minicode.providers.anthropic_provider import AnthropicProvider
from minicode.providers.errors import ProviderRequestError
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.storage.attachments import AttachmentStore
from minicode.tools.base import ToolContext
from minicode.tools.files import ReadTool
from minicode.tools.mcp import McpConnector

PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aO1sAAAAASUVORK5CYII=")


def test_read_image_reaches_real_adapter_and_restores_without_original(tmp_path):
    source = tmp_path / "截图.png"
    source.write_bytes(PNG)
    requests = []
    events = []

    def reply(request):
        payload = json.loads(request.content)
        requests.append(payload)
        if len(requests) == 1:
            delta = {"tool_calls": [{"index": 0, "id": "read-picture", "type": "function",
                     "function": {"name": "read", "arguments": json.dumps({"path": "截图.png"})}}]}
            finish = "tool_calls"
        else:
            source.unlink()
            assert payload["messages"][-2]["role"] == "tool"
            parts = payload["messages"][-1]["content"]
            assert "read-picture" in parts[0]["text"]
            assert base64.b64decode(parts[1]["image_url"]["url"].split(",")[1]) == PNG
            delta, finish = {"content": "The image was read."}, "stop"
        data = {"choices": [{"delta": delta, "finish_reason": finish}]}
        return httpx.Response(200, content=("data: "+json.dumps(data)+"\n\ndata: [DONE]\n\n").encode())

    async def run():
        from minicode.tools.registry import default_registry
        provider = CommandCodeProvider(model="vision", api_key="test", base_url="https://example.test/v1")
        provider.input_modalities = ("image",)
        provider._transport = httpx.MockTransport(reply)
        store = SqliteStore(tmp_path / "sessions.db")
        async def on_event(event): events.append(event)
        runtime = AgentRuntime(provider=provider, registry=default_registry(), store=store,
                               policy=AutoAllowPolicy(), workspace=tmp_path, provider_name="openai",
                               model="vision", on_event=on_event)
        result = await runtime.run_turn("Read 截图.png")
        assert result.error is None and len(requests) == 2
        messages = store.get_messages(result.session_id)
        block = next(block for message in messages for block in message.content if isinstance(block, ToolResultBlock))
        assert len(block.images) == 1 and block.images[0].name == "截图.png"
        event = next(event for event in events if event.type == EventType.TOOL_CALL_RESULT)
        assert event.data["images"][0]["asset_id"] == block.images[0].asset_id
        assert len(json.dumps(event.data)) < 2000
        restored = _build_payload(model="vision", max_tokens=4096, messages=messages, system=None,
                                  tools=[], attachment_store=provider.attachment_store, input_modalities=("image",))
        assert base64.b64decode(restored["messages"][-2]["content"][1]["image_url"]["url"].split(",")[1]) == PNG
        provider.input_modalities = ()
        with pytest.raises(ProviderRequestError):
            _build_payload(model="text", max_tokens=4096, messages=messages, system=None,
                           tools=[], attachment_store=provider.attachment_store)
        store.close()
        reopened = SqliteStore(tmp_path / "sessions.db")
        saved = next(block for message in reopened.get_messages(result.session_id) for block in message.content if isinstance(block, ToolResultBlock))
        assert saved.images == block.images
        reopened.close()
        await provider.aclose()
    asyncio.run(run())


def test_read_image_capability_failure_has_no_image_and_blocks_escape(tmp_path):
    (tmp_path / "valid.png").write_bytes(PNG)
    (tmp_path / "invalid.png").write_bytes(b"not an image")
    cache = AttachmentStore(tmp_path / "cache")
    tool = ReadTool()
    context = ToolContext(workspace=tmp_path, attachment_store=cache)
    unavailable = asyncio.run(tool.run({"path": "valid.png"}, context))
    assert not unavailable.success and not unavailable.images and "capability" in unavailable.error
    context.input_modalities = ("image",)
    for arguments in ({"path": "invalid.png"}, {"path": "../outside.png"}, {"path": "valid.png", "offset": 2}):
        outcome = asyncio.run(tool.run(arguments, context))
        assert not outcome.success and not outcome.images


def test_anthropic_tool_result_embeds_corresponding_image(tmp_path):
    image = AttachmentStore(tmp_path).import_bytes(PNG, "tool.png", "image/png")
    provider = AnthropicProvider(model="vision", client=SimpleNamespace())
    provider.attachment_store = AttachmentStore(tmp_path)
    result = ToolResultBlock(tool_use_id="image-call", content="read tool.png", images=[image])
    serialized = provider._serialize_block(result)
    assert serialized["tool_use_id"] == "image-call"
    assert serialized["content"][0]["text"] == "read tool.png"
    assert base64.b64decode(serialized["content"][1]["source"]["data"]) == PNG


def test_mcp_image_content_preserved_and_invalid_content_not_displayed(tmp_path):
    from mcp.types import ImageContent
    context = ToolContext(workspace=tmp_path, attachment_store=AttachmentStore(tmp_path / "cache"), input_modalities=("image",))
    result = SimpleNamespace(content=[ImageContent(data=base64.b64encode(PNG).decode(), mimeType="image/png"),
                                     ImageContent(data="invalid base64", mimeType="image/png")],
                             structured_content=None, is_error=False)
    config = SimpleNamespace(name="images", timeout_s=10, verify=lambda: None)
    connector = McpConnector(config)
    async def connect():
        async def call_tool(name, args): return result
        return SimpleNamespace(call_tool=call_tool)
    connector._connect = connect
    outcome = asyncio.run(connector._call("read_image", {}, context))
    assert outcome.success and len(outcome.images) == 1
    assert "could not be loaded" in outcome.output
    context.input_modalities = ()
    unavailable = asyncio.run(connector._call("read_image", {}, context))
    assert not unavailable.images and "unavailable" in unavailable.output
    result.is_error = True
    context.input_modalities = ("image",)
    failed = asyncio.run(connector._call("read_image", {}, context))
    assert not failed.success and not failed.images
