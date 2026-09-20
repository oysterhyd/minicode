from picker import first_or_default


def test_empty_returns_default():
    assert first_or_default([], "x") == "x"


def test_empty_default_is_none():
    assert first_or_default([]) is None


def test_nonempty_returns_first():
    assert first_or_default([1, 2]) == 1
