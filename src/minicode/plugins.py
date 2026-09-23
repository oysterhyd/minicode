"""Validated local plugin manifests and reproducible version locks."""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

from minicode import __version__


_NAME = re.compile(r"^[a-z][a-z0-9_]{0,31}$")
_VERSION = re.compile(r"^\d+\.\d+\.\d+$")
_MAX_MANIFEST = 64_000
_MAX_PLUGIN_BYTES = 16_000_000


def _fingerprint(root: Path) -> str:
    digest = hashlib.sha256()
    total = 0
    for path in sorted(root.rglob("*")):
        if "__pycache__" in path.parts or path.suffix in {".pyc", ".pyo"}:
            continue
        if path.is_symlink():
            raise ValueError(f"plugin contains a symbolic link: {path}")
        if not path.is_file():
            continue
        digest.update(path.relative_to(root).as_posix().encode("utf-8"))
        with path.open("rb") as stream:
            while chunk := stream.read(64 * 1024):
                total += len(chunk)
                if total > _MAX_PLUGIN_BYTES:
                    raise ValueError(f"plugin content is too large: {root}")
                digest.update(chunk)
    return digest.hexdigest()


@dataclass(frozen=True)
class McpServerConfig:
    name: str
    command: str
    args: tuple[str, ...]
    cwd: Path
    timeout_s: float
    plugin: str
    plugin_version: str
    plugin_digest: str

    def verify(self) -> None:
        if _fingerprint(self.cwd) != self.plugin_digest:
            raise ValueError(f"plugin content changed after discovery: {self.plugin}")


@dataclass(frozen=True)
class Plugin:
    name: str
    version: str
    digest: str
    path: Path
    enabled: bool
    skill_root: Path | None
    agent_root: Path | None
    servers: tuple[McpServerConfig, ...]


def _directory(root: Path, relative: Any) -> Path | None:
    if relative is None:
        return None
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute():
        raise ValueError("plugin directory must be a relative path")
    path = root / relative
    if path.is_symlink() or not path.resolve().is_relative_to(root) or not path.is_dir():
        raise ValueError(f"plugin directory is missing or outside plugin: {relative}")
    return path.resolve()


