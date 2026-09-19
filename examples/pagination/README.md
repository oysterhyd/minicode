# pagination 演示仓库

这是一个带**分页边界 bug** 的微型 Python 仓库，作为 minicode P0「最小闭环」的演示任务：
Agent 需要定位 `paginate.py` 中 `end = start + page_size + 1` 的 off-by-one 错误，
修复实现，并运行本目录的验收测试确认通过。

## 初始状态（故意保留的 bug）

`paginate()` 约定返回 `(page_items, has_next)`，`page` 为 1 起始。当前实现多取了一个元素：

```text
$ python -m pytest test_paginate.py
2 failed, 2 passed
```

- `test_exact_page_slice` / `test_second_page_slice` 失败：非末页会多返回一个元素；
- `test_last_page` / `test_invalid_page` 恰好不受影响，修复前后都通过。

## 用 FakeProvider 脚本一键重放修复过程（无需任何模型密钥）

```bash
minicode run "修复分页 bug" \
  --workspace examples/pagination \
  --provider fake \
  --script examples/pagination/scripts/fix_pagination.json \
  --yes
```

`scripts/fix_pagination.json` 是一个确定性的 FakeProvider 脚本，按顺序重放四步：
读取 `paginate.py` → `apply_patch` 精确替换 bug 行 → 运行 `pytest` → 输出中文总结。
`--yes` 表示自动允许 `apply_patch` / `run_command` 这类需要审批的工具。

注意：脚本中的测试命令假设 `python`（PATH 上的解释器）已安装 pytest。
如果没有，可先把脚本里的命令改成你的 venv 解释器，例如
`& "D:/miniclaudecode/.venv/Scripts/python.exe" -m pytest test_paginate.py`
（PowerShell 对带引号的路径需要 `&` 调用符）。脚本刻意不加 `-q`：
pytest 9 在双重 quiet 下会隐藏 "N passed" 摘要行。

运行结束后：

- 工作区摘要会打印退出原因、轮数、token 用量与耗时；
- 「修改摘要」展示 `apply_patch` 产生的 diff；
- `minicode report <session-id>` 可查看完整事件与工具输出。

## 接真实模型

设置 `ANTHROPIC_API_KEY` 后去掉 `--provider fake`，让模型自己完成同样的任务：

```bash
export ANTHROPIC_API_KEY=sk-ant-...
minicode run "修复分页越界错误，并运行测试验证" \
  --workspace examples/pagination \
  --provider anthropic
```

不带 `--yes` 时，`apply_patch` 与 `run_command` 会在执行前逐个请求确认。

## 提示

- 本目录的测试**故意包含失败用例**，不要把 `examples/` 加进主项目的默认测试路径；
  修复 bug 后 4 个测试应全部通过。
- 想重新体验演示，把 `end = start + page_size` 改回带 `+ 1` 的版本即可。
