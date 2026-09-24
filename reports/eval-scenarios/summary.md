# Mini Claude Code 本地评测结果

- 生成时间：2026-09-23T16:14:57+00:00
- 基线：b0、b1、b2
- 运行数：9（3 个任务 × 3 个基线）

## 基线对照

| 基线 | 通过 | 平均轮数 | 平均输入 token | 平均输出 token | 压缩次数 | 平均耗时(s) |
|---|---|---|---|---|---|---|
| b0 | 2/3（67%） | 4.7 | 727 | 220 | 0 | 2.5 |
| b1 | 2/3（67%） | 4.7 | 727 | 220 | 0 | 2.1 |
| b2 | 3/3（100%） | 6.0 | 887 | 273 | 0 | 2.6 |

## 逐任务结果

| 任务 | 标题 | b0 | b1 | b2 |
|---|---|---|---|---|
| counter_retry | 错误修复后验收失败再续跑 | ✗（completed） | ✗（completed） | ✓ |
| invoice_discount | 跨文件发票折扣与税额计算 | ✓ | ✓ | ✓ |
| log_context | 长日志中的同一请求应返回最后状态 | ✓ | ✓ | ✓ |

## 失败任务与退出原因

- counter_retry × b0：exit_reason=completed（attempts=1, rounds=4）；[tests-pass] 命令 `python -m pytest test_counter.py -q` 退出码 1; [hidden] host-side acceptance failed: ============================
___________________________ test_negative_and_zero ____________________________

    def test_negative_and_zero():
>       assert increment(-1) == 0
E       assert -1 == 0
E        +  where -1 = increment(-1)

D:\miniclaudecode\evals\hidden_tests\counter_retry\test_hidden.py:5: AssertionError
=========================== short test summary info ===========================
FAILED D:\miniclaudecode\evals\hidden_tests\counter_retry\test_hidden.py::test_negative_and_zero

- counter_retry × b1：exit_reason=completed（attempts=1, rounds=4）；[tests-pass] 命令 `python -m pytest test_counter.py -q` 退出码 1; [hidden] host-side acceptance failed: ============================
___________________________ test_negative_and_zero ____________________________

    def test_negative_and_zero():
>       assert increment(-1) == 0
E       assert -1 == 0
E        +  where -1 = increment(-1)

D:\miniclaudecode\evals\hidden_tests\counter_retry\test_hidden.py:5: AssertionError
=========================== short test summary info ===========================
FAILED D:\miniclaudecode\evals\hidden_tests\counter_retry\test_hidden.py::test_negative_and_zero


## 失败分析

共 2 次运行失败，按退出原因分布：completed=2。
exit_reason=completed 但验收失败：模型侧已自述完成（脚本走到最终总结），但修复未生效 —— 常见原因是 edit 的 old_text 与源文件不一致（脚本与 repo 不同步），或续跑轮次耗尽后仍未收敛。应先核对 script.json 与 repo/ 源文件是否由同一份模板生成。
