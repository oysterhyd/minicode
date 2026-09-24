from cart import add_item


def test_three_fresh_carts_and_explicit_identity():
    first = add_item("one")
    second = add_item("two")
    third = add_item("three")
    assert (first, second, third) == (["one"], ["two"], ["three"])
    existing = ["old"]
    assert add_item("new", existing) is existing
