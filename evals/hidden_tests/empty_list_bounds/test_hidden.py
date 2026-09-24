from picker import first_or_default


def test_falsey_defaults_and_first_item():
    assert first_or_default([], 0) == 0
    assert first_or_default([], False) is False
    assert first_or_default([None, "later"], "fallback") is None
