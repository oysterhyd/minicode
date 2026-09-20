def apply_discount(price: float, percent_off: float) -> float:
    """Return price after percent_off discount, rounded to 2 decimals."""
    return round(price * (1 - percent_off / 100))  # BUG: rounds to integer
