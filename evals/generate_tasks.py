#!/usr/bin/env python3
"""Programmatically generate the 20 offline eval tasks under ``evals/tasks/``.

每个任务是一个带 bug 的微型仓库（``repo/``）+ 任务定义（``task.yaml``）+
FakeProvider 修复脚本（``script.json``）。源码与脚本出自同一份模板字典：
脚本里的 ``old_text`` 直接取自模板中标记的 bug 行，因此 ``apply_patch``
必然精确命中；``--verify`` 会在临时目录里对每个任务做"红→绿"自检
（修复前 pytest 有失败、修复后全部通过），不污染任务目录。

用法::

    python evals/generate_tasks.py            # 生成/覆盖 20 个任务目录
    python evals/generate_tasks.py --verify   # 生成后再跑红→绿自检
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

import yaml

TASKS_ROOT = Path(__file__).resolve().parent / "tasks"

BUDGET = {"max_rounds": 10, "max_tokens": 80000, "max_seconds": 120}
MAX_FIX_ATTEMPTS = 3
DEFAULT_INPUT_TOKENS = 120
DEFAULT_OUTPUT_TOKENS = 40

# ---------------------------------------------------------------------------
# 20 个任务模板。source 为带 bug 的原文件；old_text/new_text 是 apply_patch
# 的精确替换串；修复后的文件由 source.replace(old_text, new_text, 1) 推导，
# 生成器断言 old_text 在源文件中恰好出现一次。
# ---------------------------------------------------------------------------

TASKS: list[dict] = [
    {
        "id": "pagination_bounds",
        "bug_type": "分页边界",
        "module": "paginate",
        "symptom": "非末页会多返回一个元素，has_next 也随之错位",
        "fix_note": "把 end 从 start + page_size + 1 改为 start + page_size",
        "source": '''"""Pagination helpers."""


def slice_page(items: list, page: int = 1, page_size: int = 10) -> tuple[list, bool]:
    """Return (page_items, has_next); *page* is 1-based."""
    if page < 1:
        raise ValueError("page must be >= 1")
    start = (page - 1) * page_size
    end = start + page_size + 1  # BUG: off-by-one, yields one extra item
    return items[start:end], end < len(items)
''',
        "old_text": "    end = start + page_size + 1  # BUG: off-by-one, yields one extra item",
        "new_text": "    end = start + page_size",
        "tests": '''from paginate import slice_page


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
''',
    },
    {
        "id": "loop_off_by_one",
        "bug_type": "循环差一",
        "module": "sum_to_n",
        "symptom": "sum_to_n(n) 求和漏掉了 n 本身",
        "fix_note": "把 range(1, n) 改为 range(1, n + 1)",
        "source": '''def sum_to_n(n: int) -> int:
    """Return 1 + 2 + ... + n (0 when n <= 0)."""
    total = 0
    for i in range(1, n):  # BUG: excludes n itself
        total += i
    return total
''',
        "old_text": "    for i in range(1, n):  # BUG: excludes n itself",
        "new_text": "    for i in range(1, n + 1):",
        "tests": '''from sum_to_n import sum_to_n


def test_sum_to_one():
    assert sum_to_n(1) == 1


def test_sum_to_five():
    assert sum_to_n(5) == 15


def test_sum_to_ten():
    assert sum_to_n(10) == 55


def test_non_positive():
    assert sum_to_n(0) == 0
''',
    },
    {
        "id": "comparison_reversed",
        "bug_type": "比较运算符写反",
        "module": "in_range",
        "symptom": "in_range 对界内值返回 False、对界外值返回 True",
        "fix_note": "把 value < low or value > high 改为 low <= value <= high",
        "source": '''def in_range(value: int, low: int, high: int) -> bool:
    """Return True when low <= value <= high."""
    return value < low or value > high  # BUG: comparisons inverted
''',
        "old_text": "    return value < low or value > high  # BUG: comparisons inverted",
        "new_text": "    return low <= value <= high",
        "tests": '''from in_range import in_range


def test_inside():
    assert in_range(5, 1, 10) is True


def test_below_low():
    assert in_range(0, 1, 10) is False


def test_above_high():
    assert in_range(11, 1, 10) is False


def test_boundaries_inclusive():
    assert in_range(1, 1, 10) is True
    assert in_range(10, 1, 10) is True
''',
    },
    {
        "id": "string_format",
        "bug_type": "字符串格式化错误",
        "module": "greeting",
        "symptom": "display_name 把名和姓的顺序弄反了",
        "fix_note": "把 f\"{last} {first}\" 改为 f\"{first} {last}\"",
        "source": '''def display_name(first: str, last: str) -> str:
    """Return the display name as 'First Last'."""
    return f"{last} {first}"  # BUG: swapped first/last
''',
        "old_text": '    return f"{last} {first}"  # BUG: swapped first/last',
        "new_text": '    return f"{first} {last}"',
        "tests": '''from greeting import display_name


def test_english_name():
    assert display_name("Ada", "Lovelace") == "Ada Lovelace"


def test_single_last():
    assert display_name("Grace", "Hopper") == "Grace Hopper"


def test_keeps_spacing():
    assert display_name("A", "B") == "A B"
''',
    },
    {
        "id": "dict_key_typo",
        "bug_type": "dict 键名笔误",
        "module": "user_profile",
        "symptom": "to_card 读取的键名 nmae 是笔误，抛出 KeyError",
        "fix_note": "把 user[\"nmae\"] 改为 user[\"name\"]",
        "source": '''def to_card(user: dict) -> dict:
    """Build a display card from a raw user record."""
    return {
        "name": user["nmae"],  # BUG: typo, the real key is "name"
        "level": user["level"],
    }
''',
        "old_text": '        "name": user["nmae"],  # BUG: typo, the real key is "name"',
        "new_text": '        "name": user["name"],',
        "tests": '''from user_profile import to_card


def test_builds_card():
    user = {"name": "Ada", "level": 3}
    assert to_card(user) == {"name": "Ada", "level": 3}


def test_does_not_mutate_input():
    user = {"name": "Bob", "level": 1}
    to_card(user)
    assert user == {"name": "Bob", "level": 1}
''',
    },
    {
        "id": "division_guard",
        "bug_type": "除零缺保护",
        "module": "ratio",
        "symptom": "whole 为 0 时抛 ZeroDivisionError，约定应返回 0.0",
        "fix_note": "在除法前加 whole == 0 的保护分支",
        "source": '''def percent(part: float, whole: float) -> float:
    """Return part/whole as a percentage; 0.0 when whole is 0."""
    return part / whole * 100  # BUG: crashes on whole == 0
''',
        "old_text": "    return part / whole * 100  # BUG: crashes on whole == 0",
        "new_text": "    if whole == 0:\n        return 0.0\n    return part / whole * 100",
        "tests": '''from ratio import percent


def test_simple_ratio():
    assert percent(1, 4) == 25.0


def test_zero_whole_returns_zero():
    assert percent(0, 0) == 0.0


def test_zero_part():
    assert percent(0, 5) == 0.0
''',
    },
    {
        "id": "weekday_math",
        "bug_type": "日期星期计算错",
        "module": "weekday",
        "symptom": "weekday_name 返回的星期后移了一天，周日还会数组越界",
        "fix_note": "把下标 day.weekday() + 1 改为 day.weekday()",
        "source": '''"""Weekday helpers."""

from datetime import date

_NAMES = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]


def weekday_name(day: date) -> str:
    """Return the Chinese weekday name of *day* (Monday first)."""
    return _NAMES[day.weekday() + 1]  # BUG: off by one, Sunday overflows
''',
        "old_text": "    return _NAMES[day.weekday() + 1]  # BUG: off by one, Sunday overflows",
        "new_text": "    return _NAMES[day.weekday()]",
        "tests": '''from datetime import date

from weekday import weekday_name


def test_known_thursday():
    assert weekday_name(date(1970, 1, 1)) == "周四"


def test_known_saturday():
    assert weekday_name(date(1970, 1, 3)) == "周六"


def test_known_sunday():
    assert weekday_name(date(1970, 1, 4)) == "周日"
''',
    },
    {
        "id": "mutable_default",
        "bug_type": "可变默认参数",
        "module": "cart",
        "symptom": "两次独立调用共用同一个默认列表，第二次调用会带上第一次的商品",
        "fix_note": "把默认参数改为 None，函数体内再创建新列表",
        "source": '''def add_item(item: str, cart: list[str] = []) -> list[str]:
    """Append *item* to *cart* (a fresh cart by default) and return it."""
    cart.append(item)  # BUG: mutable default shared across calls
    return cart
''',
        "old_text": '''def add_item(item: str, cart: list[str] = []) -> list[str]:
    """Append *item* to *cart* (a fresh cart by default) and return it."""
    cart.append(item)  # BUG: mutable default shared across calls
    return cart''',
        "new_text": '''def add_item(item: str, cart: list[str] | None = None) -> list[str]:
    """Append *item* to *cart* (a fresh cart by default) and return it."""
    if cart is None:
        cart = []
    cart.append(item)
    return cart''',
        "tests": '''from cart import add_item


def test_each_call_starts_fresh():
    first = add_item("apple")
    second = add_item("pear")
    assert first == ["apple"]
    assert second == ["pear"]


def test_explicit_cart_is_reused():
    cart = ["milk"]
    assert add_item("bread", cart) == ["milk", "bread"]
''',
    },
    {
        "id": "sort_key",
        "bug_type": "排序 key 错",
        "module": "leaderboard",
        "symptom": "top_scores 按分数升序排列，约定应为分数最高的在前",
        "fix_note": "给 sorted 加上 reverse=True",
        "source": '''def top_scores(players: list[dict]) -> list[str]:
    """Return player names ordered by score, highest first."""
    ranked = sorted(players, key=lambda p: p["score"])  # BUG: ascending order
    return [p["name"] for p in ranked]
''',
        "old_text": '    ranked = sorted(players, key=lambda p: p["score"])  # BUG: ascending order',
        "new_text": '    ranked = sorted(players, key=lambda p: p["score"], reverse=True)',
        "tests": '''from leaderboard import top_scores


PLAYERS = [
    {"name": "ada", "score": 3},
    {"name": "bob", "score": 9},
    {"name": "cat", "score": 5},
]


def test_highest_first():
    assert top_scores(PLAYERS) == ["bob", "cat", "ada"]


def test_empty():
    assert top_scores([]) == []
''',
    },
    {
        "id": "missing_return",
        "bug_type": "缺 return",
        "module": "finder",
        "symptom": "first_even 总是返回 None，函数体里忘了把结果返回",
        "fix_note": "在循环结束后补上 return result",
        "source": '''def first_even(numbers: list[int]) -> int | None:
    """Return the first even number in *numbers*, or None if there is none."""
    result = None
    for n in numbers:
        if n % 2 == 0:
            result = n
            break
''',
        "old_text": '''    for n in numbers:
        if n % 2 == 0:
            result = n
            break''',
        "new_text": '''    for n in numbers:
        if n % 2 == 0:
            result = n
            break
    return result''',
        "tests": '''from finder import first_even


def test_finds_first_even():
    assert first_even([1, 3, 4, 6]) == 4


def test_no_even_returns_none():
    assert first_even([1, 3, 5]) is None


def test_empty_returns_none():
    assert first_even([]) is None
''',
    },
    {
        "id": "float_rounding",
        "bug_type": "浮点舍入",
        "module": "money",
        "symptom": "apply_discount 把金额四舍五入成了整数，约定保留两位小数",
        "fix_note": "给 round 加上第二参数 2",
        "source": '''def apply_discount(price: float, percent_off: float) -> float:
    """Return price after percent_off discount, rounded to 2 decimals."""
    return round(price * (1 - percent_off / 100))  # BUG: rounds to integer
''',
        "old_text": "    return round(price * (1 - percent_off / 100))  # BUG: rounds to integer",
        "new_text": "    return round(price * (1 - percent_off / 100), 2)",
        "tests": '''from money import apply_discount


def test_ten_percent_off():
    assert apply_discount(19.99, 10) == 17.99


def test_quarter_off():
    assert apply_discount(100.0, 25) == 75.0


def test_five_percent_off():
    assert apply_discount(9.99, 5) == 9.49
''',
    },
    {
        "id": "empty_list_bounds",
        "bug_type": "空列表越界",
        "module": "picker",
        "symptom": "空列表时 first_or_default 抛 IndexError，约定应返回 default",
        "fix_note": "改为 items[0] if items else default",
        "source": '''def first_or_default(items: list, default=None):
    """Return items[0], or *default* when the list is empty."""
    return items[0]  # BUG: IndexError on empty list
''',
        "old_text": "    return items[0]  # BUG: IndexError on empty list",
        "new_text": "    return items[0] if items else default",
        "tests": '''from picker import first_or_default


def test_empty_returns_default():
    assert first_or_default([], "x") == "x"


def test_empty_default_is_none():
    assert first_or_default([]) is None


def test_nonempty_returns_first():
    assert first_or_default([1, 2]) == 1
''',
    },
    {
        "id": "regex_mismatch",
        "bug_type": "正则写错",
        "module": "validator",
        "symptom": "is_valid_zip 对 6 位数字也返回 True，正则没有锚定",
        "fix_note": "把模式改为 ^\\d{5}$",
        "source": '''import re

_ZIP = re.compile(r"\\d{5}")  # BUG: unanchored, matches prefixes like "123456"


def is_valid_zip(code: str) -> bool:
    """Valid postal codes are exactly five digits."""
    return bool(_ZIP.match(code))
''',
        "old_text": '    return bool(_ZIP.match(code))',
        "new_text": '    return bool(_ZIP.fullmatch(code))',
        "tests": '''from validator import is_valid_zip


def test_five_digits_ok():
    assert is_valid_zip("12345") is True


def test_too_short():
    assert is_valid_zip("1234") is False


def test_too_long():
    assert is_valid_zip("123456") is False


def test_non_digit():
    assert is_valid_zip("12a45") is False
''',
    },
    {
        "id": "unit_conversion",
        "bug_type": "单位换算错",
        "module": "temperature",
        "symptom": "celsius_to_fahrenheit 的偏移量写成了 30，正确公式是 +32",
        "fix_note": "把 + 30 改为 + 32",
        "source": '''def celsius_to_fahrenheit(celsius: float) -> float:
    """Convert Celsius to Fahrenheit (F = C * 9 / 5 + 32)."""
    return celsius * 9 / 5 + 30  # BUG: offset should be 32
''',
        "old_text": "    return celsius * 9 / 5 + 30  # BUG: offset should be 32",
        "new_text": "    return celsius * 9 / 5 + 32",
        "tests": '''from temperature import celsius_to_fahrenheit


def test_freezing_point():
    assert celsius_to_fahrenheit(0) == 32.0


def test_boiling_point():
    assert celsius_to_fahrenheit(100) == 212.0


def test_body_temperature_scale():
    assert celsius_to_fahrenheit(40) == 104.0
''',
    },
    {
        "id": "dedup_logic",
        "bug_type": "去重逻辑错",
        "module": "dedup",
        "symptom": "unique_values 去重失效，重复元素仍然保留",
        "fix_note": "把 seen.add(str(value)) 改为 seen.add(value)",
        "source": '''def unique_values(values: list) -> list:
    """Drop duplicates, keeping first-seen order."""
    seen: set = set()
    result: list = []
    for value in values:
        if value in seen:
            continue
        result.append(value)
        seen.add(str(value))  # BUG: stores str(value), never matches value
    return result
''',
        "old_text": "        seen.add(str(value))  # BUG: stores str(value), never matches value",
        "new_text": "        seen.add(value)",
        "tests": '''from dedup import unique_values


def test_keeps_first_seen_order():
    assert unique_values([3, 1, 3, 2, 1]) == [3, 1, 2]


def test_empty():
    assert unique_values([]) == []


def test_strings():
    assert unique_values(["a", "a", "b"]) == ["a", "b"]
''',
    },
    {
        "id": "min_max_reversed",
        "bug_type": "min/max 写反",
        "module": "boundary",
        "symptom": "clamp 把 min/max 套反了，界内值也被推到边界外",
        "fix_note": "把 max(high, min(low, value)) 改为 min(high, max(low, value))",
        "source": '''def clamp(value: float, low: float, high: float) -> float:
    """Constrain *value* to the closed interval [low, high]."""
    return max(high, min(low, value))  # BUG: min/max swapped
''',
        "old_text": "    return max(high, min(low, value))  # BUG: min/max swapped",
        "new_text": "    return min(high, max(low, value))",
        "tests": '''from boundary import clamp


def test_inside_stays():
    assert clamp(5, 0, 10) == 5


def test_below_low():
    assert clamp(-5, 0, 10) == 0


def test_above_high():
    assert clamp(15, 0, 10) == 10
''',
    },
    {
        "id": "fib_seed",
        "bug_type": "斐波那契种子错",
        "module": "fib",
        "symptom": "fib 全部返回 0，序列种子 (a, b) 写成了 (0, 0)",
        "fix_note": "把种子 a, b = 0, 0 改为 a, b = 0, 1",
        "source": '''def fib(n: int) -> int:
    """Return the n-th Fibonacci number (fib(0)=0, fib(1)=1)."""
    a, b = 0, 0  # BUG: seed should be 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a
''',
        "old_text": "    a, b = 0, 0  # BUG: seed should be 0, 1",
        "new_text": "    a, b = 0, 1",
        "tests": '''from fib import fib


def test_zero():
    assert fib(0) == 0


def test_one():
    assert fib(1) == 1


def test_two():
    assert fib(2) == 1


def test_ten():
    assert fib(10) == 55
''',
    },
    {
        "id": "queue_fifo",
        "bug_type": "队列 FIFO 顺序错",
        "module": "job_queue",
        "symptom": "drain 从队尾弹出，顺序变成了 LIFO，约定应先进先出",
        "fix_note": "把 jobs.pop() 改为 jobs.pop(0)",
        "source": '''def drain(jobs: list[str]) -> list[str]:
    """Pop every job in FIFO order and return the order they ran."""
    order: list[str] = []
    while jobs:
        order.append(jobs.pop())  # BUG: pop() takes from the end (LIFO)
    return order
''',
        "old_text": "        order.append(jobs.pop())  # BUG: pop() takes from the end (LIFO)",
        "new_text": "        order.append(jobs.pop(0))",
        "tests": '''from job_queue import drain


def test_fifo_order():
    assert drain(["a", "b", "c"]) == ["a", "b", "c"]


def test_empty():
    assert drain([]) == []


def test_single():
    assert drain(["only"]) == ["only"]
''',
    },
    {
        "id": "slugify_unicode",
        "bug_type": "slugify 中文/unicode 处理错",
        "module": "slugify",
        "symptom": "中文字符被逐个替换成连字符，产生一串 '----' 而不是单个 '-'",
        "fix_note": "正则改为 [^a-z0-9]+（按连续片段折叠），并在结尾 strip('-')",
        "source": '''import re

_INVALID = re.compile(r"[^a-z0-9-]")  # BUG: char-by-char, CJK becomes "----"


def slugify(text: str) -> str:
    """Lowercase, map every disallowed character run to one hyphen."""
    lowered = text.lower()
    return _INVALID.sub("-", lowered)
''',
        "old_text": '''_INVALID = re.compile(r"[^a-z0-9-]")  # BUG: char-by-char, CJK becomes "----"


def slugify(text: str) -> str:
    """Lowercase, map every disallowed character run to one hyphen."""
    lowered = text.lower()
    return _INVALID.sub("-", lowered)''',
        "new_text": '''_INVALID = re.compile(r"[^a-z0-9]+")


def slugify(text: str) -> str:
    """Lowercase, map every disallowed character run to one hyphen."""
    lowered = text.lower()
    return _INVALID.sub("-", lowered).strip("-")''',
        "tests": '''from slugify import slugify


def test_ascii_words():
    assert slugify("Hello World") == "hello-world"


def test_cjk_collapses_to_one_hyphen():
    assert slugify("Hi 世界 OK") == "hi-ok"


def test_cjk_only_is_empty():
    assert slugify("世界") == ""


def test_repeats_collapse():
    assert slugify("A--B!") == "a-b"
''',
    },
    {
        "id": "cache_ttl",
        "bug_type": "缓存 TTL 判断错",
        "module": "cache_ttl",
        "symptom": "is_fresh 把判断写反了：过期后反而返回 True，TTL 内反而返回 False",
        "fix_note": "把 > entry[\"ttl\"] 改为 < entry[\"ttl\"]（过期时刻不算新鲜）",
        "source": '''def is_fresh(entry: dict, now: float) -> bool:
    """True while *entry* is still within its TTL at time *now*."""
    return now - entry["created_at"] > entry["ttl"]  # BUG: inverted, fresh after expiry
''',
        "old_text": '    return now - entry["created_at"] > entry["ttl"]  # BUG: inverted, fresh after expiry',
        "new_text": '    return now - entry["created_at"] < entry["ttl"]',
        "tests": '''from cache_ttl import is_fresh

ENTRY = {"value": "a", "created_at": 100.0, "ttl": 10.0}


def test_within_ttl_is_fresh():
    assert is_fresh(ENTRY, 105.0) is True


def test_at_expiry_is_stale():
    assert is_fresh(ENTRY, 110.0) is False


def test_after_expiry_is_stale():
    assert is_fresh(ENTRY, 111.0) is False
''',
    },
]


# ---------------------------------------------------------------------------
# 生成逻辑
# ---------------------------------------------------------------------------


def _fixed_source(spec: dict) -> str:
    source = spec["source"]
    old_text = spec["old_text"]
    count = source.count(old_text)
    if count != 1:
        raise SystemExit(
            f"[{spec['id']}] old_text 在源码中出现 {count} 次（应为 1 次）：{old_text!r}"
        )
    return source.replace(old_text, spec["new_text"], 1)


def _task_yaml(spec: dict) -> str:
    module = spec["module"]
    data = {
        "id": spec["id"],
        "title": f"{spec['bug_type']}：{spec['symptom']}",
        "prompt": (
            f"运行 `python -m pytest test_{module}.py -q` 有用例失败：{spec['symptom']}。\n"
            f"请阅读 {module}.py，定位并修复其中的 bug，使该测试文件的全部用例通过。\n"
            f"只允许修改 {module}.py，不要改动测试文件；完成后重新运行测试确认退出码为 0。\n"
        ),
        "allowed_paths": [f"{module}.py"],
        "protected_paths": [f"test_{module}.py"],
        "items": [
            {
                "id": "tests-pass",
                "type": "command",
                "command": f"python -m pytest test_{module}.py -q",
            },
            {
                "id": "tests-unchanged",
                "type": "protected",
                "path": f"test_{module}.py",
            },
        ],
        "budget": dict(BUDGET),
        "max_fix_attempts": MAX_FIX_ATTEMPTS,
        "script": "script.json",
    }
    return yaml.safe_dump(data, allow_unicode=True, sort_keys=False, width=88)


def _script_json(spec: dict) -> str:
    module = spec["module"]
    script = {
        "turns": [
            {"tool_calls": [{"name": "read_file", "arguments": {"path": f"{module}.py"}}]},
            {
                "tool_calls": [
                    {
                        "name": "apply_patch",
                        "arguments": {
                            "path": f"{module}.py",
                            "old_text": spec["old_text"],
                            "new_text": spec["new_text"],
                        },
                    }
                ]
            },
            {
                "tool_calls": [
                    {
                        "name": "run_command",
                        "arguments": {"command": f"python -m pytest test_{module}.py -q"},
                    }
                ]
            },
            {"text": f"修复完成：{spec['fix_note']}。重新运行 pytest，全部用例通过。"},
        ],
        "default_input_tokens": DEFAULT_INPUT_TOKENS,
        "default_output_tokens": DEFAULT_OUTPUT_TOKENS,
    }
    return json.dumps(script, ensure_ascii=False, indent=2) + "\n"


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8", newline="\n")


def generate() -> list[Path]:
    """Write every task directory; return the task dirs in generation order."""
    generated: list[Path] = []
    for spec in TASKS:
        task_dir = TASKS_ROOT / spec["id"]
        _write(task_dir / "repo" / f"{spec['module']}.py", spec["source"])
        _write(task_dir / "repo" / f"test_{spec['module']}.py", spec["tests"])
        _write(task_dir / "task.yaml", _task_yaml(spec))
        _write(task_dir / "script.json", _script_json(spec))
        # 静态一致性检查：每文件 <40 行、old_text 唯一、修复后内容确实变化。
        for name in (f"{spec['module']}.py", f"test_{spec['module']}.py"):
            lines = (task_dir / "repo" / name).read_text(encoding="utf-8").splitlines()
            if len(lines) >= 40:
                raise SystemExit(f"[{spec['id']}] {name} 超过 40 行（{len(lines)}）")
        if _fixed_source(spec) == spec["source"]:
            raise SystemExit(f"[{spec['id']}] new_text 未产生任何变更")
        generated.append(task_dir)
    return generated


def verify(generated: list[Path]) -> int:
    """Red→green check in temp copies: pytest must fail pre-patch, pass post-patch."""
    failures = 0
    for task_dir in generated:
        task_id = task_dir.name
        spec = next(s for s in TASKS if s["id"] == task_id)
        module = spec["module"]
        test_file = f"test_{module}.py"
        with tempfile.TemporaryDirectory(prefix=f"evalgen-{task_id}-") as tmp:
            tmp_path = Path(tmp)
            module_path = tmp_path / f"{module}.py"
            (tmp_path / test_file).write_text(spec["tests"], encoding="utf-8")

            module_path.write_text(spec["source"], encoding="utf-8", newline="\n")
            red = subprocess.run(
                [sys.executable, "-m", "pytest", test_file, "-q", "--no-header"],
                cwd=tmp_path, capture_output=True, timeout=120,
            )
            module_path.write_text(_fixed_source(spec), encoding="utf-8", newline="\n")
            green = subprocess.run(
                [sys.executable, "-m", "pytest", test_file, "-q", "--no-header"],
                cwd=tmp_path, capture_output=True, timeout=120,
            )
        red_summary = red.stdout.decode("utf-8", "replace").strip().splitlines()[-1:] or ["?"]
        green_summary = green.stdout.decode("utf-8", "replace").strip().splitlines()[-1:] or ["?"]
        ok = red.returncode != 0 and green.returncode == 0
        if not ok:
            failures += 1
        status = "OK " if ok else "BAD"
        print(f"[{status}] {task_id}: red={red.returncode} ({red_summary[0][:60]})"
              f" -> green={green.returncode} ({green_summary[0][:60]})")
    if failures:
        print(f"verify: {failures} 个任务红→绿自检失败")
        return 1
    print(f"verify: 全部 {len(generated)} 个任务红→绿自检通过")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", action="store_true", help="生成后做红→绿自检")
    args = parser.parse_args()
    generated = generate()
    print(f"generated {len(generated)} tasks under {TASKS_ROOT}")
    if args.verify:
        return verify(generated)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
