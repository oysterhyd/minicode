def sum_to_n(n: int) -> int:
    """Return 1 + 2 + ... + n (0 when n <= 0)."""
    total = 0
    for i in range(1, n):  # BUG: excludes n itself
        total += i
    return total
