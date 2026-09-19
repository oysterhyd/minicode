# minicode（Mini Claude Code）

一个面向本地代码仓库的轻量级 CLI Coding Agent：用户给出任务描述，它通过工具调用检索代码、
修改文件、运行测试，在权限与资源预算内迭代，并留下可审查的 diff、命令退出码和完整执行记录。
定位是**一个具备可靠执行语义的最小 Agent Harness**——模型决定如何解决任务，Runtime 负责
执行协议、权限、预算、取消与持久化；不将调用模型包装成模型训练能力，也不宣称完整复刻商业
Claude Code。

**当前状态（P0）**：可独立使用的最小闭环——单轮任务、交互会话、基础工具、权限审批、预算与
取消、SQLite 会话持久化与执行报告。上下文压缩、会话恢复、Goal 验收等属于 P1，尚未实现
（见下方清单）。

## 快速开始

要求 Python 3.11+（Windows / Linux 均可）。

```bash
git clone <repo-url> miniclaudecode && cd miniclaudecode
python -m venv .venv
source .venv/Scripts/activate        # Windows Git Bash；Linux 为 .venv/bin/activate
pip install -e ".[dev]"
```

### 无密钥演示（FakeProvider 一键重放修复过程）

`examples/pagination` 是一个带分页边界 bug 的微型仓库；下面的命令用确定性的 FakeProvider
脚本重放「读文件 → 改文件 → 跑测试 → 总结」的完整修复过程，不需要任何 API key：

```bash
minicode run "修复分页 bug" \
  --workspace examples/pagination \
  --provider fake \
  --script examples/pagination/scripts/fix_pagination.json \
  --yes
```

运行结束后会打印退出原因、轮数、token 用量与「修改摘要」（diff）；
`minicode report <会话ID>` 可查看完整执行记录。脚本中的测试命令假设 PATH 上的 `python`
已安装 pytest，详见 [examples/pagination/README.md](examples/pagination/README.md)。

### 接真实模型

```bash
export ANTHROPIC_API_KEY=sk-ant-...        # Windows: set ANTHROPIC_API_KEY=...
minicode run "修复分页越界错误，并运行测试验证" --workspace examples/pagination --provider anthropic
```

`--provider` 默认 `auto`：设置了 `ANTHROPIC_API_KEY` 就用 anthropic，否则回退 fake。
不使用 `--yes` 时，`apply_patch` / `run_command` 会在每次执行前请求确认（y/N）。

## CLI 命令

| 命令 | 说明 |
| --- | --- |
| `minicode run "任务"` | 执行一个单轮任务：流式输出回复、`▸/✓/✗` 工具行、修改摘要与统计 |
| `minicode chat` | 交互式多轮会话（同一会话累积上下文）；`exit` / `quit` / Ctrl+D 退出，回合内 Ctrl+C 只取消当前轮 |
| `minicode sessions list` | 会话列表：ID、创建时间、工作区、模型、状态、轮数、token |
| `minicode report <会话ID>` | 执行报告：事件统计、每个工具调用的结果与错误、apply_patch 完整 diff、用量与退出原因；`--full` 打印完整工具输出 |

常用选项（`run` / `chat` 共享）：`--workspace`（默认当前目录）、`--provider auto|fake|anthropic`、
`--model`（默认 `claude-sonnet-4-5`，仅 anthropic 使用）、`--script`（FakeProvider 脚本 JSON）、
`--max-rounds`（默认 20）、`--max-tokens`（默认 200000）、`--max-seconds`（默认 600）、
`--yes/-y`（自动允许全部工具）、`--db`（默认 `~/.minicode/sessions.db`）。

会话（消息、事件、用量）实时持久化到 SQLite；Ctrl+C 取消时先落库 `cancelled` 状态再退出
（退出码 130）。

## P0 能力清单

- CLI 单轮执行、交互会话、流式文本展示、Ctrl+C 取消（取消前先持久化）。
- 两个模型适配器：Anthropic（流式，真实模型）与 FakeProvider（确定性脚本，测试/演示用）。
- 基础工具：`read_file`、`list_files`、`search_text`、`apply_patch`、`run_command`。
- 工具参数校验（pydantic schema）、工作区路径边界（含符号链接/junction）、修改与命令审批
  （ALLOW/ASK/DENY 权限门）、命令超时与输出截断。
- 会话持久化（SQLite）、逐事件执行追踪、diff 与命令退出码报告。
- 预算控制：最大轮数 / 总 token / 每轮时长，退出原因可区分（completed / max_rounds /
  token_budget / time_budget / cancelled / provider_error / internal_error）。

## 明确未实现（P1/P2）

- 会话恢复与断点续跑（P0 只持久化，不自动重放 `unknown` 副作用）。
- 分层上下文压缩与长日志按需读取。
- Goal 验收器、证据绑定与失败续跑。
- HTML 执行报告、评测集与基线对照。
- 子 Agent、任务图、后台命令。
- MCP 工具接入、Skills 按需加载、多 worker 协作。

## 架构

一句话：`cli` 只做展示与装配，`runtime` 的 AgentRuntime 驱动「流式响应 → 权限门 → 工具执行
→ 结果回填」循环，`core` 提供共享 pydantic 契约，事件与消息实时写入 SQLite。
模块图、事件流与关键语义见 [docs/architecture.md](docs/architecture.md)，
完整方案见 [plan.md](plan.md)。

## 运行测试

```bash
PYTHONPATH=src python -m pytest tests        # 未安装时
pytest                                       # pip install -e ".[dev]" 之后
```

全部测试基于 FakeProvider 与临时工作区/数据库，无密钥、可离线运行。

## 致谢与许可

机制设计基于本地教学项目 [learn-claude-code](learn-claude-code/README-zh.md)
（s01–s17 教学主线）的阅读与改造，未直接复制其代码；项目以 MIT 许可发布。
