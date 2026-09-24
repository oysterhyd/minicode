from ratio import percent


def test_zero_denominator_with_nonzero_part_and_negative_ratio():
    assert percent(9, 0) == 0.0
    assert percent(-1, 2) == -50.0
