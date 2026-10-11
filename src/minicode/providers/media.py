"""Explicit wire formats for declared multimodal inputs."""
from minicode.core.models import MediaBlock
from minicode.storage.attachments import AttachmentStore
from minicode.providers.errors import ProviderRequestError


def content_part(block: MediaBlock, store: AttachmentStore, style: str) -> dict:
    data = store.encoded(block)
    if style == "anthropic":
        if block.kind not in {"image", "pdf"}:
            raise ProviderRequestError("Anthropic Messages 不支持原生音频或视频输入")
        return {"type": "image" if block.kind == "image" else "document",
                "source": {"type": "base64", "media_type": block.mime_type, "data": data}}
    uri = f"data:{block.mime_type};base64,{data}"
    if block.kind == "image":
        return {"type": "image_url", "image_url": {"url": uri}}
    if block.kind == "pdf":
        return {"type": "file", "file": {"filename": block.name, "file_data": uri}}
    if block.kind == "audio":
        return {"type": "input_audio", "input_audio": {"data": data, "format": "wav" if block.mime_type == "audio/wav" else "mp3"}}
    return {"type": "video_url", "video_url": {"url": uri}}
