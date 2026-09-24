import pytest

from temperature import celsius_to_fahrenheit


def test_negative_and_fractional_temperature():
    assert celsius_to_fahrenheit(-40) == -40.0
    assert celsius_to_fahrenheit(37) == pytest.approx(98.6)
