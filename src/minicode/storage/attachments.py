"""Content-addressed local attachments, shared by a session database's hosts."""
from __future__ import annotations

import base64
import hashlib
import os
import tempfile
from pathlib import Path

from minicode.core.models import MediaBlock

MAX_ATTACHMENT_BYTES = 20_000_000
MAX_TOTAL_BYTES = 24_000_000
MAX_ATTACHMENTS = 10
FORMATS = {
    ".png": ("image", "image/png"), ".jpg": ("image", "image/jpeg"), ".jpeg": ("image", "image/jpeg"),
    ".gif": ("image", "image/gif"), ".webp": ("image", "image/webp"), ".pdf": ("pdf", "application/pdf"),
    ".wav": ("audio", "audio/wav"), ".mp3": ("audio", "audio/mpeg"),
    ".mp4": ("video", "video/mp4"), ".mov": ("video", "video/quicktime"), ".webm": ("video", "video/webm"),
}
SUFFIXES = {mime: suffix for suffix, (_, mime) in FORMATS.items()}
MIME_KINDS = {mime: kind for kind, mime in FORMATS.values()}


def check_attachments(blocks, modalities, api_style=None):
    from minicode.providers.errors import ProviderRequestError
    if len(blocks) > MAX_ATTACHMENTS or sum(b.size for b in blocks) > MAX_TOTAL_BYTES:
        raise ProviderRequestError("最多 10 个附件，附件总大小不能超过 24 MB")
    for block in blocks:
        if block.kind not in modalities:
            raise ProviderRequestError(f"当前模型未启用 {block.kind} 输入能力：{block.name}，请在模型设置中确认能力或切换模型")
        if api_style == "anthropic" and block.kind in {"audio", "video"}:
            raise ProviderRequestError("Anthropic Messages 接口不支持原生音频或视频附件，请选择支持这些输入的兼容接口")


class AttachmentStore:
    def __init__(self, root: Path):
        self.root = root

    def import_file(self, filename: str) -> MediaBlock:
        source = Path(filename)
        if source.suffix.lower() not in FORMATS or not source.is_file():
            raise ValueError("请选择图片、PDF、WAV / MP3 音频或 MP4 / MOV / WebM 视频")
        with source.open("rb") as stream:
            data = stream.read(MAX_ATTACHMENT_BYTES + 1)
        kind, mime = FORMATS[source.suffix.lower()]
        return self.import_bytes(data, source.name, mime)

    def import_bytes(self, data: bytes, name: str, mime: str) -> MediaBlock:
        """Import an actual tool response using the same validation as files."""
        if mime not in MIME_KINDS:
            raise ValueError("Unsupported attachment MIME type")
        if not data or len(data) > MAX_ATTACHMENT_BYTES:
            raise ValueError("附件不能为空，单个文件不能超过 20 MB")
        kind = MIME_KINDS[mime]
        valid = {
            "image/png": data.startswith(b"\x89PNG\r\n\x1a\n"), "image/jpeg": data.startswith(b"\xff\xd8\xff"),
            "image/gif": data.startswith((b"GIF87a", b"GIF89a")),
            "image/webp": data[:4] == b"RIFF" and data[8:12] == b"WEBP",
            "application/pdf": b"%PDF-" in data[:1024],
            "audio/wav": data[:4] == b"RIFF" and data[8:12] == b"WAVE",
            "audio/mpeg": data.startswith(b"ID3") or (len(data)>1 and data[0] == 255 and data[1] & 224 == 224),
            "video/mp4": data[4:8] == b"ftyp", "video/quicktime": data[4:8] == b"ftyp",
            "video/webm": data.startswith(b"\x1aE\xdf\xa3"),
        }[mime]
        if not valid:
            raise ValueError(f"文件内容与格式不符：{name}")
        digest = hashlib.sha256(mime.encode() + b"\0" + data).hexdigest()
        block = MediaBlock(asset_id=digest, kind=kind, name=name, mime_type=mime, size=len(data))
        self.root.mkdir(parents=True, exist_ok=True)
        for target, payload in [(self.root / (digest + SUFFIXES[mime]), data), (self.root / (digest + ".json"), block.model_dump_json().encode("utf-8"))]:
            temporary = None
            try:
                with tempfile.NamedTemporaryFile(dir=self.root, delete=False) as stream:
                    temporary = Path(stream.name)
                    stream.write(payload)
                    stream.flush()
                    os.fsync(stream.fileno())
                os.replace(temporary, target)
            finally:
                if temporary:
                    temporary.unlink(missing_ok=True)
        return block

    def get(self, asset_id: str) -> MediaBlock:
        from minicode.providers.errors import ProviderRequestError
        if len(asset_id) != 64 or any(c not in "0123456789abcdef" for c in asset_id):
            raise ValueError("无效附件 ID")
        try:
            block = MediaBlock.model_validate_json((self.root / (asset_id + ".json")).read_bytes())
        except (OSError, ValueError) as exc:
            raise ProviderRequestError("附件不存在或已损坏，请重新添加") from exc
        if block.asset_id != asset_id or block.mime_type not in SUFFIXES or MIME_KINDS[block.mime_type] != block.kind:
            raise ProviderRequestError("附件元数据已损坏，请重新添加")
        return block

    def path(self, block: MediaBlock) -> Path:
        return self.root / (block.asset_id + SUFFIXES[block.mime_type])

    def encoded(self, block: MediaBlock) -> str:
        from minicode.providers.errors import ProviderRequestError
        saved = self.get(block.asset_id)
        if saved.kind != block.kind or saved.mime_type != block.mime_type or saved.size != block.size:
            raise ProviderRequestError("附件元数据不一致，请重新添加")
        try:
            with self.path(saved).open("rb") as stream:
                data = stream.read(MAX_ATTACHMENT_BYTES+1)
        except OSError as exc:
            raise ProviderRequestError(f"附件无法读取：{block.name}") from exc
        if len(data) != block.size or hashlib.sha256(block.mime_type.encode()+b"\0"+data).hexdigest() != block.asset_id:
            raise ProviderRequestError(f"附件已损坏：{block.name}")
        return base64.b64encode(data).decode("ascii")
