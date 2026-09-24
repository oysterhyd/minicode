from finder import first_even


def test_first_even_at_front_or_later():
    assert first_even([0, 2, 4]) == 0
    assert first_even([-3, -2, 8]) == -2
