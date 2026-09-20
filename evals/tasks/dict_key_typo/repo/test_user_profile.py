from user_profile import to_card


def test_builds_card():
    user = {"name": "Ada", "level": 3}
    assert to_card(user) == {"name": "Ada", "level": 3}


def test_does_not_mutate_input():
    user = {"name": "Bob", "level": 1}
    to_card(user)
    assert user == {"name": "Bob", "level": 1}
