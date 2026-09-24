from service import final_status


def test_latest_status_and_no_match():
    lines = [
        "request=R-17 status=queued",
        "request=OTHER status=failed",
        "request=R-17 status=retrying",
        "request=R-17 status=done",
    ]
    assert final_status(lines, "R-17") == "done"
    assert final_status(lines, "MISSING") is None
