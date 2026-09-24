from leaderboard import top_scores


def test_ties_keep_original_order():
    players = [
        {"name": "ada", "score": 1},
        {"name": "bob", "score": 1},
        {"name": "cat", "score": 2},
    ]
    assert top_scores(players) == ["cat", "ada", "bob"]
