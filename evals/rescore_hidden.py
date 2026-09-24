#!/usr/bin/env python3
"""Add host-side held-out scoring to saved real-model workspaces without API calls."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path

from evals.hidden import HIDDEN_DIR, check_hidden
from evals.run_real_eval import write_summary


def _atomic_json(path: Path, value: dict) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


async def rescore(output: Path) -> dict:
    output = output.resolve()
    results_path = output / "results.json"
    original = results_path.read_bytes()
    results = json.loads(original)
    records = results["records"]
    for record in records:
        task_id = record["task"]
        test_path = HIDDEN_DIR / task_id / "test_hidden.py"
        run_dir = output / "runs" / task_id / record["baseline"] / str(record["repeat"])
        workspace = run_dir / "workspace"
        if not workspace.is_dir() or not (run_dir / "record.json").is_file():
            raise ValueError(f"missing saved workspace or record: {run_dir}")
        hidden_pass, detail = await check_hidden(task_id, workspace)
        record["visible_pass"] = record.get("visible_pass", record["pass"])
        record["hidden_pass"] = hidden_pass
        record["hidden_detail"] = detail[-1000:] if not hidden_pass else None
        record["hidden_test_sha256"] = hashlib.sha256(test_path.read_bytes()).hexdigest()
        record["pass"] = bool(record["visible_pass"] and hidden_pass)
        record["false_complete"] = bool(record["announced_complete"] and not record["pass"])
        record["failures"] = [f for f in record.get("failures", [])
                              if not f.startswith("[hidden]")]
        if not hidden_pass:
            record["failures"].append("[hidden] host-side acceptance failed")
        _atomic_json(run_dir / "record.json", record)
    write_summary(output, records, results["model"])
    metadata = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "source_results_sha256": hashlib.sha256(original).hexdigest(),
        "record_count": len(records),
        "hidden_suite_sha256": hashlib.sha256("\n".join(
            f"{path.parent.name}:{hashlib.sha256(path.read_bytes()).hexdigest()}"
            for path in sorted(HIDDEN_DIR.glob("*/test_hidden.py"))
        ).encode("utf-8")).hexdigest(),
        "scoring": "visible checks retained; held-out tests run only after model execution",
    }
    _atomic_json(output / "hidden-rescore.json", metadata)
    return metadata


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(asyncio.run(rescore(args.output)), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
