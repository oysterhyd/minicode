"""Tests for Usage: cumulative totals, cache accounting and hit rate."""

from __future__ import annotations

from minicode.core.models import Usage


def test_cache_hit_rate_zero_without_cache_or_input():
    assert Usage().cache_hit_rate == 0.0
    assert Usage(input_tokens=0, output_tokens=5).cache_hit_rate == 0.0


def test_cache_hit_rate_is_cached_share_of_prompt():
    usage = Usage(input_tokens=1000, output_tokens=100, cache_read_tokens=400)
    assert usage.cache_hit_rate == 0.4


def test_add_sums_cache_tokens_and_propagates_unknown():
    total = Usage(
        input_tokens=100,
        output_tokens=10,
        cache_read_tokens=30,
        cache_write_tokens=5,
    ) + Usage(
        input_tokens=200,
        output_tokens=20,
        cache_read_tokens=60,
        cache_write_tokens=7,
        available=False,
    )
    assert total.input_tokens == 300
    assert total.output_tokens == 30
    assert total.cache_read_tokens == 90
    assert total.cache_write_tokens == 12
    assert total.available is False
    assert total.cache_hit_rate == 0.3


def test_total_tokens_excludes_cache_field():
    usage = Usage(input_tokens=500, output_tokens=50, cache_read_tokens=200)
    assert usage.total_tokens == 550


def test_missing_usage_is_distinct_from_reported_zero():
    assert Usage().available is True
    assert Usage(available=False).available is False
