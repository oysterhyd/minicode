"""Read existing agent resources without executing or changing their sources.

Only availability selections belong to MiniCode. Foreign runtime hooks are not
executed; installed packages can contribute declarative skills and MCP servers.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import tomllib
import yaml
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit

from minicode.configuration import HarnessConfiguration
from minicode.context.extensions import Skill, _skill_metadata

_LIMIT = 1_000_000


def _read(path: Path):
    with path.open("rb") as stream:
        data = stream.read(_LIMIT + 1)
    if len(data) > _LIMIT:
        raise ValueError("resource configuration exceeds the size limit")
    return tomllib.loads(data.decode("utf-8-sig")) if path.suffix == ".toml" else json.loads(data)


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        content = stream.read(_LIMIT + 1)
    if len(content) > _LIMIT:
        raise ValueError("resource configuration exceeds the size limit")
    return hashlib.sha256(content).hexdigest()


def _identity(kind: str, path: Path, name: str = "") -> str:
    return kind + ":" + hashlib.sha256((str(path.resolve()) + "\0" + name).encode()).hexdigest()[:24]


def _expand(value: str, variables: dict[str, str] | None = None) -> str:
    def replace(match):
        name = match.group(1)
        if variables and name in variables:
            return variables[name]
        if name not in os.environ:
            raise ValueError(f"MCP requires environment variable {name}")
        return os.environ[name]
    return re.sub(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", replace, value)


@dataclass(frozen=True)
class LocalMcpServerConfig:
    name: str
    command: str
    args: tuple[str, ...]
    cwd: Path
    timeout_s: float
    plugin: str
    plugin_version: str
    plugin_digest: str
    transport: str
    url: str
    env: dict[str, str]
    headers: dict[str, str]
    source_path: Path
    configuration_error: str = ""

    def verify(self):
        if self.configuration_error:
            raise ValueError(self.configuration_error)
        if _digest(self.source_path) != self.plugin_digest:
            raise ValueError("local MCP configuration changed; reload extensions")


class LocalExtensionCatalog:
    def __init__(self, workspace: Path, *, configuration: HarnessConfiguration | None = None,
                 home: Path | None = None):
        self.workspace = workspace.resolve()
        self.home = (home or Path.home()).resolve()
        self.configuration = configuration or HarnessConfiguration()
        self._entries: dict[str, dict] = {}
        self._skills: dict[str, Skill] = {}
        self._servers: dict[str, tuple[Path, dict]] = {}
        self._plugin_resources: dict[str, list[str]] = {}

    def _add(self, kind, path, name, description, scope, source, *, supported=True,
             detail="", default=False, parent=""):
        key = _identity(kind, path, name + parent)
        if key in self._entries:
            return key
        selections = self._selections
        enabled = selections.get(key, default) is True
        if parent:
            enabled = selections.get(parent, False) is True
        entry = dict(id=key, kind=kind, name=name, description=description[:1000],
                     scope=scope, source=source, path=str(path), supported=supported,
                     status="available" if supported else "unsupported", detail=detail,
                     enabled=enabled and supported, parentId=parent)
        self._entries[key] = entry
        return key

    def _skills_at(self, root, scope, source, *, default=False, parent=""):
        if not root.is_dir():
            return
        paths = [root / "SKILL.md"] if (root / "SKILL.md").is_file() else sorted(root.glob("*/SKILL.md"))
        for path in paths[:1000]:
            try:
                name, description = _skill_metadata(path)
                # Resolve shared skills symlinks once; activation pins this file.
                path = path.resolve()
                key = self._add("skill", path, name, description, scope, source,
                                default=default, parent=parent)
                self._skills[key] = Skill(name, description, path, path.parent, source,
                                           hashlib.sha256((name + description).encode()).hexdigest())
                if parent:
                    self._plugin_resources[parent].append(key)
            except (OSError, ValueError, UnicodeError, yaml.YAMLError) as exc:
                self._add("skill", path, path.parent.name, "", scope, source,
                          supported=False, detail=type(exc).__name__)

    def _mcp_at(self, path, scope, source, *, parent="", individual=False, plugin_root=None,
                raw_override=None):
        if not path.is_file():
            return
        try:
            raw = _read(path) if raw_override is None else raw_override
            if not isinstance(raw, dict):
                raise ValueError("MCP configuration must be an object")
            if individual:
                servers = {str(raw.get("id") or path.stem): raw}
            else:
                servers = raw.get("mcpServers", raw.get("mcp_servers", raw.get("servers", {})))
                if not isinstance(servers, dict):
                    raise ValueError("MCP servers must be an object")
            for name, server in list(servers.items())[:256]:
                if not isinstance(server, dict):
                    continue
                transport = server.get("transport", server.get("type", "http" if server.get("url") else "stdio"))
                if isinstance(transport, str):
                    transport = {"streamable-http": "http", "streamable_http": "http"}.get(transport, transport)
                else:
                    transport = "invalid"
                valid = transport in {"stdio", "http", "sse"}
                valid &= (isinstance(server.get("command"), str) and bool(server["command"])) if transport == "stdio" else isinstance(server.get("url"), str)
                args = server.get("args") or []
                valid &= isinstance(args, list) and all(isinstance(a, str) for a in args)
                valid &= all(isinstance(server.get(field) or {}, dict) and
                             all(isinstance(k, str) and isinstance(v, str) for k, v in (server.get(field) or {}).items())
                             for field in ("env", "headers"))
                if transport in {"http", "sse"} and valid:
                    url = urlsplit(server["url"])
                    valid &= url.scheme in {"http", "https"} and bool(url.netloc) and not url.username and not url.password
                description = server.get("description") or ""
                label = server.get("label")
                label = label.strip()[:240] if isinstance(label, str) and label.strip() else str(name)[:240]
                key = self._add("mcp", path, label, str(description), scope,
                                source, supported=bool(valid), detail=str(transport), parent=parent)
                self._servers[key] = (path, {**server, "transport": transport, "sourceName": name,
                                             "pluginRoot": str(plugin_root or "")})
                if parent:
                    self._plugin_resources[parent].append(key)
        except (OSError, ValueError, UnicodeError):
            self._add("mcp", path, path.stem, "", scope, source, supported=False,
                      detail="Invalid configuration")

    def _plugin_at(self, root, scope, source):
        manifests = [root / ".claude-plugin" / "plugin.json", root / ".codex-plugin" / "plugin.json",
                     root / "manifest.json"]
        path = next((p for p in manifests if p.is_file()), None)
        if path is None:
            return
        try:
            raw = _read(path)
            if not isinstance(raw, dict):
                return
            name = str(raw.get("name") or root.name)
            key = self._add("plugin", path, name, str(raw.get("description") or ""), scope, source)
            self._plugin_resources[key] = []
            skills = raw.get("skills", "./skills")
            for relative in ([skills] if isinstance(skills, str) else skills if isinstance(skills, list) else []):
                if not isinstance(relative, str):
                    continue
                target = (root / relative).resolve()
                if target.is_relative_to(root.resolve()):
                    self._skills_at(target, scope, source, parent=key)
            self._mcp_at(root / ".mcp.json", scope, source, parent=key, plugin_root=root)
            contributes = raw.get("contributes") or {}
            mcp = raw.get("mcpServers") or (contributes.get("mcpServers") if isinstance(contributes, dict) else None)
            if isinstance(mcp, list):
                mcp = {str(item.get("id") or item.get("name") or index): item
                       for index, item in enumerate(mcp) if isinstance(item, dict)}
            if isinstance(mcp, dict):
                self._mcp_at(path, scope, source, parent=key, plugin_root=root,
                             raw_override={"mcpServers": mcp})
            elif isinstance(mcp, str):
                target = (root / mcp).resolve()
                if target.is_relative_to(root.resolve()):
                    self._mcp_at(target, scope, source, parent=key, plugin_root=root)
            if not any(self._entries[child]["supported"] for child in self._plugin_resources[key]):
                self._entries[key].update(supported=False, enabled=False, status="unsupported",
                                          detail="No compatible skills or MCP resources; runtime hooks are unsupported")
            else:
                children = [self._entries[child] for child in self._plugin_resources[key]]
                self._entries[key]["detail"] = (
                    f"{sum(c['kind'] == 'skill' for c in children)} skills, "
                    f"{sum(c['kind'] == 'mcp' for c in children)} MCP servers: "
                    + ", ".join(c["name"] for c in children)[:240]
                    + "; foreign runtime hooks are not executed")
        except (OSError, ValueError, UnicodeError):
            self._add("plugin", path, root.name, "", scope, source, supported=False,
                      detail="Invalid plugin manifest")

    def scan(self) -> list[dict]:
        self._entries.clear()
        self._skills.clear()
        self._servers.clear()
        self._plugin_resources.clear()
        self._selections = self.configuration.read().get("localExtensions", {})
        if not isinstance(self._selections, dict):
            raise ValueError("local extension selections must be an object")
        scopes = [("global", self.home)]
        if self.workspace != self.home:
            scopes.append(("project", self.workspace))
        for scope, base in scopes:
            for relative, source in ((".minicode/skills", "MiniCode"), (".agents/skills", "Agents"),
                                     (".claude/skills", "Claude"), (".codex/skills", "Codex"),
                                     (".pi/skills", "Pi"), (".pi/agent/skills", "Pi")):
                self._skills_at(base / relative, scope, source, default=source == "MiniCode")
            servers = base / ".agents" / "servers"
            if servers.is_dir():
                for path in sorted(servers.glob("*.json"))[:256]:
                    self._mcp_at(path, scope, "Agents", individual=True)
            for relative, source in ((".mcp.json", "Claude"), (".claude.json", "Claude"),
                                     (".claude/settings.json", "Claude"),
                                     (".codex/config.toml", "Codex"), (".pi/mcp.json", "Pi"),
                                     (".pi/agent/mcp.json", "Pi"), (".cursor/mcp.json", "Cursor")):
                self._mcp_at(base / relative, scope, source)
            # Installed package manifests only: no npm lookup, installation or hooks.
            for cache, source in ((base / ".codex/plugins/cache", "Codex"),
                                  (base / ".claude/plugins/cache", "Claude")):
                if cache.is_dir():
                    roots = sorted(cache.glob("*/*/*"))[:1000]
                    for root in roots:
                        if root.is_dir():
                            self._plugin_at(root, scope, source)
            index = base / ".claude/plugins/installed_plugins.json"
            if index.is_file():
                try:
                    data = _read(index).get("plugins", {})
                    for installs in data.values():
                        for install in installs if isinstance(installs, list) else []:
                            if isinstance(install, dict) and isinstance(install.get("installPath"), str):
                                self._plugin_at(Path(install["installPath"]), scope, "Claude")
                except (OSError, ValueError, AttributeError):
                    pass
        # PI-Desktop keeps installed desktop packages under its private profile.
        # View/JS-only plugins remain visible but unavailable in MiniCode.
        plugin_roots = [(self.home / ".pi-desktop/plugins", "global"),
                        (self.home / ".agents/plugins", "global")]
        if self.workspace != self.home:
            plugin_roots.append((self.workspace / ".agents/plugins", "project"))
        for base, scope in plugin_roots:
            if base.is_dir():
                for root in [*sorted(base.glob("*")), *sorted(base.glob("imported/*")),
                             *sorted(base.glob("installed/*"))][:1000]:
                    if root.is_dir():
                        self._plugin_at(root, scope, "Pi Desktop")
        agents_override = os.environ.get("PI_DESKTOP_AGENTS_DIR")
        if agents_override:
            base = Path(agents_override).expanduser()
            if base.resolve() != (self.home / ".agents").resolve():
                self._skills_at(base / "skills", "global", "Pi Desktop")
                if (base / "servers").is_dir():
                    for path in sorted((base / "servers").glob("*.json"))[:256]:
                        self._mcp_at(path, "global", "Pi Desktop", individual=True)
        if os.name == "nt":
            claude_desktop = self.home / "AppData/Roaming/Claude/claude_desktop_config.json"
        else:
            claude_desktop = self.home / ("Library/Application Support/Claude/claude_desktop_config.json"
                                         if sys.platform == "darwin" else ".config/Claude/claude_desktop_config.json")
        self._mcp_at(claude_desktop, "global", "Claude Desktop")
        for key, (_, raw) in self._servers.items():
            self._entries[key]["runtimeName"] = self._server_name(key, raw)
        return sorted((dict(e) for e in self._entries.values()), key=lambda e: (e["kind"], e["scope"], e["name"], e["id"]))

    @staticmethod
    def _server_name(key, raw):
        readable = re.sub(r"[^a-z0-9_]", "_", str(raw["sourceName"]).lower())[:16]
        return "local_" + readable + "_" + key.split(":")[1][:6]

    def set_enabled(self, key: str, enabled: bool) -> list[dict]:
        if not isinstance(enabled, bool):
            raise ValueError("enabled must be boolean")
        self.scan()
        entry = self._entries.get(key)
        if entry is None:
            raise ValueError("local resource is no longer available")
        if entry["parentId"]:
            raise ValueError("activate plugin resources using the parent plugin")
        if enabled and not entry["supported"]:
            raise ValueError("resource is incompatible with MiniCode")
        data = self.configuration.read()
        data.setdefault("localExtensions", {})[key] = enabled
        self.configuration.write(data)
        return self.scan()

    def apply_skills(self, catalog):
        if not self._entries:
            self.scan()
        # Existing MiniCode resources retain their default availability.
        disabled = {Path(e["path"]).resolve() for e in self._entries.values()
                    if e["kind"] == "skill" and e["source"] == "MiniCode" and not e["enabled"]}
        catalog.disabled_skills.update(name for name, skill in catalog.skills.items()
                                       if skill.path.resolve() in disabled)
        catalog.skills = {name: skill for name, skill in catalog.skills.items() if skill.path.resolve() not in disabled}
        for key, skill in self._skills.items():
            entry = self._entries[key]
            if entry["source"] == "MiniCode":
                continue
            # Availability changes must never rename a persisted active skill.
            name = f"local:{key.split(':')[1][:12]}:{skill.name}"
            if not entry["enabled"]:
                catalog.disabled_skills.add(name)
                continue
            catalog.skills[name] = Skill(name, skill.description, skill.path, skill.root,
                                         f"local:{entry['source']}:{entry['scope']}", skill.digest)

    def servers(self) -> list[LocalMcpServerConfig]:
        if not self._entries:
            self.scan()
        result = []
        for key, (path, raw) in self._servers.items():
            if not self._entries[key]["enabled"]:
                continue
            # Stable namespace allows different tools to have identically named servers.
            name = self._server_name(key, raw)
            variables = {n: raw["pluginRoot"] for n in ("CLAUDE_PLUGIN_ROOT", "CODEX_PLUGIN_ROOT", "PLUGIN_ROOT")}
            errors = []
            def expand(value):
                try:
                    return _expand(value, variables)
                except ValueError as exc:
                    errors.append(str(exc))
                    return value
            cwd = raw.get("cwd")
            cwd = Path(expand(cwd)).expanduser() if isinstance(cwd, str) and cwd else self.workspace
            if not cwd.is_absolute():
                cwd = path.parent / cwd
            result.append(LocalMcpServerConfig(
                name, expand(raw.get("command") or ""), tuple(expand(arg) for arg in raw.get("args") or []),
                cwd.resolve(), 30.0, "local:" + self._entries[key]["source"], "local", _digest(path),
                raw["transport"], expand(raw.get("url") or ""),
                {k: expand(v) for k, v in (raw.get("env") or {}).items()},
                {k: expand(v) for k, v in (raw.get("headers") or {}).items()}, path,
                "; ".join(dict.fromkeys(errors))))
        return result
