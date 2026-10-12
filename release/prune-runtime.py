"""Remove TUI development metadata only, retaining runtime code and licenses."""
from __future__ import annotations

import argparse
from pathlib import Path


def prune(runtime: Path) -> tuple[int, int]:
    root = (runtime / "tui" / "node_modules").resolve()
    if not root.is_relative_to(runtime.resolve()):
        raise ValueError("TUI dependencies must stay inside release runtime")
    count = size = 0
    for path in root.rglob("*"):
        if path.is_file() and path.name.endswith((".d.ts", ".d.cts", ".d.mts", ".map")):
            if not path.resolve().is_relative_to(root):
                raise ValueError("dependency metadata must stay inside TUI dependencies")
            size += path.stat().st_size
            path.unlink()
            count += 1
    return count, size


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("runtime", type=Path)
    args = parser.parse_args()
    count, size = prune(args.runtime)
    print(f"Removed {count} TUI declaration/source-map files ({size} bytes); runtime and licenses retained.")
