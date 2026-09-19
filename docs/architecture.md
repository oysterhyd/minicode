# minicode 架构（P0）

本文描述 P0 的实际实现：模块划分、一次 run 的事件流、关键执行语义，以及与
[plan.md](../plan.md) 的映射。所有用户可见字符串为中文，后端包相互独立，仅通过
`core/models.py` 的 pydantic 契约耦合。

## 1. 模块图

```text
                 ┌──────────────────────────────────────────────┐
                 │                cli.py (Typer/Rich)           │
                 │  run / chat / sessions list / report         │
                 │  选项解析、流式渲染、审批提示、报告展示          │
                 └───────────────┬──────────────────────────────┘
                                 │ 装配并驱动
                                 ▼
                 ┌──────────────────────────────────────────────┐
                 │           runtime/  (AgentRuntime)           │
                 │  loop.py    对话循环、工具调度、终止/取消       │
                 │  budget.py  轮数/token/时长预算               │
                 │  events.py  EventRecorder（持久化+回调）       │
                 │  prompt.py  系统提示词                        │
                 └───┬──────────────┬──────────────┬────────────┘
                     ▼              ▼              ▼
        ┌──────────────────┐ ┌────────────────┐ ┌──────────────────────┐
        │ providers/       │ │ tools/         │ │ security/            │
        │ base.py    契约   │ │ registry.py    │ │ policy.py            │
        │ anthropic.py     │ │ files.py       │ │ DefaultPolicy        │
        │ fake.py          │ │ search.py      │ │ AutoAllowPolicy      │
        └──────────────────┘ │ command.py     │ └──────────────────────┘
                             └────────────────┘
                     ┌──────────────────────────────┐
                     │ storage/  SqliteStore        │
                     │ sessions / messages / events │
                     └──────────────────────────────┘

        core/  models.py（Message、ModelResponse、ToolOutcome、Event、
               Budget、RunResult …）+ paths.py（工作区边界解析）
               —— 所有模块的共享契约，无业务逻辑
```

依赖方向：`cli → runtime → {providers, tools, security, storage}`，全部指向 `core`。
`cli.py` 不实现任何运行时逻辑，只做选项解析、装配（provider/registry/policy/store）、
展示（流式文本、工具单行、diff 摘要）与 Ctrl+C 接线。

## 2. 一次 run 的事件流

`minicode run "任务"` 对应如下 `EventType` 序列（`storage.get_events` 可完整回放）：

```text
SESSION_START                          # 首个 run_turn 时创建会话行
│
│  每轮循环，直到模型不再调用工具或预算耗尽：
├─ ROUND_START {round}
├─ ASSISTANT_MESSAGE {text, tool_calls, usage, stop_reason}
│    ├─（若权限门判定 ASK 且配置了审批处理器）
│    ├─ APPROVAL_REQUEST {call_id, tool_name, summary}
│    ├─ APPROVAL_DECISION {call_id, granted, reason}
│    └─
├─ TOOL_CALL_START {call_id, name, arguments}     # 每个工具调用一条，逐个串行
├─ TOOL_CALL_RESULT {call_id, name, success, exit_code, error, output_preview}
├─ ROUND_END {round}
│
└─ SESSION_END {exit_reason, rounds, total_usage[, error]}
```

对应关系：文本增量不产生事件（流式渲染走 `on_text_delta` 回调，只有装配完成的
`ASSISTANT_MESSAGE` 落库）；未知工具、参数不合法、审批拒绝、工具内部异常都会以
`TOOL_CALL_RESULT`（success=false）回填给模型并继续循环，而不是终止会话。

## 3. 关键语义

**权限门**。每个工具调用按「参数校验 → ALLOW/ASK/DENY 判定 → 执行」处理。
`DefaultPolicy`：只读工具（read_file / list_files / search_text）ALLOW，
apply_patch / run_command ASK，未配置的工具默认 DENY；`AutoAllowPolicy` 全部 ALLOW
（`--yes`）。ASK 时审批处理器拿到的是**最终参数**（`ApprovalRequest.arguments`），
CLI 用 Rich `Confirm` 展示工具名与摘要；拒绝则以错误 tool_result 回填模型，工具不执行。
参数变化即视为新调用，重新过权限门。

**预算顺序**。每轮开始依次检查：时长（每轮重置的墙钟 deadline）→ 轮数（会话累计）；
token 预算在**助手响应已计入、但其工具尚未执行之前**检查——预算已耗尽时不再产生任何
副作用（测试 `test_token_budget_stops_before_tool_execution` 固定了这一顺序）。
所有退出路径都经由唯一的 `_finalize`，退出原因可区分地持久化。

**取消**。Ctrl+C 时 asyncio.run 取消主任务；AgentRuntime 在 `CancelledError` 处理中
**先**把会话落库为 `cancelled`（含 SESSION_END 事件，且该收尾自身不可再抛异常），
**再**向上传播取消。CLI 捕获 KeyboardInterrupt 映射为退出码 130。chat 模式下每轮独立
`asyncio.run`，回合内 Ctrl+C 只取消当前轮并回到提示符。

**持久化与恢复边界**。消息、事件、会话状态随发生随写入（写事务内分配序号）。
P0 **只持久化，不恢复**：重启后事件可完整回放（`report`），但不会自动重放任何调用——
尤其是「已开始但结果未落库」的 `unknown` 副作用（如被中断的命令），按 plan.md §6.3
必须在恢复时显式核对。恢复、上下文压缩、Goal 验收均属 P1。

## 4. 与 plan.md 的映射（P0 六条能力 → 实现位置）

| plan.md §3 P0 能力 | 实现位置 |
| --- | --- |
| CLI 单轮执行、交互会话、流式展示、Ctrl+C 取消 | `src/minicode/cli.py`（run / chat / `_run_one_turn` / `_StreamPrinter`） |
| 真实模型适配器 + Fake Provider | `src/minicode/providers/anthropic_provider.py`、`providers/fake.py`（契约 `providers/base.py`） |
| 五个基础工具 | `src/minicode/tools/files.py`、`tools/search.py`、`tools/command.py`、`tools/registry.py` |
| 参数校验、工作区边界、审批、超时与输出限制 | `tools/base.py`（args 校验 + ToolLimits）、`core/paths.py`（越界解析）、`security/policy.py`、`tools/command.py`（超时/进程树终止）、`cli.py`（交互审批） |
| 会话持久化、事件追踪、diff 与退出码报告 | `storage/sqlite_store.py`、`runtime/events.py`、`cli.py`（`report` / `_print_diff_summary`） |
| 轮数 / token / 时长预算，退出原因可区分 | `core/models.py`（Budget / ExitReason / RunResult）、`runtime/budget.py`、`runtime/loop.py`（唯一 `_finalize` 路径） |

P0 演示闭环：`examples/pagination/`（带 bug 的仓库 + 失败测试）与
`examples/pagination/scripts/fix_pagination.json`（FakeProvider 脚本），
端到端测试见 `tests/test_cli.py`。
