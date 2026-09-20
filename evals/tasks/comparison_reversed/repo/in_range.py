def in_range(value: int, low: int, high: int) -> bool:
    """Return True when low <= value <= high."""
    return value < low or value > high  # BUG: comparisons inverted
