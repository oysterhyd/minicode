"""Tax rates for the invoice example."""

_RATES = {"CA": 0.07, "NONE": 0.0}  # BUG: CA is 7.5%


def tax_amount(taxable: float, region: str) -> float:
    return taxable * _RATES[region]
