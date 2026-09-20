from greeting import display_name


def test_english_name():
    assert display_name("Ada", "Lovelace") == "Ada Lovelace"


def test_single_last():
    assert display_name("Grace", "Hopper") == "Grace Hopper"


def test_keeps_spacing():
    assert display_name("A", "B") == "A B"
