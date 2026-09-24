from sum_to_n import sum_to_n


def test_small_large_and_negative():
    assert sum_to_n(2) == 3
    assert sum_to_n(100) == 5050
    assert sum_to_n(-5) == 0
