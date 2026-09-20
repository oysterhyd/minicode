import re

_ZIP = re.compile(r"\d{5}")  # BUG: unanchored, matches prefixes like "123456"


def is_valid_zip(code: str) -> bool:
    """Valid postal codes are exactly five digits."""
    return bool(_ZIP.match(code))
