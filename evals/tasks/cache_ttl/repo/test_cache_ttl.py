from cache_ttl import is_fresh

ENTRY = {"value": "a", "created_at": 100.0, "ttl": 10.0}


def test_within_ttl_is_fresh():
    assert is_fresh(ENTRY, 105.0) is True


def test_at_expiry_is_stale():
    assert is_fresh(ENTRY, 110.0) is False


def test_after_expiry_is_stale():
    assert is_fresh(ENTRY, 111.0) is False
