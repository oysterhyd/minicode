def top_scores(players: list[dict]) -> list[str]:
    """Return player names ordered by score, highest first."""
    ranked = sorted(players, key=lambda p: p["score"])  # BUG: ascending order
    return [p["name"] for p in ranked]
