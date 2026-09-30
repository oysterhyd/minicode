"""NDJSON bridge between Electron and the existing MiniCode runtime."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
import sys
import httpx
from pathlib import Path
from minicode.configuration import HarnessConfiguration

from minicode.runtime.services import attach_compactor, workspace_services
from minicode.context.extensions import ProjectInstructions, SkillCatalog
from minicode.core.catalog import DEFAULT_MODEL, EFFORT_LEVELS, MODEL_CATALOG, lookup_model, parse_effort
from minicode.core.models import ApprovalDecision, ApprovalRequest, Budget
from minicode.goals import AcceptanceSpec, EvidenceLedger, GoalChecker, ProtectedSnapshot
from minicode.plugins import PluginCatalog
from minicode.providers.anthropic_provider import AnthropicProvider
from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.fake import FakeProvider
from minicode.providers.zcode_config import discover_commandcode
from minicode.runtime.loop import AgentRuntime
from minicode.runtime.protocol import RequestServer
from minicode.runtime.prompt import build_system_prompt
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
        # Keep old demo transcripts readable. This adapter is never listed in settings.
        return FakeProvider()
    info = lookup_model(model)
    if info.provider == "anthropic":
        return AnthropicProvider(model=model, max_tokens=info.max_output_tokens or 4096)
    return CommandCodeProvider(model=model, max_tokens=info.max_output_tokens or 8192,
                               reasoning_effort=None if effort == "off" else effort)


def session_list(store: SqliteStore) -> list[dict]:
    """Sidebar rows: stored summaries plus title, pin and last activity."""
    titles = store.first_user_texts()
    meta = store.session_meta()
    activity = store.last_activity()
    rows = []
    for summary in store.list_sessions():
        info = meta.get(summary.session_id, {})
        custom = info.get("title")
        rows.append(summary.model_dump() | {
            "title": custom or titles.get(summary.session_id, "新建任务"),
            "custom_title": bool(custom),
            "pinned": bool(info.get("pinned")),
            "updated_at": activity.get(summary.session_id, summary.created_at),
        })
    return rows


class Bridge:
    def __init__(self, store: SqliteStore | None = None) -> None:
        self.store = store if store is not None else SqliteStore()
        self.client_key: str | None = None
        #: Client key that owns the in-flight turn. Events keep this tag until
        #: the run ends, so a request arriving from another key (or from a
        #: handler that forgot to send one) cannot re-label a live conversation's
        #: stream and send it to the wrong view.
        self.event_key: str | None = None
        self.runtime: AgentRuntime | None = None
        self.run_task: asyncio.Task | None = None
        self.approvals: dict[str, asyncio.Future] = {}
        #: Tool name behind each pending approval, for "always allow" answers.
        self.approval_tools: dict[str, str] = {}
        #: Tools the user allowed for the rest of this conversation. Never
        #: copied to another conversation and cleared when a new one starts.
        self.always_allow: set[str] = set()
        self.approval_seq = 0
        self.policy = ModePolicy()
        self.configuration = HarnessConfiguration()
        configured = self.configuration.models()
        self.model = self.configuration.read().get("defaultModel") or next((m["id"] for m in configured if m["available"]), configured[0]["id"] if configured else "")
        self.effort = "off"
        self.budget = Budget()
        self.acceptance: Path | None = None
        self.registry = None
        self.pending_model = False
        self.registry_dirty = False

    def make_provider(self, model):
        from minicode.providers.factory import configured_provider
        return configured_provider(model, configuration=self.configuration,
                                   effort=self.effort, fallback=provider_for)

    async def apply_pending_model(self):
        if not self.pending_model:
            return
        provider = self.make_provider(self.model)
        previous = self.runtime.provider
        self.runtime.set_model(provider=provider, provider_name=provider.name, model=self.model)
        self.pending_model = False
        await self.discard_provider(previous)

    async def before_round(self):
        await self.apply_pending_model()
        self.registry.set_agents(self.configuration.agents(), self.registry.plugin_catalog.agent_names())
        self.runtime.refresh_tools()

    def statistics(self):
        events = self.store.get_events(self.runtime.session_id) if self.runtime and self.runtime.session_id else []
        samples = []
        for event in events:
            if event.type.value != "assistant_message":
                continue
            usage = event.data.get("usage", {})
            seconds = event.data.get("request_seconds")
            samples.append({"round": len(samples) + 1, "input": usage.get("input_tokens", 0), "output": usage.get("output_tokens", 0),
                            "cached": usage.get("cache_read_tokens", 0), "available": usage.get("available", True),
                            "seconds": seconds, "tps": usage.get("output_tokens", 0) / seconds if seconds and usage.get("available", True) else None})
        measured = [s for s in samples if s["seconds"] and s["available"]]
        seconds = sum(s["seconds"] for s in measured)
        return {"toolCalls": sum(e.type.value == "tool_call_start" for e in events), "requests": len(samples),
                "modelSeconds": seconds, "tps": sum(s["output"] for s in measured) / seconds if seconds else None,
                "lastTps": samples[-1]["tps"] if samples else None, "samples": samples[-32:]}

    def emit(self, message: dict) -> None:
        emit(message | {"clientKey": self.event_key or self.client_key})

    def busy(self) -> bool:
        """Whether a turn is currently in flight for this conversation."""
        return bool(self.run_task and not self.run_task.done())

    def sessions(self):
        return session_list(self.store)

    async def discard_runtime(self) -> None:
        previous = self.runtime
        if previous is None:
            return
        self.runtime = None
        registry = self.registry
        self.registry = None
        await previous.aclose()

    async def discard_provider(self, provider: object | None = None) -> None:
        """Close a provider's HTTP client, if it has one.

        ``discard_runtime`` clears the runtime first, so the eviction path passes
        the provider explicitly instead of relying on the runtime still being set.
        """
        close = getattr(provider, "aclose", None)
        if close is not None:
            await close()

    def build_registry(self, workspace: Path):
        bundle = workspace_services(workspace, agents=self.configuration.agents())
        return bundle.registry, bundle.skills

    async def create_runtime(self, workspace: Path, model: str, session_id: str | None):
        await self.discard_runtime()
        provider = self.make_provider(model)
        registry, skills = self.build_registry(workspace)
        artifacts = ArtifactStore(self.store)
        goal_checker = None
        evidence_ledger = None
        if self.acceptance is not None and session_id is None:
            spec = AcceptanceSpec.from_yaml(self.acceptance)
            protected_paths = [item.path for item in spec.items if item.type == "protected"]
            goal_checker = GoalChecker(spec, workspace, protected_snapshot=ProtectedSnapshot(workspace, protected_paths))
            evidence_ledger = EvidenceLedger()

        async def on_text_delta(delta: str):
            self.emit({"event": "text_delta", "sessionId": self.runtime.session_id, "text": delta})

        async def on_event(event):
            self.emit({"event": "agent_event", "sessionId": self.runtime.session_id, "item": event.model_dump(mode="json")})

        async def on_approval(request: ApprovalRequest) -> ApprovalDecision:
            if request.tool_name in self.always_allow:
                self.emit({"event": "approval_auto", "sessionId": self.runtime.session_id,
                           "request": request.model_dump()})
                return ApprovalDecision(granted=True)
            self.approval_seq += 1
            approval_id = f"{self.runtime.session_id}:{self.approval_seq}"
            future = asyncio.get_running_loop().create_future()
            self.approvals[approval_id] = future
            self.approval_tools[approval_id] = request.tool_name
            self.emit({"event": "approval", "sessionId": self.runtime.session_id,
                  "approvalId": approval_id, "request": request.model_dump()})
            try:
                granted = await future
            finally:
                self.approvals.pop(approval_id, None)
                self.approval_tools.pop(approval_id, None)
            return ApprovalDecision(granted=granted)

        arguments = dict(
            provider=provider,
            registry=registry,
            store=self.store,
            policy=self.policy,
            workspace=workspace,
            provider_name=provider.name,
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
        attach_compactor(self.runtime, artifacts)
        self.model = model
        self.registry = registry
        self.runtime.before_round = self.before_round
        self.registry_dirty = False
        self.pending_model = False

    async def run_prompt(self, text: str | None):
        message = None
        try:
            result = await (self.runtime.run_turn(text) if text is not None else self.runtime.continue_turn())
            message = {"event": "run_done", "sessionId": self.runtime.session_id,
                       "result": result.model_dump(mode="json")}
        except asyncio.CancelledError:
            message = {"event": "run_done", "sessionId": self.active_session_id(),
                       "result": {"exit_reason": "cancelled"}}
        except Exception as exc:
            message = {"event": "run_error", "sessionId": self.active_session_id(), "error": str(exc)}
        finally:
            if message is None:
                message = {"event": "run_error", "sessionId": self.active_session_id(),
                           "error": "运行结束但没有产生结果"}
            self.emit(message)
            self.event_key = None

    def active_session_id(self) -> str | None:
        """The active session id, safe to read after a runtime teardown."""
        return self.runtime.session_id if self.runtime is not None else None

    async def _aclose_registry(self) -> str | None:
        """Close the tool registry, returning an error message instead of raising.

        Used by :meth:`run_prompt`, whose ``finally`` must always reach its
        ``emit`` even when the turn was cancelled.
        """
        registry = self.registry
        if registry is None:
            return None
        try:
            await registry.aclose()
        except Exception as exc:  # noqa: BLE001 - reported, never fatal
            return f"清理工具连接失败：{exc}"
        return None

    def state(self):
        runtime = self.runtime
        snapshot = runtime.snapshot() if runtime else None
        breakdown = snapshot.context_breakdown if snapshot else {"system": 0, "tools": 0, "messages": 0}
        return {"model": self.model,
                "protocolVersion": 2,
                "runId": snapshot.run_id if snapshot else None,
                "phase": snapshot.phase.value if snapshot else "idle",
                "running": snapshot.running if snapshot else False,
                "activeModel": runtime.model if runtime else self.model,
                "pendingSettings": self.pending_model or self.registry_dirty,
                "effort": self.effort,
                "permissionMode": self.policy.mode.value,
                "sessionId": runtime.session_id if runtime else None,
                "taskPending": runtime.task_pending if runtime else False,
                "rounds": runtime.rounds if runtime else 0,
                "contextTokens": sum(breakdown.values()),
                "contextBreakdown": breakdown,
                "contextWindow": runtime.context_window if runtime else lookup_model(self.model).context_window,
                "usage": runtime.usage.model_dump() if runtime else None,
                "statistics": self.statistics(),
                "budget": self.budget.model_dump(),
                "acceptance": str(self.acceptance) if self.acceptance else "",
                "alwaysAllow": sorted(self.always_allow)}

    def capabilities(self, workspace: Path):
        plugins = PluginCatalog(workspace, check_lock=False)
        skills = SkillCatalog(workspace, plugin_roots=plugins.skill_roots())
        active = self.runtime.active_skills if self.runtime and self.runtime.workspace == workspace else {}
        return {"skills": [{"name": s.name, "description": s.description, "origin": s.origin,
                            "active": s.name in active} for s in skills.skills.values()],
                "plugins": [{"name": p.name, "version": p.version, "enabled": p.enabled,
                             "digest": p.digest[:12], "path": str(p.path.parent),
                             "skills": bool(p.skill_root), "agents": bool(p.agent_root),
                             "servers": [s.name for s in p.servers]} for p in plugins.plugins.values()],
                "mcp": [{"name": s.name, "plugin": s.plugin} for s in plugins.servers()],
                "agents": [a["name"] for a in self.configuration.agents() if a["enabled"]] + plugins.agent_names(),
                "agentDefinitions": self.configuration.agents()}

    async def ensure_runtime(self, workspace: Path, session_id: str | None, model: str | None = None):
        chosen = model or (self.store.get_session(session_id).model if session_id else self.model)
        if self.runtime is None or self.runtime.session_id != session_id or self.runtime.workspace != workspace:
            if self.runtime is not None and self.runtime.session_id not in (None, session_id):
                # "Always allow" belongs to one conversation, not to this Bridge.
                self.always_allow.clear()
            await self.create_runtime(workspace, chosen, session_id)
        elif self.registry_dirty and not self.busy():
            registry, skills = self.build_registry(workspace)
            await self.runtime.configure_extensions(registry, skills)
            self.registry = registry
            self.registry_dirty = False

    async def change_model(self, model: str):
        if model != "fake" and model not in MODEL_CATALOG and self.configuration.find_model(model) is None:
            raise ValueError(f"未知模型：{model}")
        provider = self.make_provider(model)
        self.model = model
        if self.busy():
            await self.discard_provider(provider)
            self.pending_model = True
            return self.state()
        if self.runtime:
            previous = self.runtime.provider
            self.runtime.set_model(provider=provider,
                                   provider_name=provider.name,
                                   model=model)
            close = getattr(previous, "aclose", None)
            if close is not None:
                await close()
        else:
            await self.discard_provider(provider)
        self.model = model
        self.pending_model = False
        return self.state()

    async def execute_slash(self, text: str, workspace: Path | None, session_id: str | None):
        verb, _, raw_arg = text.partition(" ")
        verb, arg = verb.lower(), raw_arg.strip()
        if self.busy() and verb not in {"/help", "/?", "/model", "/effort", "/permissions", "/skill"}:
            raise ValueError("当前回合结束后才能执行命令")
        if verb in {"/help", "/?"}:
            return {"message": "可用命令：\n" + "\n".join(format_command_lines())}
        if verb in {"/exit", "/quit"}:
            return {"action": "exit"}
        if verb == "/clear":
            return {"action": "clear", "message": "已清屏，会话上下文保留。"}
        if verb == "/new":
            await self.discard_runtime()
            self.always_allow.clear()
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
            await self.handle("setEffort", {"effort": level})
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
            if self.busy():
                raise ValueError("当前任务仍在运行")
            self.event_key = self.client_key
            self.runtime.set_budget(self.budget)
            self.run_task = asyncio.create_task(self.run_prompt(None))
            return {"action": "continue", "state": self.state()}
        if verb == "/compact":
            changed = await self.runtime.compact_context()
            return {"message": "上下文已压缩。" if changed else "上下文未超过阈值，无需压缩。", "state": self.state()}
        raise ValueError(f"未知命令：{verb}")

    async def handle(self, method: str, params: dict):
        boolean_fields = {"resolveApproval": "granted", "setPluginEnabled": "enabled",
                          "setAgentEnabled": "enabled", "setSkillActive": "active"}
        field = boolean_fields.get(method)
        if field is not None and not isinstance(params.get(field), bool):
            raise ValueError(f"{field} must be a boolean")
        if method == "getConfiguration":
            return self.configuration.public()
        if method == "listModels":
            return self.configuration.models()
        if method == "saveService":
            return self.configuration.save_service(params["service"])
        if method == "deleteService":
            data = self.configuration.read()
            data["services"] = [s for s in data["services"] if s["id"] != params["id"]]
            if not any(self.configuration.model_key(s, m) == data.get("defaultModel") for s in data["services"] for m in s["models"]):
                data["defaultModel"] = ""
            self.configuration.write(data)
            return self.configuration.public()
        if method == "setDefaultModel":
            if params["model"] and not any(m["id"] == params["model"] and m["available"] for m in self.configuration.models()):
                raise ValueError("请先配置并启用该模型的服务")
            data = self.configuration.read()
            data["defaultModel"] = params["model"]
            self.configuration.write(data)
            return self.configuration.public()
        if method in {"fetchServiceModels", "testServiceConnection"}:
            draft = params["service"]
            existing = next((s for s in self.configuration.read()["services"] if s["id"] == draft.get("id")), {})
            service = {**existing, **draft, "apiKey": draft.get("apiKey") or existing.get("apiKey", "")}
            url, key = self.configuration.credentials(service)
            if not key:
                raise ValueError("请填写 API 密钥")
            from urllib.parse import urlparse
            if urlparse(url).scheme not in {"http", "https"}:
                raise ValueError("接口地址必须是 HTTP(S) URL")
            headers = {"x-api-key": key, "anthropic-version": "2023-06-01"} if service["apiStyle"] == "anthropic" else {"Authorization": f"Bearer {key}"}
            endpoint = url.rstrip("/") + ("/v1/models" if service["apiStyle"] == "anthropic" and not url.rstrip("/").endswith("/v1") else "/models")
            async with httpx.AsyncClient(timeout=20) as client:
                try:
                    response = await client.get(endpoint, headers=headers)
                    response.raise_for_status()
                    payload = response.json()
                except (httpx.HTTPError, ValueError) as exc:
                    status = getattr(getattr(exc, "response", None), "status_code", None)
                    raise ValueError(f"连接失败（HTTP {status}）" if status else "连接失败，请检查地址、网络和接口格式") from None
            models = [dict(modelId=m["id"], name=m.get("display_name") or m["id"], contextWindow=200000, maxOutputTokens=8192, supportsEffort=False)
                      for m in payload.get("data", []) if isinstance(m, dict) and isinstance(m.get("id"), str)]
            return {"models": models, "message": f"已连接，发现 {len(models)} 个模型"}
        if method == "saveAgent":
            return self.configuration.save_agent(params["agent"])
        if method == "setAgentEnabled":
            data = self.configuration.read()
            name = params["name"]
            if name not in {a["name"] for a in self.configuration.agents()}:
                raise ValueError("找不到子助手")
            disabled = set(data.get("disabledAgents", []))
            disabled.discard(name) if params["enabled"] else disabled.add(name)
            data["disabledAgents"] = sorted(disabled)
            self.configuration.write(data)
            return self.configuration.agents()
        if method == "deleteAgent":
            data = self.configuration.read()
            data["agents"] = [a for a in data["agents"] if a["name"] != params["name"]]
            data["disabledAgents"] = [n for n in data.get("disabledAgents", []) if n != params["name"]]
            self.configuration.write(data)
            return self.configuration.agents()
        if method == "initialize":
            return {"sessions": self.sessions(), "models": self.configuration.models(),
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
            if self.busy():
                if self.runtime and self.runtime.session_id == params["sessionId"]:
                    return self.state()
                raise ValueError("不能替换正在运行的会话")
            summary = self.store.get_session(params["sessionId"])
            if summary is None:
                raise ValueError("找不到会话")
            await self.ensure_runtime(Path(summary.workspace).resolve(), summary.session_id, summary.model)
            return self.state()
        if method == "getState":
            session_id = params.get("sessionId")
            if session_id and (self.runtime is None or self.runtime.session_id != session_id):
                if self.busy():
                    raise ValueError("当前任务仍在运行")
                summary = self.store.get_session(session_id)
                if summary is None:
                    raise ValueError("找不到会话")
                await self.ensure_runtime(Path(summary.workspace).resolve(), session_id, summary.model)
            return self.state()
        if method == "resetSession":
            if self.busy():
                raise ValueError("当前任务仍在运行")
            await self.discard_runtime()
            self.always_allow.clear()
            return self.state()
        if method == "getCapabilities":
            return self.capabilities(Path(params["workspace"]).resolve())
        if method == "setModel":
            return await self.change_model(params["model"])
        if method == "setEffort":
            level = parse_effort(params["effort"])
            if level is None:
                raise ValueError("无效 reasoning effort")
            if self.runtime and not self.busy():
                if not hasattr(self.runtime.provider, "reasoning_effort"):
                    raise ValueError("当前模型不支持 reasoning effort")
                self.runtime.provider.reasoning_effort = None if level == "off" else level
            self.effort = level
            if self.busy():
                self.pending_model = True
            return self.state()
        if method == "setPermissionMode":
            mode = parse_permission_mode(params["mode"])
            if mode is None:
                raise ValueError("无效权限模式")
            self.policy.set_mode(mode)
            return self.state()
        if method == "setBudget":
            self.budget = Budget(max_rounds=int(params["max_rounds"]),
                                 max_total_tokens=int(params["max_total_tokens"]),
                                 max_seconds=float(params["max_seconds"]))
            if self.runtime and not self.busy():
                self.runtime.set_budget(self.budget)
            return self.state()
        if method == "setAcceptance":
            chosen = params.get("path", "").strip()
            path = Path(chosen).resolve() if chosen else None
            if path:
                AcceptanceSpec.from_yaml(path)
            self.acceptance = path
            if not self.busy() and not (self.runtime and self.runtime.session_id):
                await self.discard_runtime()
            return self.state()
        if method == "setSkillActive":
            workspace = Path(params["workspace"]).resolve()
            await self.ensure_runtime(workspace, params.get("sessionId"))
            return self.runtime.activate_skill(params["name"]).split("\n", 1)[0] if params["active"] else self.runtime.deactivate_skill(params["name"])
        if method == "setPluginEnabled":
            workspace = Path(params["workspace"]).resolve()
            catalog = PluginCatalog(workspace, check_lock=False)
            catalog.set_enabled(params["name"], bool(params["enabled"]))
            self.registry_dirty = True
            return self.capabilities(workspace)
        if method == "lockPlugins":
            workspace = Path(params["workspace"]).resolve()
            return str(PluginCatalog(workspace, check_lock=False).write_lock())
        if method == "refreshMcp":
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
            if self.busy():
                self.run_task.cancel()
            return True
        if method == "sendPrompt":
            if self.busy():
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
            if model != "fake" and not any(m["id"] == model and m["available"] for m in self.configuration.models()):
                raise ValueError("请在设置中配置、启用并选择一个可用的 AI 模型")
            await self.ensure_runtime(workspace, session_id, model)
            if self.runtime.model != model:
                await self.change_model(model)
            self.runtime.set_budget(self.budget)
            self.event_key = self.client_key
            self.run_task = asyncio.create_task(self.run_prompt(text))
            await asyncio.sleep(0)
            return {"sessionId": self.runtime.session_id}
        if method == "resolveApproval":
            approval_id = params["approvalId"]
            future = self.approvals.get(approval_id)
            if future is None or future.done():
                return False
            granted = bool(params["granted"])
            if granted and params.get("remember") == "session" and approval_id in self.approval_tools:
                self.always_allow.add(self.approval_tools[approval_id])
            future.set_result(granted)
            return True
        if method == "clearAlwaysAllow":
            self.always_allow.clear()
            return self.state()
        raise ValueError(f"未知请求：{method}")


class BridgeRouter:
    """Keep each conversation's runtime alive independently of the selected UI."""

    #: How many idle conversations keep a live runtime. Each one owns a
    #: provider, a tool registry and possibly MCP child processes, so they are
    #: released when a conversation has not been used for a while.
    IDLE_LIMIT = 6

    def __init__(self, store: SqliteStore | None = None) -> None:
        self.store = store if store is not None else SqliteStore()
        self.clients: dict[str, Bridge] = {}
        self.sessions: dict[str, Bridge] = {}
        #: Insertion-ordered recency of conversations, oldest first.
        self.recent: list[str] = []
        self._routing_lock = asyncio.Lock()

    def _touch(self, session_id: str | None) -> None:
        if session_id and session_id in self.recent:
            self.recent.remove(session_id)
        if session_id:
            self.recent.append(session_id)

    async def _evict_idle(self) -> None:
        """Release the least recently used conversations that are not running.

        A conversation is only released when no other key still points at it and
        nothing is in flight: its runtime owns a provider, a tool registry and
        possibly MCP child processes, which must not stay alive for the whole
        app session. Resuming a released conversation goes through the normal
        ``resume`` path, so nothing is lost from the store.
        """
        while len(self.sessions) > self.IDLE_LIMIT:
            stale = next((key for key in self.recent
                          if key in self.sessions and not self.sessions[key].busy()), None)
            if stale is None:
                break  # All excess contexts are active; retry on a later request.
            self.recent.remove(stale)
            context = self.sessions.pop(stale)
            if any(other is context for other in self.sessions.values()):
                continue  # another conversation still uses this runtime
            for key, value in list(self.clients.items()):
                if value is context:
                    del self.clients[key]
            await context.discard_runtime()

    def _require_stored(self, session_id) -> str:
        if not session_id or self.store.get_session(session_id) is None:
            raise ValueError("找不到会话")
        return session_id

    async def delete_session(self, session_id: str) -> None:
        contexts = set(self.clients.values()) | set(self.sessions.values())
        owners = [c for c in contexts if c.runtime is not None and c.runtime.session_id == session_id]
        if self.sessions.get(session_id) is not None:
            owners.append(self.sessions[session_id])
        if any(owner.busy() for owner in owners):
            raise ValueError("任务运行中，无法删除会话")
        for owner in set(owners):
            await owner.discard_runtime()
            for key, value in list(self.clients.items()):
                if value is owner:
                    del self.clients[key]
            for key, value in list(self.sessions.items()):
                if value is owner:
                    del self.sessions[key]
                    if key in self.recent:
                        self.recent.remove(key)
        self.sessions.pop(session_id, None)
        if session_id in self.recent:
            self.recent.remove(session_id)
        self.store.delete_session(session_id)
        shutil.rmtree(ArtifactStore(self.store)._session_dir(session_id), ignore_errors=True)

    async def handle(self, method: str, params: dict):
        if not isinstance(method, str) or not isinstance(params, dict):
            raise ValueError("method must be a string and params must be an object")
        if method in {"cancelTurn", "resolveApproval"}:
            context = self.sessions.get(params.get("sessionId")) or self.clients.get(params.get("clientKey") or "default")
            if (params.get("sessionId") and context is not None and
                    (context.runtime is None or context.runtime.session_id != params["sessionId"])):
                return False
            return await context.handle(method, params) if context else False
        if method in {"fetchServiceModels", "testServiceConnection", "refreshMcp"}:
            return await Bridge(self.store).handle(method, params)
        async with self._routing_lock:
            return await self._handle(method, params)

    async def _handle(self, method: str, params: dict):
        # Sidebar management works on stored rows and must not create or
        # rebind a conversation's Bridge.
        if method == "renameSession":
            session_id = self._require_stored(params.get("sessionId"))
            title = str(params.get("title") or "").strip()[:120]
            self.store.set_session_title(session_id, title or None)
            return session_list(self.store)
        if method == "pinSession":
            session_id = self._require_stored(params.get("sessionId"))
            self.store.set_session_pinned(session_id, bool(params.get("pinned")))
            return session_list(self.store)
        if method == "deleteSession":
            await self.delete_session(self._require_stored(params.get("sessionId")))
            return session_list(self.store)
        client_key = params.get("clientKey") or "default"
        session_id = params.get("sessionId")
        context = self.sessions.get(session_id) if session_id else None
        if context is not None and context.runtime is not None \
                and context.runtime.session_id != session_id:
            # This conversation's entry outlived the runtime it was created for
            # (the Bridge was reused for another session). Serving the request
            # from here would hand it a foreign run slot and could reject a
            # legitimate switch with 不能替换正在运行的会话, so drop the entry —
            # and any client key still pointing at it, which is equally stale.
            del self.sessions[session_id]
            if session_id in self.recent:
                self.recent.remove(session_id)
            if self.clients.get(client_key) is context:
                del self.clients[client_key]
            context = None
        if context is None:
            context = self.clients.get(client_key)
        if context is None:
            context = Bridge(self.store)
            source = self.clients.get(params.get("sourceClientKey"))
            if source is not None:
                context.model = context.configuration.read().get("defaultModel") or (source.model if any(m["id"] == source.model and m["available"] for m in context.configuration.models()) else context.model)
                context.effort = source.effort
                context.budget = source.budget.model_copy()
                context.acceptance = source.acceptance
                context.policy.set_mode(source.policy.mode)
        # Remember which Bridge serves this UI view key. This must also run when
        # the Bridge was found through its session id: switching views is
        # exactly the case where the new key has to learn its conversation.
        self.clients[client_key] = context
        # A running conversation keeps the key that started its turn, so a
        # stray request (for example an artifact page fetched without a
        # clientKey) cannot retag its event stream into another view.
        if not context.busy():
            context.client_key = client_key
        result = await context.handle(method, params)
        if method in {"setPluginEnabled", "lockPlugins"}:
            root = Path(params["workspace"]).resolve()
            for item in set(self.clients.values()) | set(self.sessions.values()):
                if item.runtime and item.runtime.workspace == root:
                    item.registry_dirty = True
        if method in {"saveService", "deleteService"}:
            changed_id = params.get("id") or params.get("service", {}).get("id")
            for item in set(self.clients.values()) | set(self.sessions.values()):
                selected = item.configuration.find_model(item.model)
                if item.runtime is None and not any(m["id"] == item.model and m["available"] for m in item.configuration.models()):
                    item.model = item.configuration.read().get("defaultModel") or next((m["id"] for m in item.configuration.models() if m["available"]), item.model)
                if selected is not None and selected[0]["id"] == changed_id and selected[0]["enabled"] and item.configuration.credentials(selected[0])[1] and item.runtime:
                    item.pending_model = True
        current_session = context.runtime.session_id if context.runtime else None
        for old_session, owner in list(self.sessions.items()):
            if owner is context and old_session != current_session:
                del self.sessions[old_session]
                if old_session in self.recent:
                    self.recent.remove(old_session)
        if current_session:
            self.sessions[current_session] = context
            self._touch(current_session)
        await self._evict_idle()
        return result

    async def aclose(self):
        contexts = set(self.clients.values()) | set(self.sessions.values())
        running = [context.run_task for context in contexts if context.run_task and not context.run_task.done()]
        for task in running:
            task.cancel()
        await asyncio.gather(*running, return_exceptions=True)
        await asyncio.gather(*(context.discard_runtime() for context in contexts), return_exceptions=True)
        self.store.close()


async def main():
    bridge = BridgeRouter()
    server = RequestServer(bridge.handle, emit)
    try:
        while line := await asyncio.to_thread(sys.stdin.readline):
            server.submit(line)
    finally:
        await server.aclose()
        await bridge.aclose()


if __name__ == "__main__":
    asyncio.run(main())