class PluginCatalog:
    """Load manifests without executing code; optional lock pins exact contents."""

    def __init__(self, workspace: Path, *, check_lock: bool = True):
        self.workspace = workspace.resolve()
        self.root = self.workspace / ".minicode" / "plugins"
        self.lock_path = self.workspace / ".minicode" / "plugins.lock.json"
        self.plugins: dict[str, Plugin] = {}
        self._agents: dict[str, Path] = {}
        server_names: set[str] = set()
        if self.root.is_dir():
            for path in sorted(self.root.glob("*/plugin.json")):
                root = path.parent.resolve()
                if path.is_symlink() or path.parent.is_symlink() or not root.is_relative_to(self.root.resolve()):
                    raise ValueError(f"plugin path is not trusted: {path}")
                data = path.read_bytes()
                if len(data) > _MAX_MANIFEST:
                    raise ValueError(f"plugin manifest is too large: {path}")
                raw = json.loads(data)
                if not isinstance(raw, dict):
                    raise ValueError(f"plugin manifest must be an object: {path}")
                name, version = raw.get("name"), raw.get("version")
                if not isinstance(name, str) or not _NAME.fullmatch(name) or name != root.name:
                    raise ValueError(f"invalid plugin name: {path}")
                if not isinstance(version, str) or not _VERSION.fullmatch(version):
                    raise ValueError(f"invalid plugin version: {path}")
                if raw.get("minicode_version") != __version__:
                    raise ValueError(f"plugin {name} requires minicode {raw.get('minicode_version')}; installed {__version__}")
                enabled = raw.get("enabled", False)
                if not isinstance(enabled, bool):
                    raise ValueError(f"plugin {name} enabled must be boolean")
                if name in self.plugins:
                    raise ValueError(f"duplicate plugin: {name}")
                digest = _fingerprint(root)
                skill_root = _directory(root, raw.get("skills"))
                agent_root = _directory(root, raw.get("agents"))
                server_configs = raw.get("mcp_servers", [])
                if not isinstance(server_configs, list):
                    raise ValueError(f"plugin {name} mcp_servers must be a list")
                servers: list[McpServerConfig] = []
                for server in server_configs:
                    if not isinstance(server, dict):
                        raise ValueError(f"invalid MCP server in plugin {name}")
                    server_name = server.get("name")
                    command = server.get("command")
                    args = server.get("args", [])
                    timeout = server.get("timeout_s", 30)
                    if (not isinstance(server_name, str) or not _NAME.fullmatch(server_name)
                            or (enabled and server_name in server_names)):
                        raise ValueError(f"duplicate or invalid MCP server name: {server_name}")
                    if not isinstance(command, str) or not command or not isinstance(args, list) or any(
                        not isinstance(arg, str) for arg in args
                    ):
                        raise ValueError(f"invalid MCP command for {server_name}")
                    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 0 < timeout <= 300:
                        raise ValueError(f"invalid MCP timeout for {server_name}")
                    if enabled:
                        server_names.add(server_name)
                    servers.append(McpServerConfig(
                        server_name, sys.executable if command == "$PYTHON" else command,
                        tuple(args), root, float(timeout), name, version, digest,
                    ))
                self.plugins[name] = Plugin(
                    name, version, digest, path, enabled, skill_root, agent_root, tuple(servers)
                )
                if enabled and agent_root is not None:
                    for agent_file in sorted(agent_root.glob("*/AGENT.md")):
                        if agent_file.is_symlink() or agent_file.parent.is_symlink():
                            raise ValueError(f"agent path is not trusted: {agent_file}")
                        agent_name = agent_file.parent.name
                        if not _NAME.fullmatch(agent_name):
                            raise ValueError(f"invalid plugin agent name: {agent_name}")
                        key = f"plugin:{name}:{agent_name}"
                        if key in self._agents:
                            raise ValueError(f"duplicate plugin agent: {key}")
                        self._agents[key] = agent_file
        if check_lock:
            self._check_lock()

    def _check_lock(self) -> None:
        if not self.lock_path.exists():
            return
        data = json.loads(self.lock_path.read_text(encoding="utf-8"))
        expected = self.lock_data()
        if data != expected:
            raise ValueError("plugin lock differs from local manifests; run `minicode plugins lock` after reviewing changes")

    def lock_data(self) -> dict[str, Any]:
        return {"plugins": {
            name: {"version": plugin.version, "sha256": plugin.digest}
            for name, plugin in sorted(self.plugins.items())
        }}

    def write_lock(self) -> Path:
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w", encoding="utf-8", dir=self.lock_path.parent,
                prefix=".plugins-lock-", suffix=".tmp", delete=False,
            ) as stream:
                temporary = Path(stream.name)
                stream.write(json.dumps(self.lock_data(), indent=2) + "\n")
            os.replace(temporary, self.lock_path)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        return self.lock_path

    def skill_roots(self) -> list[tuple[str, Path]]:
        return [(f"plugin:{p.name}", p.skill_root) for p in self.plugins.values()
                if p.enabled and p.skill_root is not None]

    def servers(self) -> list[McpServerConfig]:
        return [server for p in self.plugins.values() if p.enabled for server in p.servers]

    def agent_names(self) -> list[str]:
        return sorted(self._agents)

    def agent_instructions(self, kind: str) -> tuple[str, dict[str, str]]:
        path = self._agents.get(kind)
        if path is None:
            raise ValueError(f"unknown plugin agent: {kind}")
        plugin_name = kind.split(":", 2)[1]
        plugin = self.plugins[plugin_name]
        if _fingerprint(plugin.path.parent) != plugin.digest:
            raise ValueError(f"plugin content changed after discovery: {plugin_name}")
        content = path.read_bytes()
        if len(content) > 64_000:
            raise ValueError(f"plugin agent instructions are too large: {path}")
        text = content.decode("utf-8").replace("\r\n", "\n")
        if not text.startswith("---\n") or "\n---\n" not in text[4:]:
            raise ValueError(f"plugin agent needs YAML front matter: {path}")
        header, body = text[4:].split("\n---\n", 1)
        metadata = yaml.safe_load(header)
        if (not isinstance(metadata, dict) or metadata.get("name") != path.parent.name
                or not isinstance(metadata.get("description"), str)):
            raise ValueError(f"invalid plugin agent metadata: {path}")
        return body, {"plugin": plugin_name, "plugin_version": plugin.version,
                      "plugin_sha256": plugin.digest, "agent": path.parent.name}
