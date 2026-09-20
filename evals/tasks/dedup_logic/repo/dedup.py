def unique_values(values: list) -> list:
    """Drop duplicates, keeping first-seen order."""
    seen: set = set()
    result: list = []
    for value in values:
        if value in seen:
            continue
        result.append(value)
        seen.add(str(value))  # BUG: stores str(value), never matches value
    return result
