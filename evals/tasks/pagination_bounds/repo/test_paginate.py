from paginate import slice_page


def test_first_page_is_exact():
    assert slice_page(list(range(25)), page=1, page_size=10) == (list(range(10)), True)


def test_second_page_is_exact():
    assert slice_page(list(range(25)), page=2, page_size=10) == (list(range(10, 20)), True)


def test_last_page_has_no_next():
    assert slice_page(list(range(25)), page=3, page_size=10) == (list(range(20, 25)), False)


def test_invalid_page():
    import pytest

    with pytest.raises(ValueError):
        slice_page(list(range(25)), page=0)
