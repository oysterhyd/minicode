#!/usr/bin/env python3
"""Repeatable CommandCode evaluation with independent host-side grading.

Each run gets a clean fixture and its own durable session database. A record
is written only after grading; completed records are never rerun implicitly.
No model key or raw gateway response is written to the result files.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
import time
import zipfile
from datetime import datetime, timezone
from pathlib import Path

from evals.run_eval import (DEFAULT_TASKS_DIR, EvalError, TaskDef, check_acceptance,
                      check_allowed_paths, discover_tasks, ensure_python_on_path,
                      snapshot_protected, snapshot_workspace)
from evals.hidden import HIDDEN_DIR, check_hidden

from minicode.context.compact import CompactConfig, ContextCompactor
from minicode.core.catalog import lookup_model
from minicode.core.models import Budget, EventType, ExitReason
from minicode.goals import AcceptanceItem, AcceptanceSpec, EvidenceLedger, GoalChecker, ProtectedSnapshot
from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.zcode_config import DEFAULT_MODEL, discover_commandcode
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.registry import default_registry

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = Path.cwd() / "reports" / "eval-real"


def _file_hashes(root: Path) -> dict[str, str]:
    return snapshot_workspace(root)


def _fixture_hash(task: TaskDef) -> str:
    digest = hashlib.sha256()
    digest.update((task.repo_dir.parent / "task.yaml").read_bytes())
    for name, value in sorted(_file_hashes(task.repo_dir).items()):
        digest.update(name.encode("utf-8") + b"\0" + value.encode("utf-8"))
    return digest.hexdigest()


def _git_revision() -> str | None:
    proc = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True, check=False)
    return proc.stdout.strip() if proc.returncode == 0 else None


def _source_files(tasks: list[TaskDef]) -> list[Path]:
    package_root = ROOT / "src" / "minicode"
    if not package_root.is_dir():
        package_root = ROOT / "minicode"  # installed wheel
    files = list(package_root.rglob("*.py"))
    files.extend(ROOT / "evals" / name for name in
                 ("run_eval.py", "run_real_eval.py", "hidden.py"))
    for task in tasks:
        files.extend(path for path in task.repo_dir.parent.rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts)
        files.append(HIDDEN_DIR / task.id / "test_hidden.py")
    return sorted(set(files))


def _source_digest(files: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in files:
        digest.update(_source_label(path).encode("utf-8") + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def _source_label(path: Path) -> str:
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        tag = hashlib.sha256(str(path.parent).encode("utf-8")).hexdigest()[:12]
        return f"external/{tag}/{path.name}"


def _snapshot_sources(output: Path, files: list[Path]) -> str:
    snapshot = output / "source-snapshot.zip"
    if not snapshot.exists():
        temporary = output / "source-snapshot.zip.tmp"
        with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in files:
                archive.write(path, _source_label(path))
        os.replace(temporary, snapshot)
    return hashlib.sha256(snapshot.read_bytes()).hexdigest()


def _goal_spec(task: TaskDef) -> AcceptanceSpec:
    return AcceptanceSpec(
        id=task.id, title=task.title, max_fix_attempts=max(1, task.max_fix_attempts),
        items=[AcceptanceItem(id=item.id, type=item.type,
                              command=item.command, path=item.path) for item in task.items],
    )


async def run_one(task: TaskDef, baseline: str, repeat: int, output: Path,
                  model: str) -> dict:
    run_dir = output / "runs" / task.id / baseline / str(repeat)
    record_path = run_dir / "record.json"
    if record_path.exists():
        previous = json.loads(record_path.read_text(encoding="utf-8"))
        if previous["model"] != model or previous["fixture_sha256"] != _fixture_hash(task):
            raise EvalError(f"existing run has different model or fixture: {record_path}")
        hidden_digest = previous.get("hidden_test_sha256")
        if hidden_digest and hidden_digest != hashlib.sha256(
            (HIDDEN_DIR / task.id / "test_hidden.py").read_bytes()
        ).hexdigest():
            raise EvalError(f"hidden acceptance changed; rescore saved workspace first: {record_path}")
        return previous
    if run_dir.exists():
        raise EvalError(f"incomplete run directory needs inspection: {run_dir}")
    run_dir.mkdir(parents=True)
    workspace = run_dir / "workspace"
    shutil.copytree(task.repo_dir, workspace, ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache"))
    before = _file_hashes(workspace)
    protected = snapshot_protected(task, workspace)
    store = SqliteStore(run_dir / "session.sqlite3")
    provider = CommandCodeProvider(model=model, max_tokens=lookup_model(model).max_output_tokens)
    registry = default_registry()
    artifacts = ArtifactStore(store)
    goal = None
    ledger = None
    if baseline == "b2":
        goal = GoalChecker(_goal_spec(task), workspace,
                           ProtectedSnapshot(workspace, task.protected_paths))
        ledger = EvidenceLedger()
    runtime = AgentRuntime(provider=provider, registry=registry, store=store,
                           policy=AutoAllowPolicy(), workspace=workspace,
                           provider_name="commandcode", model=model,
                           budget=Budget(max_rounds=task.max_rounds,
                                         max_total_tokens=task.max_tokens,
                                         max_seconds=task.max_seconds),
                           goal_checker=goal, evidence_ledger=ledger,
                           artifact_store=artifacts)
    if baseline == "b2":
        runtime._compactor = ContextCompactor(
            CompactConfig(max_context_tokens=max(runtime.context_window, 1)),
            spill_fn=lambda kind, content: artifacts.spill(runtime.session_id, kind, content).artifact_id,
            context_tokens_fn=lambda: runtime.context_window,
            output_tokens_fn=runtime.effective_max_output_tokens,
            estimate_scale_fn=lambda: getattr(runtime.provider, "prompt_scale", 1.0),
        )
    started_at = datetime.now(timezone.utc).isoformat()
    started = time.monotonic()
    result = None
    error = None
    try:
        result = await runtime.run_turn(task.prompt)
    except Exception as exc:  # infrastructure failure still stays in denominator
        error = f"{type(exc).__name__}: {exc}"
    finally:
        close = getattr(provider, "aclose", None)
        if callable(close):
            await close()
        await registry.aclose()
    runtime_seconds = round(time.monotonic() - started, 3)
    passed_checks, failures = check_acceptance(task, workspace, protected)
    failures.extend(check_allowed_paths(task, before, workspace))
    visible_pass = bool(result is not None and result.exit_reason is ExitReason.COMPLETED
                        and passed_checks and not failures)
    hidden_pass, hidden_detail = await check_hidden(task.id, workspace)
    if not hidden_pass:
        failures.append("[hidden] host-side acceptance failed")
    elapsed = round(time.monotonic() - started, 3)
    passed = visible_pass and hidden_pass
    events = store.get_events(runtime.session_id) if runtime.session_id else []
    announced_complete = result is not None and result.exit_reason is ExitReason.COMPLETED
    record = {
        "task": task.id, "baseline": baseline, "repeat": repeat,
        "model": model, "provider": "commandcode", "seed": None,
        "fixture_sha256": _fixture_hash(task), "git_revision": _git_revision(),
        "prompt_sha256": hashlib.sha256(task.prompt.encode("utf-8")).hexdigest(),
        "started_at": started_at,
        "session_id": runtime.session_id, "pass": passed,
        "visible_pass": visible_pass,
        "hidden_pass": hidden_pass, "hidden_detail": hidden_detail[-1000:] if not hidden_pass else None,
        "hidden_test_sha256": hashlib.sha256(
            (HIDDEN_DIR / task.id / "test_hidden.py").read_bytes()).hexdigest(),
        "announced_complete": announced_complete,
        "false_complete": bool(announced_complete and not passed),
        "exit_reason": result.exit_reason.value if result else "infrastructure_error",
        "failures": failures, "error": error or (result.error if result else None),
        "seconds": elapsed, "rounds": result.rounds if result else runtime.rounds,
        "runtime_seconds": runtime_seconds,
        "usage": runtime.usage.model_dump(),
        "compactions": sum(e.type is EventType.CONTEXT_COMPACTED for e in events),
        "goal_checks": sum(e.type is EventType.GOAL_CHECK for e in events),
    }
    record_path.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    store.close()
    return record


def write_summary(output: Path, records: list[dict], model: str) -> None:
    records = sorted(records, key=lambda r: (r["task"], r["baseline"], r["repeat"]))

    def infrastructure(record: dict) -> bool:
        if record["exit_reason"] in {"infrastructure_error", "provider_error"}:
            return True
        usage = record["usage"]
        return (record["exit_reason"] == "time_budget"
                and usage["input_tokens"] + usage["output_tokens"] == 0)

    by_baseline = {}
    for baseline in ("b0", "b2"):
        subset = [r for r in records if r["baseline"] == baseline]
        if not subset:
            continue
        seconds = sorted(r["seconds"] for r in subset)
        by_baseline[baseline] = {
            "runs": len(subset), "passed": sum(bool(r["pass"]) for r in subset),
            "visible_passed": sum(bool(r.get("visible_pass", r["pass"])) for r in subset),
            "hidden_passed": sum(r.get("hidden_pass") is True for r in subset),
            "false_complete": sum(bool(r["false_complete"]) for r in subset),
            "infrastructure_errors": sum(infrastructure(r) for r in subset),
            "usage_unknown": sum((not r["usage"]["available"])
                                 or (infrastructure(r) and r["usage"]["input_tokens"]
                                     + r["usage"]["output_tokens"] == 0)
                                 for r in subset),
            "total_input_tokens": sum(r["usage"]["input_tokens"] for r in subset),
            "total_output_tokens": sum(r["usage"]["output_tokens"] for r in subset),
            "total_seconds": round(sum(r["seconds"] for r in subset), 3),
            "median_seconds": round(statistics.median(seconds), 3),
            "p95_seconds": round(seconds[math.ceil(.95 * len(seconds)) - 1], 3),
        }
    summary = {"generated_at": datetime.now(timezone.utc).isoformat(),
               "model": model, "seed_supported": False,
               "records": records, "by_baseline": by_baseline}
    target = output / "results.json"
    temporary = output / "results.json.tmp"
    temporary.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, target)
    lines = ["# Real model evaluation", "",
             f"Model: `{model}`. No seed support; every run starts from a clean fixture.",
             "Success requires a completed runtime, external command checks, protected files, allowed paths, and host-only held-out tests.",
             "Held-out tests are executed after the model run and are never copied into its workspace.",
             "Gateway cost is unknown unless a dated price table is supplied; token counts are recorded without an invented dollar value.",
             "A timeout before any model response is counted as an infrastructure/response timeout and remains in the end-to-end denominator.",
             "", "| Baseline | Final pass | Visible pass | Hidden pass | Runs | False complete | Infra/response timeout | Median s | P95 s | Input tokens | Output tokens |",
             "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for baseline, row in by_baseline.items():
        lines.append(f"| {baseline} | {row['passed']} | {row['visible_passed']} | "
                     f"{row['hidden_passed']} | {row['runs']} | {row['false_complete']} | "
                     f"{row['infrastructure_errors']} | {row['median_seconds']} | {row['p95_seconds']} | "
                     f"{row['total_input_tokens']} | {row['total_output_tokens']} |")
    lines.extend(["", "| Task | B0 passed/runs | B2 passed/runs |", "| --- | ---: | ---: |"])
    for task_id in sorted({r["task"] for r in records}):
        cells = []
        for baseline in ("b0", "b2"):
            subset = [r for r in records if r["task"] == task_id and r["baseline"] == baseline]
            cells.append(f"{sum(bool(r['pass']) for r in subset)}/{len(subset)}")
        lines.append(f"| {task_id} | {cells[0]} | {cells[1]} |")
    lines.extend(["", "Raw records and SQLite traces are under `runs/`.", ""])
    (output / "summary.md").write_text("\n".join(lines), encoding="utf-8")


async def main_async(args: argparse.Namespace) -> int:
    if discover_commandcode() is None:
        raise EvalError("CommandCode credentials are unavailable")
    ensure_python_on_path()
    tasks = discover_tasks(args.tasks_dir, args.task)
    baselines = args.baselines.split(",")
    if not baselines or any(b not in {"b0", "b2"} for b in baselines):
        raise EvalError("--baselines must contain only b0,b2")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source_files = _source_files(tasks)
    source_sha256 = _source_digest(source_files)
    snapshot_sha256 = _snapshot_sources(output, source_files)
    config = {
        "model": args.model, "provider": "commandcode", "temperature": None,
        "seed": None, "concurrency": args.concurrency, "repeats": args.repeats,
        "schedule": args.schedule,
        "baselines": baselines, "tasks": [task.id for task in tasks],
        "permission": "auto_allow", "task_budgets": {
            task.id: {"max_rounds": task.max_rounds, "max_tokens": task.max_tokens,
                      "max_seconds": task.max_seconds} for task in tasks},
        "git_revision": _git_revision(),
        "source_sha256": source_sha256,
        "source_snapshot_sha256": snapshot_sha256,
    }
    config_path = output / "config.json"
    if config_path.exists():
        previous = json.loads(config_path.read_text(encoding="utf-8"))
        if previous != config:
            raise EvalError(f"existing evaluation has different configuration: {config_path}")
    else:
        config_path.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    records = []
    semaphore = asyncio.Semaphore(args.concurrency)

    async def bounded(task: TaskDef, baseline: str, repeat: int) -> dict:
        async with semaphore:
            print(f"{task.id} {baseline} repeat {repeat}/{args.repeats}", flush=True)
            return await run_one(task, baseline, repeat, output, args.model)

    pending = []
    for task in tasks:
        if args.schedule == "grouped":
            pairs = [(baseline, repeat) for baseline in baselines
                     for repeat in range(1, args.repeats + 1)]
        else:
            pairs = [(baseline, repeat) for repeat in range(1, args.repeats + 1)
                     for baseline in (baselines if repeat % 2 else list(reversed(baselines)))]
        for baseline, repeat in pairs:
            pending.append(asyncio.create_task(bounded(task, baseline, repeat)))
    for finished in asyncio.as_completed(pending):
        record = await finished
        records.append(record)
        write_summary(output, records, args.model)
        print(f"  {record['task']} {record['baseline']}#{record['repeat']} "
              f"{'PASS' if record['pass'] else 'FAIL'} {record['exit_reason']} "
              f"{record['seconds']}s", flush=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks-dir", type=Path, default=DEFAULT_TASKS_DIR)
    parser.add_argument("--task", action="append")
    parser.add_argument("--baselines", default="b0,b2")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--schedule", choices=("alternating", "grouped"), default="alternating")
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    if args.concurrency < 1 or args.concurrency > 8:
        parser.error("--concurrency must be between 1 and 8")
    try:
        return asyncio.run(main_async(args))
    except EvalError as exc:
        parser.error(str(exc))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
