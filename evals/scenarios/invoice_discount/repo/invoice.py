"""Invoice totals composed from pricing and tax modules."""

from tax import tax_amount


def total(line_items: list[float], region: str, discount: float = 0.0) -> float:
    subtotal = sum(line_items)
    return round(subtotal + tax_amount(subtotal, region) - discount, 2)  # BUG: discount after tax
