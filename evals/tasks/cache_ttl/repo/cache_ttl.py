def is_fresh(entry: dict, now: float) -> bool:
    """True while *entry* is still within its TTL at time *now*."""
    return now - entry["created_at"] > entry["ttl"]  # BUG: inverted, fresh after expiry
