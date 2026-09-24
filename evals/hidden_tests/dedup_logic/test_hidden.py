from dedup import unique_values


def test_mixed_types_and_repeated_numbers():
    assert unique_values([1, "1", 1, "1", 2]) == [1, "1", 2]
    assert unique_values([0, 0, -1, -1]) == [0, -1]
