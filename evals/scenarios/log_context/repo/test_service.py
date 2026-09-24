from pathlib import Path

from service import final_status


def test_last_status_in_long_trace():
    lines = Path("trace.log").read_text(encoding="utf-8").splitlines()
    assert final_status(lines, "R-17") == "completed"


def test_missing_request():
    assert final_status(["request=A status=ok"], "B") is None
