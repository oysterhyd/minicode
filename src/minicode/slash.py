"""Slash-command registry shared by the CLI REPL and the TUI.

Single source of truth for command names, usage lines, Chinese summaries
and prefix filtering (the autocomplete candidate list). Handlers stay in
the frontends; this module only describes and filters the commands.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SlashCommand:
    """One slash command and how it is presented to the user."""

    name: str  # "/model"
    usage: str  # "/model [name]"
    summary: str  # Chinese one-liner
    aliases: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class SubmenuItem:
    """One option inside a cascading second-level menu (submenu)."""

    value: str  # applied value, e.g. "z.ai/glm-5.3-flash" or "low"
    detail: str = ""  # "(上下文 1M)" style annotation
    badges: tuple[str, ...] = ()  # ("当前", "默认") markers


#: Commands that open a cascading submenu instead of executing directly on
#: Enter. Maps command name -> submenu title (breadcrumb label). A command
#: belongs here as soon as its usage takes a value from a known set (enumerated
#: argument) or from a list the harness can enumerate (stored sessions);
#: commands without arguments, or with a free-form one, execute directly.
SUBMENU_COMMANDS: dict[str, str] = {
    "/model": "选择模型",
    "/effort": "选择推理预算",
    "/permissions": "选择权限模式",
    "/resume": "选择要恢复的会话",
}


SLASH_COMMANDS: tuple[SlashCommand, ...] = (
    SlashCommand("/help", "/help", "显示本帮助", aliases=("/?",)),
    SlashCommand("/model", "/model [名称]", "查看或切换活跃模型（z.ai/glm-5.3-flash 等）"),
    SlashCommand(
        "/effort",
        "/effort [off|low|medium|high|xhigh|max]",
        "调整推理预算（reasoning effort，六档）",
    ),
    SlashCommand(
        "/permissions",
        "/permissions [default|accept_edits|bypass]",
        "查看或切换权限模式",
    ),
    SlashCommand("/clear", "/clear", "清空屏幕显示（保留会话上下文）"),
    SlashCommand("/new", "/new", "彻底重置上下文，开启全新会话"),
    SlashCommand("/compact", "/compact", "手动压缩当前会话上下文"),
    SlashCommand("/skill", "/skill [名称|off 名称]", "列出、激活或停用本地技能"),
    SlashCommand("/sessions", "/sessions", "列出历史会话"),
    SlashCommand("/resume", "/resume <会话ID8>", "恢复一个历史会话"),
    SlashCommand("/continue", "/continue", "从上次暂停处继续未完成任务"),
    SlashCommand("/exit", "/exit", "退出（或 Ctrl+Q / Ctrl+C 两次）", aliases=("/quit",)),
)


def filter_commands(prefix: str) -> list[SlashCommand]:
    """Commands whose name starts with *prefix* (e.g. ``"/h"`` -> ``/help``).

    ``"/"`` alone (or an empty string) returns everything; *prefix* is
    lowercased for the match and expected to include the leading slash.
    """
    prefix = prefix.lower()
    return [cmd for cmd in SLASH_COMMANDS if cmd.name.startswith(prefix)]


def format_command_lines() -> list[str]:
    """Aligned ``  /usage  summary`` lines for the /help display."""
    return [f"  {cmd.usage:<30} {cmd.summary}" for cmd in SLASH_COMMANDS]
