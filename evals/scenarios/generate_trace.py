"""Regenerate the deterministic 1600-line log_context fixture."""

from pathlib import Path


def main() -> None:
    path = Path(__file__).resolve().parent / "log_context" / "repo" / "trace.log"
    lines = []
    for index in range(1600):
        if index == 100:
            status = "request=R-17 status=started"
        elif index == 900:
            status = "request=R-17 status=retrying"
        elif index == 1510:
            status = "request=R-17 status=completed"
        else:
            status = f"request=R-{100 + index % 90} status=ok"
        lines.append(f"2026-09-23T12:{index // 60:02d}:{index % 60:02d}Z {status} "
                     f"payload={'x' * 64}\n")
    path.write_text("".join(lines), encoding="utf-8")
    print(f"{path}: {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
