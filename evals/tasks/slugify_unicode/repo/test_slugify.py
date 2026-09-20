from slugify import slugify


def test_ascii_words():
    assert slugify("Hello World") == "hello-world"


def test_cjk_collapses_to_one_hyphen():
    assert slugify("Hi 世界 OK") == "hi-ok"


def test_cjk_only_is_empty():
    assert slugify("世界") == ""


def test_repeats_collapse():
    assert slugify("A--B!") == "a-b"
