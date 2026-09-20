from sum_to_n import sum_to_n


def test_sum_to_one():
    assert sum_to_n(1) == 1


def test_sum_to_five():
    assert sum_to_n(5) == 15


def test_sum_to_ten():
    assert sum_to_n(10) == 55


def test_non_positive():
    assert sum_to_n(0) == 0
