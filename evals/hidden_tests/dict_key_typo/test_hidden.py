from user_profile import to_card


def test_unicode_name_and_extra_input_fields():
    user = {"name": "李", "level": 0, "private": "keep"}
    assert to_card(user) == {"name": "李", "level": 0}
    assert user["private"] == "keep"
