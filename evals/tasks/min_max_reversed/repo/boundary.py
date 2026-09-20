def clamp(value: float, low: float, high: float) -> float:
    """Constrain *value* to the closed interval [low, high]."""
    return max(high, min(low, value))  # BUG: min/max swapped
