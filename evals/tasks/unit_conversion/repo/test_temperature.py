from temperature import celsius_to_fahrenheit


def test_freezing_point():
    assert celsius_to_fahrenheit(0) == 32.0


def test_boiling_point():
    assert celsius_to_fahrenheit(100) == 212.0


def test_body_temperature_scale():
    assert celsius_to_fahrenheit(40) == 104.0
