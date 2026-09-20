def celsius_to_fahrenheit(celsius: float) -> float:
    """Convert Celsius to Fahrenheit (F = C * 9 / 5 + 32)."""
    return celsius * 9 / 5 + 30  # BUG: offset should be 32
