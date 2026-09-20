from boundary import clamp


def test_inside_stays():
    assert clamp(5, 0, 10) == 5


def test_below_low():
    assert clamp(-5, 0, 10) == 0


def test_above_high():
    assert clamp(15, 0, 10) == 10
