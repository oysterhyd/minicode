from job_queue import drain


def test_fifo_with_repeated_names_and_drains_input():
    jobs = ["first", "middle", "first", "last"]
    assert drain(jobs) == ["first", "middle", "first", "last"]
    assert jobs == []
