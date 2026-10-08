"""Verify the MyGo archive's signed SHA-256 against the committed public key."""
from __future__ import annotations

import base64
import hashlib
import json
import sys
from pathlib import Path
from urllib.parse import urlparse

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


def verify(manifest_path: Path, config_path: Path) -> Path:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    config = json.loads(config_path.read_text(encoding="utf-8"))
    if manifest["version"] != config["version"]:
        raise ValueError("update version differs from application version")
    parsed = urlparse(manifest["url"])
    if parsed.scheme != "https" or parsed.hostname != "github.com":
        raise ValueError("update URL must use GitHub HTTPS")
    expected = f"/{config['updates']['github']}/releases/download/v{config['version']}/"
    if not parsed.path.startswith(expected):
        raise ValueError("update URL does not point to the configured release")
    archive = manifest_path.parent / Path(parsed.path).name
    if not archive.is_file() or archive.stat().st_size != manifest["size"]:
        raise ValueError("update archive is missing or has the wrong size")
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").digest()
    public = Ed25519PublicKey.from_public_bytes(base64.b64decode(config["updates"]["publicKey"], validate=True))
    public.verify(base64.b64decode(manifest["signature"], validate=True), digest)
    return archive


if __name__ == "__main__":
    archive = verify(Path(sys.argv[1]), Path(sys.argv[2]))
    print(f"Signed update verified: {archive.name}")
