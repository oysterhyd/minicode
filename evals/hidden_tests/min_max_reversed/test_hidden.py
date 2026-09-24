from boundary import clamp


def test_equal_bound_and_negative_range():
    assert clamp(3, 3, 3) == 3
    assert clamp(-10, -5, -1) == -5
    assert clamp(-3, -5, -1) == -3
