# evals —— P1 本地任务评测集（20 任务 × 三基线，可离线运行）

这是一套完全离线的评测：20 个带 bug 的微型 Python 仓库，由 FakeProvider 重放
确定性修复脚本驱动 `AgentRuntime`，runner 自己做验收（子进程跑验收命令 +
对比受保护文件哈希），不依赖 goals 包、不需要任何模型密钥。

## 目录结构

```
evals/
├── generate_tasks.py        # 程序化生成全部 20 个任务（模板字典 → 任务目录）
├── run_eval.py              # 三基线评测 runner
├── README.md
└── tasks/<task_id>/         # 提交在仓库里的生成产物（可再生）
    ├── repo/                # 带 bug 的微型仓库：<module>.py + test_<module>.py
    ├── task.yaml            # 任务定义（schema 见下，goals 模块兼容此格式）
    └── script.json          # FakeProvider 修复脚本（读文件 → apply_patch → 跑测试 → 总结）
```

每个任务的仓库刻意保持极小（每文件 < 40 行）：测试**先失败**（红），
修复后全部通过（绿）。`generate_tasks.py` 从同一份模板字典同时生成源码与
脚本的 `old_text`，保证 `apply_patch` 精确命中；`python evals/generate_tasks.py
--verify` 会在临时目录里对每个任务做红→绿自检。

## 运行方式

```bash
# 全量三基线（约 60 次运行，每次 2s 左右）
PYTHONPATH=src .venv/Scripts/python.exe -m evals.run_eval

# 只跑部分基线 / 指定任务（两种调用方式等价）
PYTHONPATH=src .venv/Scripts/python.exe -m evals.run_eval --baselines b0,b2 --output reports/eval-smoke
.venv/Scripts/python.exe evals/run_eval.py --task pagination_bounds --task fib_seed

# 调试：保留每个组合的工作区副本（输出在 <output>/workspaces/）
PYTHONPATH=src .venv/Scripts/python.exe -m evals.run_eval --task queue_fifo --keep-workspaces
```

参数：`--tasks-dir`（默认 `evals/tasks`）、`--task <id>`（可重复，缺省全部）、
`--baselines b0,b1,b2`、`--output`（默认 `reports/eval`）、`--keep-workspaces`。

runner 会把启动解释器所在目录（venv 的 `Scripts/`）前置到子进程 PATH，
保证 `run_command` 与验收子进程里的 `python` / `pytest` 解析到同一套环境。

## task.yaml schema

```yaml
id: pagination_bounds
title: 分页边界：……
prompt: |
  <给模型的任务描述：现象 + 验收标准>
allowed_paths: [paginate.py]
protected_paths: [test_paginate.py]
items:
  - id: tests-pass
    type: command
    command: python -m pytest test_paginate.py -q
  - id: tests-unchanged
    type: protected
    path: test_paginate.py
budget: {max_rounds: 10, max_tokens: 80000, max_seconds: 120}
max_fix_attempts: 3
script: script.json
```

## 20 个任务

