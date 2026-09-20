from in_range import in_range


def test_inside():
    assert in_range(5, 1, 10) is True


def test_below_low():
    assert in_range(0, 1, 10) is False


def test_above_high():
    assert in_range(11, 1, 10) is False


def test_boundaries_inclusive():
    assert in_range(1, 1, 10) is True
    assert in_range(10, 1, 10) is True
