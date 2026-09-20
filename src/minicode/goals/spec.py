"""Acceptance spec: typed schema and strict YAML loader for goal acceptance.

The YAML format is shared with (future) evals and must stay stable::

    id: pagination-boundary          # optional, default "goal"
    title: 修复分页越界               # optional
    max_fix_attempts: 3              # optional, default 3
    items:                           # required, at least one
      - id: tests-pass               # required, unique
        type: command                # command | artifact | protected
        command: python -m pytest    # required for type=command
      - id: report-exists
        type: artifact
        path: report.txt             # required for type=artifact/protected

Loading is strict: unknown fields, missing kind-specific fields, empty item
lists and duplicate ids are all rejected with a Chinese ``ValueError`` that
carries the file path and, best effort, the offending line number.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, Literal

import yaml
from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator
from yaml.nodes import MappingNode, SequenceNode

ItemKind = Literal["command", "artifact", "protected"]

_ITEM_TYPES = ("command", "artifact", "protected")


class AcceptanceItem(BaseModel):
    """One acceptance check item."""

    model_config = ConfigDict(extra="forbid")

    id: str
    type: ItemKind
    command: str | None = None
    path: str | None = None
    description: str | None = None

    @model_validator(mode="after")
    def _require_kind_specific_field(self) -> "AcceptanceItem":
        if self.type == "command" and not (self.command and self.command.strip()):
            raise ValueError("type=command 的检查项必须提供 command 字段")
        if self.type in ("artifact", "protected") and not (self.path and self.path.strip()):
            raise ValueError(f"type={self.type} 的检查项必须提供 path 字段")
        return self


class AcceptanceSpec(BaseModel):
    """Top-level acceptance configuration."""

    model_config = ConfigDict(extra="forbid")

    id: str = "goal"
    title: str = ""
    max_fix_attempts: int = Field(default=3, ge=1)
    items: list[AcceptanceItem] = Field(min_length=1)

    @model_validator(mode="after")
    def _unique_item_ids(self) -> "AcceptanceSpec":
        seen: set[str] = set()
        for item in self.items:
            if item.id in seen:
                raise ValueError(f"检查项 id 重复: {item.id!r}")
            seen.add(item.id)
        return self

    @classmethod
    def from_yaml(cls, path: Path) -> "AcceptanceSpec":
        """Load and validate an acceptance spec from *path*.

        Every failure mode (missing file, unreadable file, YAML syntax error,
        non-mapping root, unknown field, schema violation) raises ValueError
        with a Chinese message containing the file path and, where possible,
        the 1-based line number of the offending entry.
        """
        path = Path(path)
        if not path.is_file():
            raise ValueError(f"验收配置文件不存在: {path}")
        try:
            raw = path.read_text(encoding="utf-8")
        except UnicodeDecodeError as exc:
            raise ValueError(f"验收配置文件不是有效的 UTF-8 文本 ({path}): {exc}") from exc
        except OSError as exc:
            raise ValueError(f"验收配置文件无法读取 ({path}): {exc}") from exc

        try:
            data = yaml.safe_load(raw)
        except yaml.YAMLError as exc:
            mark = getattr(exc, "problem_mark", None)
            line = getattr(mark, "line", None)
            where = f"，第 {line + 1} 行" if isinstance(line, int) else ""
            raise ValueError(f"验收配置 YAML 解析失败 ({path}{where}): {exc}") from exc

        if not isinstance(data, dict):
            raise ValueError(
                f"验收配置根节点必须是映射（mapping），实际为 {type(data).__name__} ({path})"
            )

        # Best-effort location index for error messages; safe_load already
        # proved the document parses, so this cannot realistically fail.
        try:
            lines = _key_line_index(yaml.compose(raw))
        except yaml.YAMLError:  # pragma: no cover
            lines = {}

        try:
            return cls.model_validate(data)
        except ValidationError as exc:
            parts = [_format_spec_error(err, lines) for err in exc.errors()[:8]]
            raise ValueError(f"验收配置校验失败 ({path}): " + "；".join(parts)) from exc


def _key_line_index(node: Any, prefix: tuple = ()) -> dict[tuple, int]:
    """Map pydantic-style loc tuples to 1-based YAML lines, best effort."""
    lines: dict[tuple, int] = {}
    if isinstance(node, MappingNode):
        for key_node, value_node in node.value:
            key = key_node.value
            if not isinstance(key, str):
                continue
            child = prefix + (key,)
            lines[child] = key_node.start_mark.line + 1
            lines.update(_key_line_index(value_node, child))
    elif isinstance(node, SequenceNode):
        for idx, item_node in enumerate(node.value):
            child = prefix + (idx,)
            lines[child] = item_node.start_mark.line + 1
            lines.update(_key_line_index(item_node, child))
    return lines


def _lookup_line(lines: dict[tuple, int], loc: tuple) -> int | None:
    key = tuple(loc)
    while key:
        if key in lines:
            return lines[key]
        key = key[:-1]
    return None


def _format_spec_error(err: dict[str, Any], lines: dict[tuple, int]) -> str:
    """Render one pydantic error as a Chinese, line-annotated message."""
    loc = tuple(err.get("loc", ()))
    etype = str(err.get("type", ""))
    msg = str(err.get("msg", ""))
    field = ".".join(str(p) for p in loc)

    if etype == "extra_forbidden":
        text = f"未知字段 '{field}'（配置文件中不允许该字段）"
    elif etype == "missing":
        text = f"缺少必填字段 '{field}'"
    elif etype == "too_short":
        text = f"字段 '{field}' 至少需要 1 项检查"
    elif etype == "value_error":
        text = msg.split("Value error, ", 1)[-1]
    elif etype == "literal_error":
        text = f"字段 '{field}' 取值非法: {err.get('input')!r}"
        if field.endswith("type"):
            text += f"（type 必须是 {'/'.join(_ITEM_TYPES)} 之一）"
    else:
        text = f"字段 '{field}' 校验失败: {msg}"

    line = _lookup_line(lines, loc)
    if line is not None:
        return f"第 {line} 行: {text}"
    return text
