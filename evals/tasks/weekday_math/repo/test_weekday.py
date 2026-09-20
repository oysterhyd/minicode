from datetime import date

from weekday import weekday_name


def test_known_thursday():
    assert weekday_name(date(1970, 1, 1)) == "周四"


def test_known_saturday():
    assert weekday_name(date(1970, 1, 3)) == "周六"


def test_known_sunday():
    assert weekday_name(date(1970, 1, 4)) == "周日"
