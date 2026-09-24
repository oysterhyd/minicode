"""Install the built wheel in an isolated venv and run bundled evaluation.

This is a CI command, not a pytest test: it intentionally exercises package
installation and a CLI invocation outside the source checkout.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import venv
from pathlib import Path


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    wheel_dir = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else root / "dist"
    wheels = list(wheel_dir.glob("minicode-*.whl"))
    if len(wheels) != 1:
        raise RuntimeError(f"expected one built wheel in {wheel_dir}, found {len(wheels)}")
    cache = root / ".pytest_cache"
    cache.mkdir(exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="wheel-smoke-", dir=cache)).resolve()
    venv_dir = scratch / "venv"
    venv.EnvBuilder(with_pip=True).create(venv_dir)
    python = venv_dir / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    subprocess.run([str(python), "-m", "pip", "install", f"{wheels[0]}[eval]"],
                   check=True, timeout=300)
    run_dir = scratch / "run"
    run_dir.mkdir()
    env = os.environ.copy()
    env.pop("PYTHONPATH", None)
    subprocess.run([str(python), "-m", "minicode", "eval", "--task", "pagination_bounds",
                    "--baselines", "b0,b2", "--output", "results"],
                   cwd=run_dir, env=env, check=True, timeout=180)
    results = json.loads((run_dir / "results" / "results.json").read_text(encoding="utf-8"))
    if results["run_count"] != 2 or results["passed"] != 2:
        raise RuntimeError(f"installed evaluation failed: {results['passed']}/{results['run_count']}")
    print(f"installed wheel smoke passed in {run_dir}")


if __name__ == "__main__":
    main()
