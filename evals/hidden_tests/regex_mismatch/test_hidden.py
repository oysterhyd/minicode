from validator import is_valid_zip


def test_whitespace_and_newline_not_part_of_zip():
    assert is_valid_zip("00000") is True
    assert is_valid_zip(" 12345") is False
    assert is_valid_zip("12345 ") is False
    assert is_valid_zip("12345\n") is False
