def first_or_default(items: list, default=None):
    """Return items[0], or *default* when the list is empty."""
    return items[0]  # BUG: IndexError on empty list
