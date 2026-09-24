# 8–10 分钟演示脚本

目标：展示修复、权限、恢复、独立验收和失败 trace。建议在 PowerShell 7 中操作，先激活 `.venv`，并把本页命令中的仓库根目录设为当前目录。

1. **准备干净副本（约 1 分钟）**。`Copy-Item -Recurse examples/pagination $env:TEMP/minicode-demo-<日期>`。在副本里运行 `python -m pytest test_paginate.py -q`，确认初始失败。不要直接修改仓库内 fixture。
2. **展示目标和授权（约 1 分钟）**。运行 `minicode run "修复分页边界错误，只改 paginate.py 并验证" --workspace <副本路径> --provider commandcode --acceptance examples/pagination/acceptance.yaml --max-rounds 1`。默认模式会对写入和 Shell 逐次审批；说明工作区读取与写入审批的区别。`--max-rounds 1` 在第一个模型轮次后保存会话。
3. **恢复（约 2 分钟）**。从上一条输出复制会话 ID，运行 `minicode resume <ID> --workspace <副本路径> --provider commandcode --acceptance examples/pagination/acceptance.yaml`，在交互提示符输入 `/continue`。展示已保存的工具结果不重复运行；未知写入会要求先核实。恢复前可用 `minicode report <ID>` 看已有事件。
4. **验收与报告（约 2 分钟）**。模型完成后检查 `python -m pytest test_paginate.py -q`、`git diff --no-index examples/pagination/paginate.py <副本路径>/paginate.py`（退出码 1 表示有差异），以及 `minicode report <ID> --format html`。报告是离线单文件，展示时间线、diff、退出码、用量和验收证据。
5. **失败路径（约 1 分钟）**。打开真实评测 `reports/eval-real-20260923/summary.md` 中一个失败 record，并用其 `session_id` 查看 SQLite trace 或 `minicode report <ID> --db <该次运行的 session.sqlite3>`。解释资源上限或验收失败如何保存进度，避免假报完成。
6. **机制追问（约 1–2 分钟）**。展示 `tests/test_crash_windows.py` 的三处崩溃点、四篇 ADR 和外部评分规则。说明 worktree 与本机权限门不是执行沙箱。

网络不可用时，可以使用 `--provider fake --script examples/pagination/scripts/fix_pagination.json --yes` 生成**明确标为脚本回放**的离线演示；该路径不能证明模型独立修复能力。真实模型评测原始记录与配置见 `evals/README.md`。

另可用 `python examples/render_trace_replay.py --db <真实模型会话的 session.sqlite3> --output reports/demo/trace-replay.mp4`
从持久事件生成**历史执行压缩回放**。此脚本需要 Pillow 与 ffmpeg，视频会明确标注它不是实时屏幕录制。
