from leaderboard import top_scores


PLAYERS = [
    {"name": "ada", "score": 3},
    {"name": "bob", "score": 9},
    {"name": "cat", "score": 5},
]


def test_highest_first():
    assert top_scores(PLAYERS) == ["bob", "cat", "ada"]


def test_empty():
    assert top_scores([]) == []
