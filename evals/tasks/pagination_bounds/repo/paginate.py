"""Pagination helpers."""


def slice_page(items: list, page: int = 1, page_size: int = 10) -> tuple[list, bool]:
    """Return (page_items, has_next); *page* is 1-based."""
    if page < 1:
        raise ValueError("page must be >= 1")
    start = (page - 1) * page_size
    end = start + page_size + 1  # BUG: off-by-one, yields one extra item
    return items[start:end], end < len(items)
