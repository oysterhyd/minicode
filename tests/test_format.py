"""Tests for the compact terminal formatting of token counts."""

from __future__ import annotations

from minicode.core.format import format_tokens


def test_small_values_stay_exact():
    assert format_tokens(0) == "0"
    assert format_tokens(950) == "950"


def test_thousands_use_k():
    assert format_tokens(1_000) == "1k"
    assert format_tokens(1_500) == "1.5k"
    assert format_tokens(12_345) == "12.3k"
    assert format_tokens(300_000) == "300k"
    assert format_tokens(383_009) == "383k"


def test_large_values_use_m_and_never_overflow_the_unit():
    assert format_tokens(1_000_000) == "1M"
    assert format_tokens(1_048_576) == "1M"
    # Would round to 1000.0k at one decimal: promoted instead.
    assert format_tokens(999_999) == "1M"
    assert format_tokens(999_949) == "999.9k"


def test_negative_values_keep_their_sign():
    assert format_tokens(-5_000) == "-5k"
