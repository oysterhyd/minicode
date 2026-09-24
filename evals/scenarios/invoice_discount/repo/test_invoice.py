from invoice import total
from tax import tax_amount


def test_discount_precedes_tax():
    assert total([100.0], "CA", discount=10.0) == 96.75


def test_no_tax_region():
    assert total([10.0, 20.0], "NONE", discount=5.0) == 25.0


def test_ca_tax_rate():
    assert tax_amount(100.0, "CA") == 7.5
