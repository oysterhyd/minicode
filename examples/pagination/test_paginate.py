"""Tests for paginate.py.

While the off-by-one bug is present, ``test_exact_page_slice`` and
``test_second_page_slice`` intentionally fail (the slice returns one extra
item); the last page and the validation test pass either way. This gives the
minicode demo a red-to-green story.
"""

import pytest

from paginate import paginate


def test_exact_page_slice():
    # Page 1 must return exactly items 0..9, never one extra.
    assert paginate(list(range(25)), page=1, page_size=10) == (list(range(10)), True)


def test_second_page_slice():
    # Page 2 must return exactly items 10..19.
    assert paginate(list(range(25)), page=2, page_size=10) == (list(range(10, 20)), True)


def test_last_page():
    # Final page: only the remaining items, and has_next is False.
    assert paginate(list(range(25)), page=3, page_size=10) == (list(range(20, 25)), False)


def test_invalid_page():
    with pytest.raises(ValueError):
        paginate(list(range(25)), page=0, page_size=10)
