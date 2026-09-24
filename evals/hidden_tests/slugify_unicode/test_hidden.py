from slugify import slugify


def test_underscore_and_hyphen_runs():
    assert slugify("  Foo___Bar  ") == "foo-bar"
    assert slugify("A--B--C") == "a-b-c"
    assert slugify("é") == ""
