"""Persistent harness services and personal delegate definitions shared by hosts.

Secrets stay in the bridge; public drafts contain only hasApiKey.
"""
from __future__ import annotations

import json
import os
import re
import tempfile
import uuid
from pathlib import Path
from urllib.parse import urlparse

from minicode.core.catalog import MODEL_CATALOG
from minicode.providers.zcode_config import discover_commandcode


TOOLS = ["read", "ls", "grep", "read_artifact", "bash", "edit", "write"]
BUILTIN_AGENTS = [
    dict(name="explore", label="探索者", description="调查项目结构，定位文件和实现，报告证据。", tools=["read", "ls", "grep", "read_artifact"], instructions="查找与任务相关的实现，给出文件路径和发现。"),
    dict(name="review", label="代码审查员", description="审查具体改动的正确性、边界情况和回归风险。", tools=["read", "ls", "grep", "read_artifact"], instructions="审查指定代码，只报告可验证的问题和证据。"),
    dict(name="test-runner", label="测试执行者", description="执行指定测试或构建，分析失败原因。", tools=["read", "ls", "grep", "bash"], instructions="运行与任务相关的测试或构建，报告命令、结果和失败原因。"),
    dict(name="fixer", label="修复者", description="根据具体任务实现完整的代码修复。", tools=TOOLS, instructions="先检查相关代码，完成最小完整修复，验证后报告改动和证据。"),
    dict(name="ui-designer", label="UI 设计师", description="实现界面和交互，检查布局、可访问性与状态。", tools=TOOLS, instructions="沿用项目视觉风格，实现任务要求的界面和交互，并验证布局。"),
]


