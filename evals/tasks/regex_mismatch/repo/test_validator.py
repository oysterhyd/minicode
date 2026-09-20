from validator import is_valid_zip


def test_five_digits_ok():
    assert is_valid_zip("12345") is True


def test_too_short():
    assert is_valid_zip("1234") is False


def test_too_long():
    assert is_valid_zip("123456") is False


def test_non_digit():
    assert is_valid_zip("12a45") is False
