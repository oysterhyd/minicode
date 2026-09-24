from in_range import in_range


def test_single_point_and_negative_interval():
    assert in_range(7, 7, 7) is True
    assert in_range(6, 7, 7) is False
    assert in_range(-3, -5, -1) is True
