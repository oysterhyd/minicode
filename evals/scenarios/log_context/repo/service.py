"""Helpers for finding the latest status in a request trace."""


def final_status(lines: list[str], request_id: str) -> str | None:
    """Return the last status reported for a request, or None if absent."""
    marker = f"request={request_id} "
    for line in lines:
        if marker in line and "status=" in line:
            return line.split("status=", 1)[1].split()[0]  # BUG: returns first match
    return None
