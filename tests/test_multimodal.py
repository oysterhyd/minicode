"""Attachment persistence, capability gates and real adapter wire payloads."""
import asyncio
import base64
import json
import sys
import wave
from pathlib import Path
from types import SimpleNamespace

import httpx
import pytest

from minicode.configuration import HarnessConfiguration
from minicode.core.models import MediaBlock, Message, ModelCapabilities, TextBlock
from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.anthropic_provider import AnthropicProvider
from minicode.providers.errors import ProviderRequestError
from minicode.storage import SqliteStore
from minicode.storage.attachments import AttachmentStore

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "desktop"))
import bridge as bridge_mod


@pytest.fixture
def media_files(tmp_path):
    sources = {
        "截图.png": base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aO1sAAAAASUVORK5CYII="),
        "文档.pdf": b"%PDF-1.4\n1 0 obj <</Type /Catalog>> endobj\n%%EOF",
        "演示.mp4": b"\0\0\0\x18ftypisom\0\0\0\0isommp42",
    }
    for name, data in sources.items(): (tmp_path/name).write_bytes(data)
    with wave.open(str(tmp_path/"录音.wav"), "wb") as stream:
        stream.setnchannels(1); stream.setsampwidth(2); stream.setframerate(8000); stream.writeframes(b"\0\0"*800)
    sources["录音.wav"] = (tmp_path/"录音.wav").read_bytes()
    return [tmp_path/name for name in ["截图.png", "文档.pdf", "录音.wav", "演示.mp4"]], sources


def test_capabilities_save_reload_and_old_defaults(tmp_path):
    config=HarnessConfiguration(tmp_path/"config.json")
    draft=dict(name="Gateway",baseUrl="https://example.test/v1",apiStyle="openai",apiKey="test-key",enabled=True,
               models=[dict(modelId="vision",name="Vision",contextWindow=100000,maxOutputTokens=4096,supportsEffort=True)])
    config.save_service(draft)
    assert config.models()[0]["capabilities"] == ModelCapabilities().model_dump()
    draft=config.public()["services"][0]
    draft["models"][0]["capabilities"] = dict(images=True,pdf=True,audio=True,video=True)
    config.save_service(draft)
    assert HarnessConfiguration(config.path).models()[0]["capabilities"] == draft["models"][0]["capabilities"]
    before=config.path.read_bytes()
    draft["models"][0]["capabilities"]["images"]="false"
    with pytest.raises(ValueError): config.save_service(draft)
    assert config.path.read_bytes()==before


def test_cache_survives_source_removal_and_rejects_corruption(media_files,tmp_path):
    paths, content=media_files
    store=AttachmentStore(tmp_path/"cache")
    blocks=[store.import_file(str(p)) for p in paths]
    for path in paths: path.unlink()
    for block in blocks:
        assert base64.b64decode(store.encoded(block))==content[block.name]
    store.path(blocks[0]).write_bytes(b"corrupt")
    with pytest.raises(ProviderRequestError,match="损坏"): store.encoded(blocks[0])
    with pytest.raises(ValueError): store.get("../arbitrary")
    bad=tmp_path/"invalid.png"; bad.write_bytes(b"not an image")
    with pytest.raises(ValueError,match="格式"): store.import_file(str(bad))


def test_bridge_sends_all_modalities_and_restores_cached_history(media_files,tmp_path,monkeypatch):
    paths, content=media_files
    config=HarnessConfiguration(tmp_path/"config.json")
    config.save_service(dict(name="Gateway",baseUrl="https://example.test/v1",apiStyle="openai",apiKey="test-key",enabled=True,
        models=[dict(modelId="vision",name="Vision",contextWindow=100000,maxOutputTokens=4096,supportsEffort=False,capabilities=dict(images=True,pdf=True,audio=True,video=True))]))
    monkeypatch.setattr(bridge_mod,"HarnessConfiguration",lambda:config)
    captured=[]
    def reply(request):
        captured.append(json.loads(request.content))
        body='data: '+json.dumps({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]})+'\n\ndata: [DONE]\n\n'
        return httpx.Response(200,content=body.encode())
    original=bridge_mod.Bridge.make_provider
    def make_provider(self,model):
        provider=original(self,model); provider._transport=httpx.MockTransport(reply); return provider
    monkeypatch.setattr(bridge_mod.Bridge,"make_provider",make_provider)
    async def scenario():
        b=bridge_mod.Bridge(SqliteStore(tmp_path/"sessions.db"))
        blocks=await b.handle("importAttachments",{"paths":[str(p) for p in paths]})
        for path in paths: path.unlink()
        await b.handle("sendPrompt",dict(text="分析附件",workspace=str(tmp_path),model=config.models()[0]["id"],attachments=blocks))
        await b.run_task
        parts=captured[0]["messages"][1]["content"]
        assert [p["type"] for p in parts]==["image_url","file","input_audio","video_url","text"]
        for index,key in [(0,"image_url"),(3,"video_url")]:
            assert base64.b64decode(parts[index][key]["url"].split(",",1)[1])==content[blocks[index]["name"]]
        assert base64.b64decode(parts[1]["file"]["file_data"].split(",",1)[1])==content["文档.pdf"]
        assert base64.b64decode(parts[2]["input_audio"]["data"])==content["录音.wav"]
        sid=b.runtime.session_id
        saved=b.store.get_messages(sid)[0]
        assert sum(isinstance(block,MediaBlock) for block in saved.content)==4
        assert "base64" not in saved.model_dump_json() and len(saved.model_dump_json())<3000
        await b.handle("selectSession",{"sessionId":sid})
        await b.runtime.run_turn("再检查一次")
        assert captured[1]["messages"][1]["content"]==parts
        await b.discard_runtime(); b.store.close()
    asyncio.run(scenario())


def test_anthropic_image_pdf_and_unsupported_modalities(media_files,tmp_path):
    paths,_=media_files
    store=AttachmentStore(tmp_path/"cache")
    blocks=[store.import_file(str(p)) for p in paths]
    provider=AnthropicProvider(model="test",client=SimpleNamespace())
    provider.attachment_store=store
    parts=provider._serialize_messages([Message(role="user",content=[*blocks[:2],TextBlock(text="describe")])])[0]["content"]
    assert [p["type"] for p in parts]==["image","document","text"]
    assert parts[1]["source"]["media_type"]=="application/pdf"
    with pytest.raises(ProviderRequestError): provider._serialize_messages([Message(role="user",content=[blocks[2]])])


def test_disabled_model_capability_rejects_before_http(media_files,tmp_path):
    paths,_=media_files
    store=AttachmentStore(tmp_path/"cache"); block=store.import_file(str(paths[0]))
    provider=CommandCodeProvider(model="custom",api_key="test-key",base_url="https://example.test/v1",transport=httpx.MockTransport(lambda _: pytest.fail("unexpected HTTP request")))
    provider.attachment_store=store
    async def scenario():
        with pytest.raises(ProviderRequestError,match="输入能力"):
            async for _ in provider.stream(system="",messages=[Message(role="user",content=[block])],tools=[]): pass
        await provider.aclose()
    asyncio.run(scenario())
