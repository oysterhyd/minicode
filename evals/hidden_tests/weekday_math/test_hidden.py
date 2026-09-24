from datetime import date

from weekday import weekday_name


def test_monday_and_leap_day():
    assert weekday_name(date(2024, 1, 1)) == "周一"
    assert weekday_name(date(2024, 2, 29)) == "周四"
