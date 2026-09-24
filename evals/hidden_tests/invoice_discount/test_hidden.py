from invoice import total
from tax import tax_amount


def test_discounted_subtotal_and_input_unchanged():
    items = [40.0, 60.0]
    assert total(items, "CA", discount=20.0) == 86.0
    assert items == [40.0, 60.0]
    assert tax_amount(200.0, "CA") == 15.0
