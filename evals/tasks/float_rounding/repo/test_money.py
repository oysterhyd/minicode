from money import apply_discount


def test_ten_percent_off():
    assert apply_discount(19.99, 10) == 17.99


def test_quarter_off():
    assert apply_discount(100.0, 25) == 75.0


def test_five_percent_off():
    assert apply_discount(9.99, 5) == 9.49
