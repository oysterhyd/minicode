def add_item(item: str, cart: list[str] = []) -> list[str]:
    """Append *item* to *cart* (a fresh cart by default) and return it."""
    cart.append(item)  # BUG: mutable default shared across calls
    return cart
