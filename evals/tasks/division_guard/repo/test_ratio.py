from ratio import percent


def test_simple_ratio():
    assert percent(1, 4) == 25.0


def test_zero_whole_returns_zero():
    assert percent(0, 0) == 0.0


def test_zero_part():
    assert percent(0, 5) == 0.0
