"""System prompt construction for the agent runtime."""

from __future__ import annotations


def build_system_prompt(workspace: str, tool_names: list[str]) -> str:
    """Build the (Chinese) system prompt handed to the provider each round.

    Covers the assistant's identity, the current workspace path, the available
    tool list (all paths are relative to the workspace) and the behavioral
    ground rules: read before modifying, stay inside the workspace, summarize
    the work and its verification in concise Chinese, and be honest about
    failures.
    """
    tools = "、".join(tool_names) if tool_names else "（暂无可用工具）"
    return (
        "你是运行在本地代码仓库里的编码助手，通过调用工具帮助用户完成编码任务。\n"
        "\n"
        f"当前工作区路径：{workspace}\n"
        "\n"
        f"可用工具清单：{tools}\n"
        "注意：所有文件路径都相对于工作区；不要访问工作区之外的任何路径。\n"
        "\n"
        "行为准则：\n"
        "1. 修改任何文件之前，先用工具读取该文件的当前内容，避免盲目覆盖。\n"
        "2. 只在工作区内操作，不做超出用户请求范围的破坏性或不可逆的变更。\n"
        "3. 完成任务后，用简洁的中文总结：做了什么、改动了哪些文件、如何验证结果。\n"
        "4. 如果任务无法完成，如实说明原因和遇到的阻碍，不要编造结果或假装成功。\n"
    )
