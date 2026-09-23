"""Scoped project instructions and lazily loaded local skills."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

import yaml


_MAX_INSTRUCTIONS = 64_000
_MAX_SKILL = 64_000


@dataclass(frozen=True)
class Source:
    path: Path
    digest: str
    content: str

    @classmethod
    def read(cls, path: Path, limit: int) -> "Source":
        with path.open("rb") as stream:
            data = stream.read(limit + 1)
        if len(data) > limit:
            raise ValueError(f"instruction file is too large: {path}")
        return cls(path, hashlib.sha256(data).hexdigest(), data.decode("utf-8"))

    def label(self) -> str:
        return f"{self.path} (sha256:{self.digest[:12]})"


class ProjectInstructions:
    """Read the root file first; discover nested files only for touched paths."""

    def __init__(self, workspace: Path):
        self.workspace = workspace.resolve()
        self.loaded: dict[Path, Source] = {}
        root = self.workspace / "AGENTS.md"
        if root.is_file():
            self.loaded[root] = Source.read(root, _MAX_INSTRUCTIONS)

    def discover(self, target: str) -> list[Source]:
        path = (self.workspace / target).resolve()
        try:
            relative = path.relative_to(self.workspace)
        except ValueError:
            return []
        found: list[Source] = []
        current = self.workspace
        scope_parts = relative.parts if path.is_dir() else relative.parts[:-1]
        for part in scope_parts:
            current /= part
            candidate = current / "AGENTS.md"
            if candidate.is_file() and candidate not in self.loaded:
                source = Source.read(candidate, _MAX_INSTRUCTIONS)
                self.loaded[candidate] = source
                found.append(source)
        return found

    def prompt_section(self) -> str:
        return "\n\n".join(
            f"项目指令来源：{source.label()}\n{source.content}"
            for source in self.loaded.values()
        )

    def sources(self) -> list[dict[str, str]]:
        return [{"path": str(s.path), "sha256": s.digest} for s in self.loaded.values()]


@dataclass(frozen=True)
class Skill:
    name: str
    description: str
    path: Path
    root: Path
    origin: str
    digest: str


def _skill_metadata(path: Path) -> tuple[str, str]:
    # Only the front matter is read during discovery. The body is loaded on
    # activation; large skills do not silently bloat every request.
    with path.open("r", encoding="utf-8") as stream:
        if stream.readline().strip() != "---":
            raise ValueError(f"skill has no YAML front matter: {path}")
        header: list[str] = []
        closed = False
        for _ in range(80):
            line = stream.readline(4097)
            if len(line) > 4096 or sum(map(len, header)) + len(line) > 8192:
                raise ValueError(f"skill header is too large: {path}")
            if not line:
                break
            if line.strip() == "---":
                closed = True
                break
            header.append(line)
        else:
            raise ValueError(f"skill header is too long: {path}")
    if not closed:
        raise ValueError(f"skill header is not closed: {path}")
    metadata = yaml.safe_load("".join(header)) or {}
    if not isinstance(metadata, dict):
        raise ValueError(f"invalid skill header: {path}")
    name = metadata.get("name")
    description = metadata.get("description")
    if not isinstance(name, str) or not name or not isinstance(description, str) or not description:
        raise ValueError(f"skill needs name and description: {path}")
    return name, description


class SkillCatalog:
    def __init__(self, workspace: Path, user_root: Path | None = None):
        roots = [("user", user_root or Path.home() / ".minicode" / "skills"),
                 ("project", workspace / ".minicode" / "skills")]
        self.skills: dict[str, Skill] = {}
        for origin, root in roots:
            if not root.is_dir():
                continue
            for path in sorted(root.glob("*/SKILL.md")):
                if path.is_symlink() or path.parent.is_symlink():
                    continue
                name, description = _skill_metadata(path)
                if name != path.parent.name:
                    raise ValueError(f"skill name must match directory: {path}")
                # A metadata fingerprint is enough for the catalog; full body
                # hashing happens only when the skill is activated.
                header = f"{name}\n{description}".encode()
                self.skills[name] = Skill(name, description, path, path.parent.resolve(), origin,
                                          hashlib.sha256(header).hexdigest())

    def listing(self, max_chars: int | None = None) -> str:
        if not self.skills:
            return "（未发现 Skills）"
        lines = [f"- {s.name}: {s.description[:240]} [{s.origin}]"
                 for s in self.skills.values()]
        listing = "\n".join(lines)
        if max_chars is not None and len(listing) > max_chars:
            return listing[:max_chars] + "\n…目录已截断；可用 skills_list 查看完整列表。"
        return listing

    def load(self, name: str) -> Source:
        skill = self.skills.get(name)
        if skill is None:
            raise ValueError(f"unknown skill: {name}")
        if (skill.path.is_symlink() or skill.path.parent.is_symlink()
                or skill.path.parent.resolve() != skill.root):
            raise ValueError(f"skill path changed: {name}")
        return Source.read(skill.path, _MAX_SKILL)

    def resource(self, name: str, path: str) -> Source:
        skill = self.skills.get(name)
        if skill is None:
            raise ValueError(f"unknown skill: {name}")
        if skill.path.is_symlink() or skill.path.parent.resolve() != skill.root:
            raise ValueError(f"skill path changed: {name}")
        root = skill.root
        target = (root / path).resolve()
        if not target.is_relative_to(root) or not target.is_file():
            raise ValueError("skill resource is outside the registered skill")
        return Source.read(target, _MAX_SKILL)
