from money import apply_discount


def test_fractional_prices_keep_cents():
    assert apply_discount(1.23, 0) == 1.23
    assert apply_discount(12.34, 50) == 6.17
