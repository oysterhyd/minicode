from counter import increment


def test_negative_and_zero():
    assert increment(-1) == 0
    assert increment(0) == 1
