from dedup import unique_values


def test_keeps_first_seen_order():
    assert unique_values([3, 1, 3, 2, 1]) == [3, 1, 2]


def test_empty():
    assert unique_values([]) == []


def test_strings():
    assert unique_values(["a", "a", "b"]) == ["a", "b"]
