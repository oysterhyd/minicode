from cache_ttl import is_fresh


def test_near_expiry_and_zero_ttl():
    entry = {"created_at": 20.0, "ttl": 5.0}
    assert is_fresh(entry, 24.999) is True
    assert is_fresh(entry, 25.0) is False
    assert is_fresh({"created_at": 20.0, "ttl": 0.0}, 20.0) is False