| 任务 id | bug 类型 | 被测模块 | 验收命令 |
|---|---|---|---|
| pagination_bounds | 分页边界 | paginate.py | `python -m pytest test_paginate.py -q` |
| loop_off_by_one | 循环差一 | sum_to_n.py | `python -m pytest test_sum_to_n.py -q` |
| comparison_reversed | 比较运算符写反 | in_range.py | `python -m pytest test_in_range.py -q` |
| string_format | 字符串格式化错误 | greeting.py | `python -m pytest test_greeting.py -q` |
| dict_key_typo | dict 键名笔误 | user_profile.py | `python -m pytest test_user_profile.py -q` |
| division_guard | 除零缺保护 | ratio.py | `python -m pytest test_ratio.py -q` |
| weekday_math | 日期星期计算错 | weekday.py | `python -m pytest test_weekday.py -q` |
| mutable_default | 可变默认参数 | cart.py | `python -m pytest test_cart.py -q` |
| sort_key | 排序 key 错 | leaderboard.py | `python -m pytest test_leaderboard.py -q` |
| missing_return | 缺 return | finder.py | `python -m pytest test_finder.py -q` |
| float_rounding | 浮点舍入 | money.py | `python -m pytest test_money.py -q` |
| empty_list_bounds | 空列表越界 | picker.py | `python -m pytest test_picker.py -q` |
| regex_mismatch | 正则写错 | validator.py | `python -m pytest test_validator.py -q` |
| unit_conversion | 单位换算错 | temperature.py | `python -m pytest test_temperature.py -q` |
| dedup_logic | 去重逻辑错 | dedup.py | `python -m pytest test_dedup.py -q` |
| min_max_reversed | min/max 写反 | boundary.py | `python -m pytest test_boundary.py -q` |
| fib_seed | 斐波那契种子错 | fib.py | `python -m pytest test_fib.py -q` |
| queue_fifo | 队列 FIFO 顺序错 | job_queue.py | `python -m pytest test_job_queue.py -q` |
| slugify_unicode | slugify 中文/unicode 处理错 | slugify.py | `python -m pytest test_slugify.py -q` |
| cache_ttl | 缓存 TTL 判断错 | cache_ttl.py | `python -m pytest test_cache_ttl.py -q` |

## 基线定义

每个 (task, baseline) 组合都从 `repo/` 的**干净副本**开始（禁止复用工作区），
各自使用独立的 SqliteStore。所有基线共用任务 `budget` 与 `max_fix_attempts`。

- **b0 —— 基础循环**：一次 `run_turn(prompt)`，无上下文压缩、无验收门。
  结束后 runner 验收：验收命令退出码 0 且受保护文件哈希与运行前一致。
- **b1 —— 同 b0（预留压缩钩子）**：行为与 b0 完全一致。`run_combo` 预留了
  `compactor=None` 参数位置（TODO 注释标明接线点），主 agent 的上下文压缩
  集成后在此接入；当前用于记录未压缩时的 token 基线。
- **b2 —— b1 + 验收失败续跑**：`run_turn` 结束后 runner 验收；若失败且未超
  `max_fix_attempts` 与预算（rounds/tokens），把失败详情（每个失败项 + 退出码 +
  “必须修复，不得改受保护文件”）作为新一轮 `run_turn` 输入继续跑，每次续跑后
  重新验收，最多 `max_fix_attempts` 次。

### 成功定义

一次运行记为 **pass** 当且仅当：全部 `type: command` 验收项退出码为 0，且全部
`type: protected` 文件哈希未变。失败、超时、预算耗尽、续跑耗尽全部计入分母。

## 结果目录结构

```
<output>/
├── results.json   # 结构化结果：generated_at / baselines / run_count / passed / records[]
│                  # 每条 record: task, baseline, pass, exit_reason, attempts,
│                  #             rounds, input_tokens, output_tokens, seconds, error
├── summary.md     # 基线对照表 + 逐任务结果表 + 失败任务与退出原因 + 确定性失败分析
└── workspaces/    # 仅 --keep-workspaces 时存在：<task>__<baseline>/workspace/
```

## 诚实说明：FakeProvider 评测度量的是 harness 机制，不是模型智力

FakeProvider 按脚本重放固定的 `read_file → apply_patch → run_command → 总结`
序列，模型"决定"的环节是被排除的。因此这套评测能回答的问题仅限于 harness
层：脚本重放是否稳定、工作区是否干净隔离、预算是否生效、验收命令与受保护
文件哈希是否被如实执行、b2 的续跑分支是否按预期触发、退出原因是否可区分。
它**不能**回答"模型能不能独立修好 bug"——那需要接真实模型的在线评测。
在当前脚本保证一次修复成功的设定下，b0/b1/b2 的成功率和轮数必然一致，b2 的
续跑分支只在验收失败时才会产生差异（runner 的失败分析模板也照此措辞）。

## 再生成

```bash
python evals/generate_tasks.py --verify   # 重新生成 20 个任务并跑红→绿自检
```

任务目录已提交；`generate_tasks.py` 幂等（覆盖式重写），可随时再生。
