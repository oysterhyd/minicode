import re

_INVALID = re.compile(r"[^a-z0-9-]")  # BUG: char-by-char, CJK becomes "----"


def slugify(text: str) -> str:
    """Lowercase, map every disallowed character run to one hyphen."""
    lowered = text.lower()
    return _INVALID.sub("-", lowered)
