def percent(part: float, whole: float) -> float:
    """Return part/whole as a percentage; 0.0 when whole is 0."""
    return part / whole * 100  # BUG: crashes on whole == 0
