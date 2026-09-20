from job_queue import drain


def test_fifo_order():
    assert drain(["a", "b", "c"]) == ["a", "b", "c"]


def test_empty():
    assert drain([]) == []


def test_single():
    assert drain(["only"]) == ["only"]
