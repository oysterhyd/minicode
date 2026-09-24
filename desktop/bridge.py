"""NDJSON bridge between Electron and the existing MiniCode runtime."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

from minicode.cli import _attach_compactor
from minicode.context.extensions import ProjectInstructions, SkillCatalog
from minicode.core.catalog import DEFAULT_MODEL, EFFORT_LEVELS, MODEL_CATALOG, lookup_model, parse_effort
from minicode.core.models import ApprovalDecision, ApprovalRequest, Budget
from minicode.goals import AcceptanceSpec, EvidenceLedger, GoalChecker, ProtectedSnapshot
from minicode.plugins import PluginCatalog
from minicode.providers.anthropic_provider import AnthropicProvider
from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeTurn
from minicode.providers.zcode_config import discover_commandcode
from minicode.runtime.loop import AgentRuntime
from minicode.security.policy import ModePolicy, PermissionMode, parse_permission_mode
from minicode.slash import SLASH_COMMANDS, format_command_lines
from minicode.storage import SqliteStore
from minicode.storage.artifacts import ArtifactStore
from minicode.tasks.background import BackgroundManager
from minicode.tasks.taskstore import TaskStore
from minicode.tools.registry import default_registry


def emit(message: dict) -> None:
    print(json.dumps(message, ensure_ascii=False), flush=True)


def provider_for(model: str, effort: str = "off"):
    if model == "fake":
        return FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="离线演示已连接。请选择已配置的模型来执行真实编码任务。")]))
    info = lookup_model(model)
    if info.provider == "anthropic":
        return AnthropicProvider(model=model, max_tokens=info.max_output_tokens or 4096)
    return CommandCodeProvider(model=model, max_tokens=info.max_output_tokens or 8192,
                               reasoning_effort=None if effort == "off" else effort)


def first_prompt(messages) -> str:
    for message in messages:
        if message.role == "user":
            for block in message.content:
                if block.type == "text" and block.text.strip():
                    return block.text.strip().splitlines()[0][:72]
    return "新建任务"


class Bridge:
    def __init__(self) -> None:
        self.store = SqliteStore()
        self.runtime: AgentRuntime | None = None
        self.run_task: asyncio.Task | None = None
        self.approvals: dict[str, asyncio.Future] = {}
        self.approval_seq = 0
        self.policy = ModePolicy()
        self.model = DEFAULT_MODEL if discover_commandcode() else "claude-sonnet-4-5" if os.environ.get("ANTHROPIC_API_KEY") else "fake"
        self.effort = "off"
        self.budget = Budget()
        self.acceptance: Path | None = None
        self.plugins: PluginCatalog | None = None
        self.skills: SkillCatalog | None = None
        self.registry = None
        self.artifacts: ArtifactStore | None = None

    def sessions(self):
        return [summary.model_dump() | {"title": first_prompt(self.store.get_messages(summary.session_id))}
                for summary in self.store.list_sessions()]

    async def create_runtime(self, workspace: Path, model: str, session_id: str | None):
        provider = provider_for(model, self.effort)
        plugins = PluginCatalog(workspace)
        skills = SkillCatalog(workspace, plugin_roots=plugins.skill_roots())
        registry = default_registry(skills=skills, delegation=True, tasks=True, memory=True,
                                    agent_kinds=plugins.agent_names())
        registry.plugin_catalog = plugins
        for server in plugins.servers():
            registry.add_mcp_server(server)
        artifacts = ArtifactStore(self.store)
        goal_checker = None
        evidence_ledger = None
        if self.acceptance is not None:
            spec = AcceptanceSpec.from_yaml(self.acceptance)
            protected_paths = [item.path for item in spec.items if item.type == "protected"]
            goal_checker = GoalChecker(spec, workspace, protected_snapshot=ProtectedSnapshot(workspace, protected_paths))
            evidence_ledger = EvidenceLedger()

        async def on_text_delta(delta: str):
            emit({"event": "text_delta", "sessionId": self.runtime.session_id, "text": delta})

        async def on_event(event):
            emit({"event": "agent_event", "sessionId": self.runtime.session_id, "item": event.model_dump(mode="json")})

        async def on_approval(request: ApprovalRequest) -> ApprovalDecision:
            self.approval_seq += 1
            approval_id = str(self.approval_seq)
            future = asyncio.get_running_loop().create_future()
            self.approvals[approval_id] = future
            emit({"event": "approval", "sessionId": self.runtime.session_id,
                  "approvalId": approval_id, "request": request.model_dump()})
            try:
                granted = await future
            finally:
                self.approvals.pop(approval_id, None)
            return ApprovalDecision(granted=granted)

        arguments = dict(
            provider=provider,
            registry=registry,
            store=self.store,
            policy=self.policy,
            workspace=workspace,
            provider_name="fake" if model == "fake" else lookup_model(model).provider,
            model=model,
            budget=self.budget,
            approval_handler=on_approval,
            on_text_delta=on_text_delta,
            on_event=on_event,
            artifact_store=artifacts,
            background_manager=BackgroundManager(),
            goal_checker=goal_checker,
            evidence_ledger=evidence_ledger,
            project_instructions=ProjectInstructions(workspace),
            skills=skills,
        )
        if session_id:
            self.runtime = AgentRuntime.resume(session_id=session_id, **arguments)
        else:
            self.runtime = AgentRuntime(**arguments)
        _attach_compactor(self.runtime, artifacts)
        self.model = model
        self.plugins = plugins
        self.skills = skills
        self.registry = registry
        self.artifacts = artifacts

    async def run_prompt(self, text: str | None):
        try:
            result = await (self.runtime.run_turn(text) if text is not None else self.runtime.continue_turn())
            emit({"event": "run_done", "sessionId": self.runtime.session_id,
                  "result": result.model_dump(mode="json")})
        except asyncio.CancelledError:
            emit({"event": "run_done", "sessionId": self.runtime.session_id,
                  "result": {"exit_reason": "cancelled"}})
        except Exception as exc:
            emit({"event": "run_error", "sessionId": self.runtime.session_id, "error": str(exc)})
        finally:
            await self.registry.aclose()

    def state(self):
        runtime = self.runtime
        return {"model": runtime.model if runtime else self.model,
                "effort": getattr(runtime.provider, "reasoning_effort", None) or "off" if runtime else self.effort,
                "permissionMode": self.policy.mode.value,
                "sessionId": runtime.session_id if runtime else None,
                "taskPending": runtime.task_pending if runtime else False,
                "rounds": runtime.rounds if runtime else 0,
                "contextTokens": runtime.context_tokens_used() if runtime else 0,
                "contextWindow": runtime.context_window if runtime else lookup_model(self.model).context_window,
                "usage": runtime.usage.model_dump() if runtime else None,
                "budget": (runtime._budget if runtime else self.budget).model_dump(),
                "acceptance": str(self.acceptance) if self.acceptance else ""}

    def capabilities(self, workspace: Path):
        plugins = PluginCatalog(workspace, check_lock=False)
        skills = SkillCatalog(workspace, plugin_roots=plugins.skill_roots())
        active = self.runtime._active_skills if self.runtime and self.runtime.workspace == workspace else {}
        return {"skills": [{"name": s.name, "description": s.description, "origin": s.origin,
                            "active": s.name in active} for s in skills.skills.values()],
                "plugins": [{"name": p.name, "version": p.version, "enabled": p.enabled,
                             "digest": p.digest[:12], "path": str(p.path.parent),
                             "skills": bool(p.skill_root), "agents": bool(p.agent_root),
                             "servers": [s.name for s in p.servers]} for p in plugins.plugins.values()],
                "mcp": [{"name": s.name, "plugin": s.plugin} for s in plugins.servers()],
                "agents": ["explore", "review", *plugins.agent_names()]}

    async def ensure_runtime(self, workspace: Path, session_id: str | None, model: str | None = None):
        chosen = model or (self.store.get_session(session_id).model if session_id else self.model)
        if self.runtime is None or self.runtime.session_id != session_id or self.runtime.workspace != workspace:
            await self.create_runtime(workspace, chosen, session_id)

    async def change_model(self, model: str):
        if self.run_task and not self.run_task.done():
            raise ValueError("当前回合结束后才能切换模型")
        if model != "fake" and model not in MODEL_CATALOG:
            raise ValueError(f"未知模型：{model}")
        provider = provider_for(model, self.effort)
        if self.runtime:
            self.runtime.set_model(provider=provider,
                                   provider_name="fake" if model == "fake" else lookup_model(model).provider,
                                   model=model)
        self.model = model
        return self.state()

    async def execute_slash(self, text: str, workspace: Path | None, session_id: str | None):
        verb, _, raw_arg = text.partition(" ")
        verb, arg = verb.lower(), raw_arg.strip()
        if verb in {"/help", "/?"}:
            return {"message": "可用命令：\n" + "\n".join(format_command_lines())}
        if verb in {"/exit", "/quit"}:
            return {"action": "exit"}
        if verb == "/clear":
            return {"action": "clear", "message": "已清屏，会话上下文保留。"}
        if verb == "/new":
            self.runtime = None
            return {"action": "new", "message": "已开启新会话。"}
        if verb == "/sessions":
            return {"message": "\n".join(f"{s['session_id'][:8]} · {s['title']} · {s['status']}" for s in self.sessions()[:20]) or "暂无会话。"}
        if verb == "/resume":
            if not arg:
                return {"action": "sessions"}
            matches = [s for s in self.sessions() if s["session_id"].startswith(arg)]
            if len(matches) != 1:
                raise ValueError("会话 ID 不唯一或不存在")
            return {"action": "resume", "sessionId": matches[0]["session_id"]}
        if workspace is None:
            raise ValueError("请先选择工作区")
        await self.ensure_runtime(workspace, session_id)
        if verb == "/model":
            return {"action": "model"} if not arg else {"state": await self.change_model(arg), "message": f"Model: {arg}"}
        if verb == "/effort":
            if not arg:
                return {"action": "effort", "state": self.state()}
            level = parse_effort(arg)
            if level is None:
                raise ValueError(f"可选档位：{', '.join(EFFORT_LEVELS)}")
            if not hasattr(self.runtime.provider, "reasoning_effort"):
                raise ValueError("当前模型不支持 reasoning effort")
            self.effort = level
            self.runtime.provider.reasoning_effort = None if level == "off" else level
            return {"state": self.state(), "message": f"Reasoning effort: {level}"}
        if verb == "/permissions":
            if not arg:
                return {"action": "permissions", "state": self.state()}
            mode = parse_permission_mode(arg)
            if mode is None:
                raise ValueError("可选模式：default, accept_edits, bypass")
            self.policy.set_mode(mode)
            return {"state": self.state(), "message": f"Permissions: {mode.value}"}
        if verb == "/skill":
            if not arg:
                return {"action": "skills"}
            message = self.runtime.deactivate_skill(arg[4:].strip()) if arg.startswith("off ") else self.runtime.activate_skill(arg)
            return {"message": message.split("\n", 1)[0]}
        if verb == "/continue":
            if not self.runtime.task_pending:
                raise ValueError("当前没有暂停的任务")
            if self.run_task and not self.run_task.done():
                raise ValueError("当前任务仍在运行")
            self.run_task = asyncio.create_task(self.run_prompt(None))
            return {"action": "continue", "state": self.state()}
        if verb == "/compact":
            compactor = self.runtime._compactor
            if self.runtime.session_id is None:
                raise ValueError("会话尚未开始")
            if not compactor.needs_compaction(self.runtime._system_prompt, self.runtime._messages, self.registry.specs()):
                return {"message": "上下文未超过阈值，无需压缩。"}
            result = compactor.compact(self.runtime._system_prompt, self.runtime._messages, self.registry.specs())
            if result.changed:
                self.runtime._messages = result.messages
                self.store.replace_messages(self.runtime.session_id, result.messages)
            return {"message": "上下文已压缩。" if result.changed else "没有可压缩的内容。", "state": self.state()}
        raise ValueError(f"未知命令：{verb}")

    async def handle(self, method: str, params: dict):
        if method == "initialize":
            commandcode = discover_commandcode() is not None
            anthropic = bool(os.environ.get("ANTHROPIC_API_KEY"))
            return {"sessions": self.sessions(), "models": [
                {"id": info.name, "provider": info.provider,
                 "available": commandcode if info.provider == "commandcode" else anthropic}
                for info in MODEL_CATALOG.values()] + [{"id": "fake", "provider": "offline", "available": True}],
                "defaultModel": self.model,
                "commands": [{"name": cmd.name, "usage": cmd.usage, "summary": cmd.summary} for cmd in SLASH_COMMANDS],
                "state": self.state()}
        if method == "listSessions":
            return self.sessions()
        if method == "listTasks":
            session_id = params.get("sessionId")
            return [task.model_dump() for task in TaskStore(self.store).list_tasks(session_id)] if session_id else []
        if method == "getSession":
            session_id = params["sessionId"]
            summary = self.store.get_session(session_id)
            if summary is None:
                raise ValueError("找不到会话")
            return {"summary": summary.model_dump(),
                    "messages": [message.model_dump() for message in self.store.get_messages(session_id)],
                    "events": [event.model_dump(mode="json") for event in self.store.get_events(session_id)]}
        if method == "readArtifact":
            result = ArtifactStore(self.store).read_page(params["sessionId"], params["artifactId"],
                                                         int(params.get("offset", 0)), 20_000)
            if result is None:
                raise ValueError("归档内容不可用")
            page, total, has_more = result
            return {"text": page, "total": total, "hasMore": has_more}
        if method == "selectSession":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前任务仍在运行")
            summary = self.store.get_session(params["sessionId"])
            if summary is None:
                raise ValueError("找不到会话")
            await self.ensure_runtime(Path(summary.workspace).resolve(), summary.session_id, summary.model)
            return self.state()
        if method == "getState":
            return self.state()
        if method == "resetSession":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前任务仍在运行")
            self.runtime = None
            return self.state()
        if method == "getCapabilities":
            return self.capabilities(Path(params["workspace"]).resolve())
        if method == "setModel":
            return await self.change_model(params["model"])
        if method == "setEffort":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能调整推理预算")
            level = parse_effort(params["effort"])
            if level is None:
                raise ValueError("无效 reasoning effort")
            if self.runtime:
                if not hasattr(self.runtime.provider, "reasoning_effort"):
                    raise ValueError("当前模型不支持 reasoning effort")
                self.runtime.provider.reasoning_effort = None if level == "off" else level
            self.effort = level
            return self.state()
        if method == "setPermissionMode":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能切换权限模式")
            mode = parse_permission_mode(params["mode"])
            if mode is None:
                raise ValueError("无效权限模式")
            self.policy.set_mode(mode)
            return self.state()
        if method == "setBudget":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能调整预算")
            self.budget = Budget(max_rounds=int(params["max_rounds"]),
                                 max_total_tokens=int(params["max_total_tokens"]),
                                 max_seconds=float(params["max_seconds"]))
            if self.runtime:
                self.runtime._budget = self.budget
            return self.state()
        if method == "setAcceptance":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能调整验收文件")
            if self.runtime and self.runtime.session_id:
                raise ValueError("请先使用 /new 开始新会话，再设置验收文件")
            chosen = params.get("path", "").strip()
            path = Path(chosen).resolve() if chosen else None
            if path:
                AcceptanceSpec.from_yaml(path)
            self.acceptance = path
            self.runtime = None
            return self.state()
        if method == "setSkillActive":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能管理 Skills")
            workspace = Path(params["workspace"]).resolve()
            await self.ensure_runtime(workspace, params.get("sessionId"))
            return self.runtime.activate_skill(params["name"]).split("\n", 1)[0] if params["active"] else self.runtime.deactivate_skill(params["name"])
        if method == "setPluginEnabled":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能修改插件")
            workspace = Path(params["workspace"]).resolve()
            catalog = PluginCatalog(workspace, check_lock=False)
            plugin = catalog.plugins[params["name"]]
            manifest = json.loads(plugin.path.read_text(encoding="utf-8"))
            manifest["enabled"] = bool(params["enabled"])
            plugin.path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            PluginCatalog(workspace, check_lock=False).write_lock()
            self.runtime = None
            return self.capabilities(workspace)
        if method == "lockPlugins":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能更新插件锁")
            workspace = Path(params["workspace"]).resolve()
            return str(PluginCatalog(workspace, check_lock=False).write_lock())
        if method == "refreshMcp":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前回合结束后才能重新发现 MCP")
            workspace = Path(params["workspace"]).resolve()
            catalog = PluginCatalog(workspace)
            registry = default_registry()
            for server in catalog.servers():
                registry.add_mcp_server(server)
            try:
                servers = await registry.prepare()
                return {"servers": servers, "tools": [s.name for s in registry.specs() if s.name.startswith("mcp__")]}
            finally:
                await registry.aclose()
        if method == "runSlash":
            root = Path(params["workspace"]).resolve() if params.get("workspace") else None
            return await self.execute_slash(params["text"], root, params.get("sessionId"))
        if method == "cancelTurn":
            if self.run_task and not self.run_task.done():
                self.run_task.cancel()
            return True
        if method == "sendPrompt":
            if self.run_task and not self.run_task.done():
                raise ValueError("当前任务仍在运行")
            text = params["text"].strip()
            if not text:
                raise ValueError("请输入任务内容")
            session_id = params.get("sessionId")
            workspace = Path(params["workspace"]).resolve()
            if not workspace.is_dir():
                raise ValueError("工作区不存在")
            if session_id:
                saved = self.store.get_session(session_id)
                if saved is None or Path(saved.workspace).resolve() != workspace:
                    raise ValueError("会话不属于当前工作区")
            model = params.get("model") or self.model
            await self.ensure_runtime(workspace, session_id, model)
            if self.runtime.model != model:
                await self.change_model(model)
            self.run_task = asyncio.create_task(self.run_prompt(text))
            await asyncio.sleep(0)
            return {"sessionId": self.runtime.session_id}
        if method == "resolveApproval":
            self.approvals[params["approvalId"]].set_result(bool(params["granted"]))
            return True
        raise ValueError(f"未知请求：{method}")


async def main():
    bridge = Bridge()
    while line := await asyncio.to_thread(sys.stdin.readline):
        request = json.loads(line)
        try:
            result = await bridge.handle(request["method"], request.get("params", {}))
            emit({"id": request["id"], "result": result})
        except Exception as exc:
            emit({"id": request["id"], "error": str(exc)})


if __name__ == "__main__":
    asyncio.run(main())
