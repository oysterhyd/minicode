from fib import fib


def test_medium_terms():
    assert fib(3) == 2
    assert fib(7) == 13
    assert fib(20) == 6765
