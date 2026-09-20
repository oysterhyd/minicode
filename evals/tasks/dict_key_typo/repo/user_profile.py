def to_card(user: dict) -> dict:
    """Build a display card from a raw user record."""
    return {
        "name": user["nmae"],  # BUG: typo, the real key is "name"
        "level": user["level"],
    }
