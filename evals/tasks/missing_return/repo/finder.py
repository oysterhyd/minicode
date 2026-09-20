def first_even(numbers: list[int]) -> int | None:
    """Return the first even number in *numbers*, or None if there is none."""
    result = None
    for n in numbers:
        if n % 2 == 0:
            result = n
            break
