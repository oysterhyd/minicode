#!/usr/bin/env python3
"""P1 offline evaluation: 20 local bug-fix tasks x three baselines (FakeProvider).

对每个 (task, baseline) 组合，把任务的 ``repo/`` 拷到全新临时工作区，用
FakeProvider 重放 ``script.json`` 驱动 AgentRuntime，然后由 runner 自己做验收
（子进程跑验收命令 + 对比受保护文件哈希），不依赖 goals 包。

基线语义（详见 evals/README.md）：

- ``b0`` 基础循环：一次 ``run_turn``，无压缩无验收门；结束后 runner 验收。
- ``b1`` 同 b0：压缩钩子（compactor）参数位置已预留，主 agent 集成后接线；
  当前仅用于记录未压缩时的 token 基线。
- ``b2`` b1 + 验收失败续跑：验收失败且预算/续跑次数未耗尽时，把失败详情作为
  新一轮 ``run_turn`` 输入继续跑，最多 ``max_fix_attempts`` 次。

用法::

    PYTHONPATH=src python -m evals.run_eval --baselines b0,b2 --output reports/eval
    python evals/run_eval.py --task pagination_bounds --keep-workspaces
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parents[1] / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))

import argparse
import asyncio
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

import yaml
from pydantic import ValidationError

from minicode.core.models import Budget
from minicode.providers import FakeProvider, FakeProviderOptions, FakeTurn
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.tools.registry import default_registry

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TASKS_DIR = Path(__file__).resolve().parent / "tasks"
DEFAULT_OUTPUT = REPO_ROOT / "reports" / "eval"
BASELINES = ("b0", "b1", "b2")
ACCEPTANCE_TIMEOUT_S = 120.0

# ---------------------------------------------------------------------------
# Errors
# ---------------------------------------------------------------------------


class EvalError(Exception):
    """An evaluation-input problem (missing files, bad YAML, bad script)."""


class ScriptError(EvalError):
    """A FakeProvider script could not be loaded."""


# ---------------------------------------------------------------------------
# FakeProvider script loading (self-contained: cli.py would pull in typer/rich)
# ---------------------------------------------------------------------------


def load_script(path: Path) -> FakeProviderOptions:
    """Parse a FakeProvider script JSON file, raising :class:`ScriptError` on
    anything off (missing file, invalid JSON, invalid turn fields)."""
    if not path.is_file():
        raise ScriptError(f"脚本文件不存在: {path}")
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ScriptError(f"无法读取脚本 {path}: {exc}") from exc
    if not isinstance(raw, dict):
        raise ScriptError(f"脚本根节点必须是 JSON 对象: {path}")
    turns_raw = raw.get("turns", [])
    if not isinstance(turns_raw, list):
        raise ScriptError(f"脚本的 turns 必须是数组: {path}")
    try:
        turns = [FakeTurn(**turn) for turn in turns_raw]
    except ValidationError as exc:
        raise ScriptError(f"脚本 turn 字段不合法 ({path}): {exc}") from exc
    kwargs = {key: value for key, value in raw.items() if key != "turns"}
    try:
        return FakeProviderOptions(turns=turns, **kwargs)
    except ValidationError as exc:
        raise ScriptError(f"脚本字段不合法 ({path}): {exc}") from exc


# ---------------------------------------------------------------------------
# Task definitions (task.yaml, schema shared with the goals module)
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class AcceptanceItem:
    """One acceptance item from ``task.yaml``: a command or a protected file."""

    id: str
    type: str  # "command" | "protected"
    command: str | None = None
    path: str | None = None


@dataclass(slots=True)
class TaskDef:
    """A loaded evaluation task."""

    id: str
    title: str
    prompt: str
    allowed_paths: list[str]
    protected_paths: list[str]
    items: list[AcceptanceItem]
    max_rounds: int
    max_tokens: int
    max_seconds: float
    max_fix_attempts: int
    repo_dir: Path
    script_path: Path


def load_task(task_dir: Path) -> TaskDef:
    """Load and validate one ``task.yaml`` (schema: 见 evals/README.md）。"""
    yaml_path = task_dir / "task.yaml"
    if not yaml_path.is_file():
        raise EvalError(f"缺少 task.yaml: {task_dir}")
    try:
        raw = yaml.safe_load(yaml_path.read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError) as exc:
        raise EvalError(f"task.yaml 解析失败 ({yaml_path}): {exc}") from exc
    if not isinstance(raw, dict):
        raise EvalError(f"task.yaml 根节点必须是映射: {yaml_path}")

    def require(key: str) -> Any:
        if key not in raw:
            raise EvalError(f"task.yaml 缺少字段 {key!r}: {yaml_path}")
        return raw[key]

    task_id = require("id")
    if task_id != task_dir.name:
        raise EvalError(f"task.id ({task_id}) 与目录名 ({task_dir.name}) 不一致")

    items_raw = require("items")
    if not isinstance(items_raw, list) or not items_raw:
        raise EvalError(f"items 必须是非空数组: {yaml_path}")
    items: list[AcceptanceItem] = []
    for entry in items_raw:
        item_id = entry.get("id", "?")
        item_type = entry.get("type")
        if item_type == "command":
            items.append(AcceptanceItem(id=item_id, type=item_type, command=entry.get("command")))
        elif item_type == "protected":
            items.append(AcceptanceItem(id=item_id, type=item_type, path=entry.get("path")))
        else:
            raise EvalError(f"items 里的 type 必须是 command/protected: {entry}")

    budget = require("budget")
    script_name = raw.get("script", "script.json")
    return TaskDef(
        id=task_id,
        title=str(require("title")),
        prompt=str(require("prompt")),
        allowed_paths=list(require("allowed_paths")),
        protected_paths=list(require("protected_paths")),
        items=items,
        max_rounds=int(budget["max_rounds"]),
        max_tokens=int(budget["max_tokens"]),
        max_seconds=float(budget["max_seconds"]),
        max_fix_attempts=int(raw.get("max_fix_attempts", 0)),
        repo_dir=task_dir / "repo",
        script_path=task_dir / str(script_name),
    )


def discover_tasks(tasks_dir: Path, only: list[str] | None) -> list[TaskDef]:
    """Discover task dirs (sorted) and optionally filter by explicit ids."""
    if not tasks_dir.is_dir():
        raise EvalError(f"任务目录不存在: {tasks_dir}")
    dirs = sorted(p for p in tasks_dir.iterdir() if (p / "task.yaml").is_file())
    if not dirs:
        raise EvalError(f"{tasks_dir} 下没有任何任务（缺少 <id>/task.yaml）")
    tasks = [load_task(d) for d in dirs]
    if only:
        known = {t.id for t in tasks}
        unknown = [tid for tid in only if tid not in known]
        if unknown:
            raise EvalError(f"未知任务 id: {', '.join(unknown)}（可用: {', '.join(sorted(known))}）")
        wanted = set(only)
        tasks = [t for t in tasks if t.id in wanted]
    return tasks


# ---------------------------------------------------------------------------
# Acceptance checking (runner-owned: subprocess command + protected-file hash)
# ---------------------------------------------------------------------------


def ensure_python_on_path() -> None:
    """Prepend this interpreter's directory to PATH so ``python``/``pytest``
    inside tool subprocesses (bash) and acceptance checks resolve to the
    same environment the runner was started with, regardless of shell state."""
    exe_dir = str(Path(sys.executable).resolve().parent)
    parts = os.environ.get("PATH", "").split(os.pathsep)
    if exe_dir not in parts:
        os.environ["PATH"] = os.pathsep.join([exe_dir, *parts])


def _shell_wrap(command: str) -> list[str]:
    """Same shell strategy as RunCommandTool: pwsh/powershell on Windows."""
    if os.name == "nt":
        shell = shutil.which("pwsh") or "powershell"
        return [shell, "-NoProfile", "-NonInteractive", "-Command", command]
    return ["bash", "-c", command]


def run_acceptance_command(command: str, workspace: Path) -> tuple[int, str]:
    """Run one acceptance command in *workspace*; return (exit_code, output)."""
    try:
        proc = subprocess.run(
            _shell_wrap(command),
            cwd=str(workspace),
            capture_output=True,
            timeout=ACCEPTANCE_TIMEOUT_S,
        )
    except subprocess.TimeoutExpired:
        return 124, f"验收命令超时（>{ACCEPTANCE_TIMEOUT_S:g}s）: {command}"
    except OSError as exc:
        return 125, f"验收命令无法启动: {exc}"
    combined = proc.stdout + proc.stderr
    output = combined.decode("utf-8", errors="replace").replace("\r\n", "\n")
    return proc.returncode, output[-2000:]


def sha256_file(path: Path) -> str | None:
    """SHA-256 of a file, or None when it does not exist / cannot be read."""
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError:
        return None


def snapshot_protected(task: TaskDef, workspace: Path) -> dict[str, str | None]:
    """Hash every protected file right after the clean copy (before the run)."""
    return {rel: sha256_file(workspace / rel) for rel in task.protected_paths}


def check_acceptance(
    task: TaskDef, workspace: Path, protected_hashes: dict[str, str | None]
) -> tuple[bool, list[str]]:
    """Run every acceptance item; return (all_passed, failure_details)."""
    failures: list[str] = []
    for item in task.items:
        if item.type == "command":
            assert item.command is not None  # validated in load_task
            code, _output = run_acceptance_command(item.command, workspace)
            if code != 0:
                failures.append(f"[{item.id}] 命令 `{item.command}` 退出码 {code}")
        elif item.type == "protected":
            assert item.path is not None  # validated in load_task
            if sha256_file(workspace / item.path) != protected_hashes.get(item.path):
                failures.append(f"[{item.id}] 受保护文件被修改或缺失: {item.path}")
    return (not failures), failures


def build_continuation_message(failures: list[str]) -> str:
    """The message fed into the next run_turn when b2 continues after failure."""
    lines = ["上一轮修复未通过验收，必须修复以下问题后重新验收："]
    lines.extend(f"- {failure}" for failure in failures)
    lines.append("必须修复，不得改受保护文件。请先读取相关文件，再用 edit 修复，并重新运行验收命令确认。")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# One (task, baseline) run
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class RunRecord:
    """Outcome of one (task, baseline) combination."""

    task: str
    baseline: str
    passed: bool
    exit_reason: str
    attempts: int
    rounds: int
    input_tokens: int
    output_tokens: int
    seconds: float
    error: str | None

    def to_json(self) -> dict[str, Any]:
        # JSON key is literally "pass" per the results.json contract.
        return {
            "task": self.task,
            "baseline": self.baseline,
            "pass": self.passed,
            "exit_reason": self.exit_reason,
            "attempts": self.attempts,
            "rounds": self.rounds,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "seconds": round(self.seconds, 3),
            "error": self.error,
        }


def _fresh_workspace(task: TaskDef, baseline: str, keep: bool, keep_root: Path) -> tuple[Path, Path, Path | None]:
    """Create a clean workspace copy; return (workspace, db_path, cleanup_dir)."""
    if keep:
        combo_dir = keep_root / f"{task.id}__{baseline}"
        if combo_dir.exists():
            shutil.rmtree(combo_dir)
        combo_dir.mkdir(parents=True)
        return combo_dir / "workspace", combo_dir / "session.sqlite3", None
    tmp_dir = Path(tempfile.mkdtemp(prefix=f"minicode-eval-{task.id}-{baseline}-"))
    return tmp_dir / "workspace", tmp_dir / "session.sqlite3", tmp_dir


async def run_combo(
    task: TaskDef,
    baseline: str,
    *,
    keep: bool = False,
    keep_root: Path | None = None,
    # TODO(P1): 压缩钩子由主 agent 集成后，在 baseline == "b1" 时在此传入
    # compactor 实例并接线到 AgentRuntime；当前 b1 与 b0 行为完全一致。
    compactor: Any | None = None,
) -> RunRecord:
    """Run one (task, baseline) combo on a fresh workspace copy."""
    del compactor  # reserved slot, see TODO above
    if not task.repo_dir.is_dir():
        raise EvalError(f"任务缺少 repo/ 目录: {task.repo_dir}")
    workspace, db_path, cleanup_dir = _fresh_workspace(
        task, baseline, keep, keep_root or (Path.cwd() / "workspaces")
    )
    shutil.copytree(
        task.repo_dir,
        workspace,
        ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache"),
    )
    protected_hashes = snapshot_protected(task, workspace)
    options = load_script(task.script_path)
    budget = Budget(
        max_rounds=task.max_rounds,
        max_total_tokens=task.max_tokens,
        max_seconds=task.max_seconds,
    )
    store = SqliteStore(db_path)
    started = time.monotonic()
    try:
        runtime = AgentRuntime(
            provider=FakeProvider(options),
            registry=default_registry(),
            store=store,
            policy=AutoAllowPolicy(),
            workspace=workspace,
            provider_name="fake",
            model="scripted-fix",
            budget=budget,
        )
        result = await runtime.run_turn(task.prompt)
        attempts = 1
        passed, failures = check_acceptance(task, workspace, protected_hashes)
        if baseline == "b2" and not passed:
            # b2: 验收失败续跑 —— 失败详情作为新的一轮输入，直到通过或预算耗尽。
            for _ in range(task.max_fix_attempts):
                rounds_left = runtime.rounds < task.max_rounds
                tokens_left = runtime.usage.total_tokens < task.max_tokens
                if not (rounds_left and tokens_left):
                    break
                result = await runtime.run_turn(build_continuation_message(failures))
                attempts += 1
                passed, failures = check_acceptance(task, workspace, protected_hashes)
                if passed:
                    break
        seconds = time.monotonic() - started
        usage = runtime.usage
        return RunRecord(
            task=task.id,
            baseline=baseline,
            passed=passed,
            exit_reason=result.exit_reason.value,
            attempts=attempts,
            rounds=runtime.rounds,
            input_tokens=usage.input_tokens,
            output_tokens=usage.output_tokens,
            seconds=seconds,
            error=None if passed else "; ".join(failures),
        )
    finally:
        store.close()
        if cleanup_dir is not None:
            shutil.rmtree(cleanup_dir, ignore_errors=True)


# ---------------------------------------------------------------------------
# Reports
# ---------------------------------------------------------------------------


def _fmt_rate(passed: int, total: int) -> str:
    return f"{passed}/{total}（{passed / total:.0%}）" if total else "-"


def baseline_stats(records: list[RunRecord], baseline: str) -> dict[str, float]:
    """Aggregate one baseline's records into summary numbers."""
    rows = [r for r in records if r.baseline == baseline]
    count = len(rows)
    if not count:
        return {"passed": 0, "total": 0}
    passed = sum(1 for r in rows if r.passed)
    return {
        "passed": passed,
        "total": count,
        "avg_rounds": sum(r.rounds for r in rows) / count,
        "avg_input": sum(r.input_tokens for r in rows) / count,
        "avg_output": sum(r.output_tokens for r in rows) / count,
        "avg_seconds": sum(r.seconds for r in rows) / count,
        "failures": count - passed,
    }


def build_summary_md(
    records: list[RunRecord],
    tasks: list[TaskDef],
    baselines: list[str],
    generated_at: str,
) -> str:
    """Deterministic markdown report: 对照表 + 逐任务表 + 失败明细 + 失败分析。"""
    titles = {t.id: t.title for t in tasks}
    lines: list[str] = []
    lines.append("# Mini Claude Code 本地评测结果")
    lines.append("")
    lines.append(f"- 生成时间：{generated_at}")
    lines.append(f"- 基线：{'、'.join(baselines)}")
    lines.append(f"- 运行数：{len(records)}（{len(tasks)} 个任务 × {len(baselines)} 个基线）")
    lines.append("")
    lines.append("## 基线对照")
    lines.append("")
    lines.append("| 基线 | 通过 | 平均轮数 | 平均输入 token | 平均输出 token | 平均耗时(s) |")
    lines.append("|---|---|---|---|---|---|")
    for baseline in baselines:
        stats = baseline_stats(records, baseline)
        if stats["total"] == 0:
            lines.append(f"| {baseline} | - | - | - | - | - |")
            continue
        lines.append(
            f"| {baseline} | {_fmt_rate(int(stats['passed']), int(stats['total']))} "
            f"| {stats['avg_rounds']:.1f} | {stats['avg_input']:.0f} "
            f"| {stats['avg_output']:.0f} | {stats['avg_seconds']:.1f} |"
        )
    lines.append("")
    lines.append("## 逐任务结果")
    lines.append("")
    header = "| 任务 | 标题 | " + " | ".join(baselines) + " |"
    sep = "|---|---|" + "---|" * len(baselines)
    lines.append(header)
    lines.append(sep)
    for task in tasks:
        cells = []
        for baseline in baselines:
            record = next(
                (r for r in records if r.task == task.id and r.baseline == baseline), None
            )
            if record is None:
                cells.append("-")
            elif record.passed:
                cells.append("✓")
            else:
                cells.append(f"✗（{record.exit_reason}）")
        lines.append(f"| {task.id} | {titles[task.id]} | " + " | ".join(cells) + " |")
    lines.append("")
    lines.append("## 失败任务与退出原因")
    lines.append("")
    failed = [r for r in records if not r.passed]
    if not failed:
        lines.append("无 —— 全部运行通过验收。")
    else:
        for record in failed:
            error = f"；{record.error}" if record.error else ""
            lines.append(
                f"- {record.task} × {record.baseline}：exit_reason={record.exit_reason}"
                f"（attempts={record.attempts}, rounds={record.rounds}）{error}"
            )
    lines.append("")
    lines.append("## 失败分析")
    lines.append("")
    lines.extend(build_analysis(records, baselines))
    lines.append("")
    return "\n".join(lines)


def build_analysis(records: list[RunRecord], baselines: list[str]) -> list[str]:
    """Deterministic failure-analysis paragraphs (template text, no LLM)."""
    paragraphs: list[str] = []
    failed = [r for r in records if not r.passed]
    if not failed:
        if "b0" in baselines and "b2" in baselines:
            b0 = baseline_stats(records, "b0")
            b2 = baseline_stats(records, "b2")
            if b0["total"] and b2["total"]:
                paragraphs.append(
                    f"本次运行全部通过。b2 平均轮数 {b2['avg_rounds']:.1f} 与 b0 平均轮数"
                    f" {b0['avg_rounds']:.1f} 一致：FakeProvider 脚本保证一次修复即可通过验收，"
                    "b2 的续跑分支（验收失败 → 追加一轮 run_turn）在全部通过时不会被触发，"
                    "两者的差异只在验收失败时出现。"
                )
        paragraphs.append(
            "成功率反映的是 harness 机制（脚本重放、预算、验收、续跑）是否按设计工作，"
            "而不是模型解决问题的能力 —— 见 README 的诚实说明。"
        )
        return paragraphs

    by_reason: dict[str, int] = {}
    for record in failed:
        by_reason[record.exit_reason] = by_reason.get(record.exit_reason, 0) + 1
    reason_text = "、".join(f"{reason}={count}" for reason, count in sorted(by_reason.items()))
    paragraphs.append(f"共 {len(failed)} 次运行失败，按退出原因分布：{reason_text}。")

    if "completed" in by_reason:
        paragraphs.append(
            "exit_reason=completed 但验收失败：模型侧已自述完成（脚本走到最终总结），"
            "但修复未生效 —— 常见原因是 edit 的 old_text 与源文件不一致（脚本与"
            " repo 不同步），或续跑轮次耗尽后仍未收敛。应先核对 script.json 与 repo/"
            " 源文件是否由同一份模板生成。"
        )
    if "max_rounds" in by_reason:
        paragraphs.append(
            "exit_reason=max_rounds：轮数预算耗尽。续跑机制每轮消耗一个 round，"
            "预算中 max_rounds 需要覆盖初始修复 + 每次续跑的开销。"
        )
    if any(r in by_reason for r in ("token_budget", "time_budget")):
        paragraphs.append(
            "exit_reason=token_budget/time_budget：token 或时长预算耗尽，通常是任务"
            "或脚本规模超出预算设定，而不是机制缺陷。"
        )
    protected_failures = [r for r in failed if r.error and "受保护文件" in r.error]
    if protected_failures:
        paragraphs.append(
            f"有 {len(protected_failures)} 次运行改动了受保护文件：保护约定未被遵守，"
            "说明权限/提示层对写操作的约束需要加强（这是 harness 层问题，不是脚本问题）。"
        )
    return paragraphs


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="evals.run_eval",
        description="Mini Claude Code P1 本地评测（FakeProvider，三基线）。",
    )
    parser.add_argument("--tasks-dir", default=str(DEFAULT_TASKS_DIR), help="任务根目录")
    parser.add_argument("--task", action="append", default=[], help="只跑指定任务 id（可重复）")
    parser.add_argument(
        "--baselines", default=",".join(BASELINES), help="逗号分隔的基线（b0,b1,b2）"
    )
    parser.add_argument("--output", default=str(DEFAULT_OUTPUT), help="结果输出目录")
    parser.add_argument("--keep-workspaces", action="store_true", help="保留每个组合的工作区副本")
    args = parser.parse_args(argv)

    baselines = [b.strip() for b in args.baselines.split(",") if b.strip()]
    invalid = [b for b in baselines if b not in BASELINES]
    if invalid:
        print(f"未知基线: {', '.join(invalid)}（可选: {', '.join(BASELINES)}）", file=sys.stderr)
        return 2
    if not baselines:
        print("至少需要一个基线", file=sys.stderr)
        return 2

    ensure_python_on_path()
    try:
        tasks = discover_tasks(Path(args.tasks_dir), args.task or None)
    except EvalError as exc:
        print(f"错误: {exc}", file=sys.stderr)
        return 2

    output_dir = Path(args.output)
    output_dir.mkdir(parents=True, exist_ok=True)
    keep_root = output_dir / "workspaces"

    combos = [(task, baseline) for task in tasks for baseline in baselines]
    records: list[RunRecord] = []
    infra_errors = 0

    async def run_all() -> None:
        nonlocal infra_errors
        for index, (task, baseline) in enumerate(combos, 1):
            try:
                record = await run_combo(
                    task, baseline, keep=args.keep_workspaces, keep_root=keep_root
                )
            except EvalError as exc:
                infra_errors += 1
                print(f"[{index}/{len(combos)}] {task.id} × {baseline}: ERROR {exc}", file=sys.stderr)
                record = RunRecord(
                    task=task.id,
                    baseline=baseline,
                    passed=False,
                    exit_reason="error",
                    attempts=0,
                    rounds=0,
                    input_tokens=0,
                    output_tokens=0,
                    seconds=0.0,
                    error=str(exc),
                )
            records.append(record)
            status = "PASS" if record.passed else "FAIL"
            print(
                f"[{index}/{len(combos)}] {task.id} × {baseline}: {status}"
                f" rounds={record.rounds} tokens={record.input_tokens + record.output_tokens}"
                f" attempts={record.attempts} {record.seconds:.1f}s"
            )

    asyncio.run(run_all())

    generated_at = datetime.now(timezone.utc).isoformat(timespec="seconds")
    results = {
        "generated_at": generated_at,
        "tasks_dir": str(Path(args.tasks_dir).resolve()),
        "baselines": baselines,
        "run_count": len(records),
        "passed": sum(1 for r in records if r.passed),
        "records": [r.to_json() for r in records],
    }
    results_path = output_dir / "results.json"
    results_path.write_text(
        json.dumps(results, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    summary_path = output_dir / "summary.md"
    summary_path.write_text(
        build_summary_md(records, tasks, baselines, generated_at), encoding="utf-8"
    )

    print()
    for baseline in baselines:
        stats = baseline_stats(records, baseline)
        if stats["total"]:
            print(
                f"{baseline}: {stats['passed']}/{stats['total']} 通过"
                f"（平均 {stats['avg_rounds']:.1f} 轮 / {stats['avg_input'] + stats['avg_output']:.0f} token）"
            )
    print(f"结果已写入 {results_path} 和 {summary_path}")
    return 1 if infra_errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
