from greeting import display_name


def test_unicode_and_hyphenated_names():
    assert display_name("李", "雷") == "李 雷"
    assert display_name("Anne-Marie", "Smith") == "Anne-Marie Smith"
