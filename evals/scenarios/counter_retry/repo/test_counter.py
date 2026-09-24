from counter import increment


def test_increment_positive():
    assert increment(4) == 5


def test_increment_zero():
    assert increment(0) == 1
