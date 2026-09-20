def fib(n: int) -> int:
    """Return the n-th Fibonacci number (fib(0)=0, fib(1)=1)."""
    a, b = 0, 0  # BUG: seed should be 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a
