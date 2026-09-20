from cart import add_item


def test_each_call_starts_fresh():
    first = add_item("apple")
    second = add_item("pear")
    assert first == ["apple"]
    assert second == ["pear"]


def test_explicit_cart_is_reused():
    cart = ["milk"]
    assert add_item("bread", cart) == ["milk", "bread"]