class HarnessConfiguration:
    def __init__(self, path: Path | None = None):
        self.path = path or Path.home() / ".minicode" / "desktop-config.json"

    def read(self):
        if self.path.exists():
            if self.path.stat().st_size > 4_000_000:
                raise ValueError("harness configuration exceeds the size limit")
            data = json.loads(self.path.read_text(encoding="utf-8"))
            if (not isinstance(data, dict) or not isinstance(data.get("services"), list)
                    or not isinstance(data.get("agents"), list)):
                raise ValueError("harness configuration must contain services and agents lists")
            return data
        discovered = discover_commandcode()
        services = []
        for provider, label, url in [("commandcode", "CommandCode", discovered[0] if discovered else "https://api.commandcode.ai/provider/v1"), ("anthropic", "Anthropic", "https://api.anthropic.com")]:
            services.append(dict(id=provider, name=label, baseUrl=url,
                                 apiStyle="anthropic" if provider == "anthropic" else "openai",
                                 apiKey="", enabled=True, builtin=True,
                                 models=[dict(modelId=i.name, name=i.name, contextWindow=i.context_window,
                                              maxOutputTokens=i.max_output_tokens or 8192, supportsEffort=i.supports_effort)
                                         for i in MODEL_CATALOG.values() if i.provider == provider]))
        return dict(services=services, agents=[], disabledAgents=[], defaultModel="")

    def write(self, data):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=self.path.parent, delete=False) as stream:
                temporary = Path(stream.name)
                json.dump(data, stream, ensure_ascii=False, indent=2)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, self.path)
        finally:
            if temporary:
                temporary.unlink(missing_ok=True)

    @staticmethod
    def model_key(service, model):
        original = MODEL_CATALOG.get(model["modelId"])
        return model["modelId"] if service.get("builtin") and original and original.provider == service["id"] else f'{service["id"]}::{model["modelId"]}'

    def credentials(self, service):
        if service.get("apiKey"):
            return service["baseUrl"], service["apiKey"]
        if service["id"] == "commandcode":
            found = discover_commandcode()
            if found:
                return service["baseUrl"], found[1]
        if service["id"] == "anthropic":
            return service["baseUrl"], os.environ.get("ANTHROPIC_API_KEY", "")
        return service["baseUrl"], ""

    def public(self):
        data = self.read()
        return {**data, "services": [{**s, "apiKey": "", "hasApiKey": bool(self.credentials(s)[1])} for s in data["services"]], "agents": self.agents()}

    def models(self):
        return [dict(id=self.model_key(s, m), name=m.get("name") or m["modelId"], modelId=m["modelId"],
                     serviceId=s["id"], provider=s["name"], supportsEffort=m["supportsEffort"],
                     contextWindow=m["contextWindow"], maxOutputTokens=m["maxOutputTokens"],
                     available=bool(s["enabled"] and self.credentials(s)[1]))
                for s in self.read()["services"] for m in s["models"]]

    def find_model(self, key):
        for service in self.read()["services"]:
            for model in service["models"]:
                if self.model_key(service, model) == key:
                    return service, model
        return None

    def save_service(self, draft):
        data = self.read()
        existing = next((s for s in data["services"] if s["id"] == draft.get("id")), None)
        service = {**draft, "id": existing["id"] if existing else uuid.uuid4().hex,
                   "builtin": bool(existing and existing.get("builtin")),
                   "apiKey": draft.get("apiKey") or (existing or {}).get("apiKey", "")}
        service.pop("hasApiKey", None)
        service["name"] = str(service.get("name", "")).strip()
        service["baseUrl"] = str(service.get("baseUrl", "")).strip().rstrip("/")
        parsed = urlparse(service["baseUrl"])
        if not service["name"] or parsed.scheme not in {"http", "https"} or not parsed.netloc or parsed.username or parsed.password or parsed.query or parsed.fragment:
            raise ValueError("请填写服务名称和有效的 HTTP(S) 接口地址")
        if service.get("apiStyle") not in {"openai", "anthropic"}:
            raise ValueError("不支持的接口格式")
        seen = set()
        for model in service.get("models", []):
            model["modelId"] = str(model.get("modelId", "")).strip()
            if not model["modelId"] or model["modelId"] in seen:
                raise ValueError("模型 ID 不能为空或重复")
            seen.add(model["modelId"])
            model["contextWindow"] = int(model["contextWindow"])
            model["maxOutputTokens"] = int(model["maxOutputTokens"])
            if not 0 < model["maxOutputTokens"] < model["contextWindow"]:
                raise ValueError("输出上限必须大于 0 且小于上下文窗口")
            model["supportsEffort"] = bool(model.get("supportsEffort")) and service["apiStyle"] == "openai"
        service["enabled"] = bool(service.get("enabled", True))
        data["services"] = [s for s in data["services"] if s["id"] != service["id"]] + [service]
        valid = {self.model_key(s, m) for s in data["services"] if s["enabled"] and self.credentials(s)[1] for m in s["models"]}
        if data.get("defaultModel") not in valid:
            data["defaultModel"] = next((self.model_key(s, m) for s in data["services"] if s["enabled"] and self.credentials(s)[1] for m in s["models"]), "")
        self.write(data)
        return self.public()

    def agents(self):
        data = self.read()
        return [{**a, "builtin": builtin, "inheritTools": a.get("inheritTools", False),
                 "enabled": a["name"] not in data.get("disabledAgents", [])}
                for builtin, items in [(True, BUILTIN_AGENTS), (False, data["agents"])] for a in items]

    def save_agent(self, draft):
        data = self.read()
        name = str(draft.get("name", "")).strip()
        if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}", name) or name in {a["name"] for a in BUILTIN_AGENTS}:
            raise ValueError("名称需为 1–64 位字母、数字、连字符或下划线，不能与内置子助手重复")
        original = draft.get("originalName")
        if any(a["name"] == name and a["name"] != original for a in data["agents"]):
            raise ValueError("该子助手名称已存在")
        if not draft.get("description", "").strip() or not draft.get("instructions", "").strip():
            raise ValueError("请填写委派条件和指令")
        if len(draft["instructions"].encode("utf-8")) > 32_000:
            raise ValueError("指令不能超过 32 KB")
        if any(t not in TOOLS for t in draft.get("tools", [])):
            raise ValueError("未知工具")
        agent = {k: draft[k] for k in ["description", "instructions", "tools", "inheritTools"]}
        agent.update(name=name, label=draft.get("label") or name)
        data["agents"] = [a for a in data["agents"] if a["name"] != original] + [agent]
        data["disabledAgents"] = [n for n in data.get("disabledAgents", []) if n != original]
        self.write(data)
        return self.agents()
