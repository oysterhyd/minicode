# Mini Claude Code 本地评测结果

- 生成时间：2026-09-23T16:09:57+00:00
- 基线：b0、b1、b2
- 运行数：60（20 个任务 × 3 个基线）

## 基线对照

| 基线 | 通过 | 平均轮数 | 平均输入 token | 平均输出 token | 压缩次数 | 平均耗时(s) |
|---|---|---|---|---|---|---|
| b0 | 20/20（100%） | 4.0 | 480 | 160 | 0 | 2.5 |
| b1 | 20/20（100%） | 4.0 | 480 | 160 | 0 | 2.1 |
| b2 | 20/20（100%） | 4.0 | 480 | 160 | 0 | 2.1 |

## 逐任务结果

| 任务 | 标题 | b0 | b1 | b2 |
|---|---|---|---|---|
| cache_ttl | 缓存 TTL 判断错：is_fresh 把判断写反了：过期后反而返回 True，TTL 内反而返回 False | ✓ | ✓ | ✓ |
| comparison_reversed | 比较运算符写反：in_range 对界内值返回 False、对界外值返回 True | ✓ | ✓ | ✓ |
| dedup_logic | 去重逻辑错：unique_values 去重失效，重复元素仍然保留 | ✓ | ✓ | ✓ |
| dict_key_typo | dict 键名笔误：to_card 读取的键名 nmae 是笔误，抛出 KeyError | ✓ | ✓ | ✓ |
| division_guard | 除零缺保护：whole 为 0 时抛 ZeroDivisionError，约定应返回 0.0 | ✓ | ✓ | ✓ |
| empty_list_bounds | 空列表越界：空列表时 first_or_default 抛 IndexError，约定应返回 default | ✓ | ✓ | ✓ |
| fib_seed | 斐波那契种子错：fib 全部返回 0，序列种子 (a, b) 写成了 (0, 0) | ✓ | ✓ | ✓ |
| float_rounding | 浮点舍入：apply_discount 把金额四舍五入成了整数，约定保留两位小数 | ✓ | ✓ | ✓ |
| loop_off_by_one | 循环差一：sum_to_n(n) 求和漏掉了 n 本身 | ✓ | ✓ | ✓ |
| min_max_reversed | min/max 写反：clamp 把 min/max 套反了，界内值也被推到边界外 | ✓ | ✓ | ✓ |
| missing_return | 缺 return：first_even 总是返回 None，函数体里忘了把结果返回 | ✓ | ✓ | ✓ |
| mutable_default | 可变默认参数：两次独立调用共用同一个默认列表，第二次调用会带上第一次的商品 | ✓ | ✓ | ✓ |
| pagination_bounds | 分页边界：非末页会多返回一个元素，has_next 也随之错位 | ✓ | ✓ | ✓ |
| queue_fifo | 队列 FIFO 顺序错：drain 从队尾弹出，顺序变成了 LIFO，约定应先进先出 | ✓ | ✓ | ✓ |
| regex_mismatch | 正则写错：is_valid_zip 对 6 位数字也返回 True，正则没有锚定 | ✓ | ✓ | ✓ |
| slugify_unicode | slugify 中文/unicode 处理错：中文字符被逐个替换成连字符，产生一串 '----' 而不是单个 '-' | ✓ | ✓ | ✓ |
| sort_key | 排序 key 错：top_scores 按分数升序排列，约定应为分数最高的在前 | ✓ | ✓ | ✓ |
| string_format | 字符串格式化错误：display_name 把名和姓的顺序弄反了 | ✓ | ✓ | ✓ |
| unit_conversion | 单位换算错：celsius_to_fahrenheit 的偏移量写成了 30，正确公式是 +32 | ✓ | ✓ | ✓ |
| weekday_math | 日期星期计算错：weekday_name 返回的星期后移了一天，周日还会数组越界 | ✓ | ✓ | ✓ |

## 失败任务与退出原因

无 —— 全部运行通过验收。

## 失败分析

本次运行全部通过。b2 平均轮数 4.0 与 b0 平均轮数 4.0 一致：FakeProvider 脚本保证一次修复即可通过验收，b2 的续跑分支（验收失败 → 追加一轮 run_turn）在全部通过时不会被触发，两者的差异只在验收失败时出现。
成功率反映的是 harness 机制（脚本重放、预算、验收、续跑）是否按设计工作，而不是模型解决问题的能力 —— 见 README 的诚实说明。
