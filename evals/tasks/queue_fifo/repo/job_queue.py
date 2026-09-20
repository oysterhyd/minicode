def drain(jobs: list[str]) -> list[str]:
    """Pop every job in FIFO order and return the order they ran."""
    order: list[str] = []
    while jobs:
        order.append(jobs.pop())  # BUG: pop() takes from the end (LIFO)
    return order
