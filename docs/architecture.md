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
        │ anthropic.py     │ │ files.py       │ │ ModePolicy           │
        │ fake.py          │ │ search.py      │ │ DefaultPolicy        │
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
`ModePolicy` 按三态权限模式判定：default 下只读工具（read / ls / grep）ALLOW、
edit / write / bash ASK、未配置工具默认 DENY；accept_edits 额外自动允许文件编辑；
bypass 全部 ALLOW。`/permissions` 命令可在运行时切换模式并同步到状态栏
（`--yes`）。ASK 时审批处理器拿到的是**最终参数**（`ApprovalRequest.arguments`），
CLI 用 Rich `Confirm` 展示工具名与摘要；拒绝则以错误 tool_result 回填模型，工具不执行。
参数变化即视为新调用，重新过权限门。

**预算顺序**。每轮开始依次检查：时长（每轮重置的墙钟 deadline）→ 轮数（会话累计）；
token 预算在**助手响应已计入、但其工具尚未执行之前**检查——预算已耗尽时不再产生任何
副作用（测试 `test_token_budget_stops_before_tool_execution` 固定了这一顺序）。
所有退出路径都经由唯一的 `_finalize`，退出原因可区分地持久化。

**token 默认不限制**。`Budget.max_total_tokens` 为 0（或负）时 token 检查恒不触发，
默认只靠轮数与每轮时长兜底。原因：它统计的是**各轮 prompt 之和**（每轮重发全部上下文，
所以 20 轮 × 20k 上下文 ≈ 400k「token」），度量的是花费而不是上下文占用——把上下文
交给压缩层（§5.2）按模型窗口控制才是对的。需要硬性成本上限时用 `--max-tokens N`。

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

## 5. P1：可靠性能力（实际实现）

P1 在不动 P0 主干的前提下叠加了六层能力。Runtime 对新模块只依赖鸭子类型协议
（`loop.py` 不 import 新包），装配发生在 CLI 层（`cli.py` 的 `_build_services` /
`_attach_compactor`）。

### 5.1 模块增量

```text
runtime/loop.py      P1 钩子：压缩 / Goal 门 / 后台投递 / 恢复 / artifact 转存
context/             estimate（保守 token 估算）、compact（归档→压缩→摘要）
goals/               spec（验收 YAML）、checker（指纹/快照/检查）、evidence（证据账本）
tasks/               background（后台命令）、taskstore（任务依赖）
tools/command.py     bash 增加 background 参数
storage/artifacts.py ArtifactStore：会话工件目录 + 清单表（schema v2）
reports/             render_session_html：单文件离线 HTML 报告
ui/                  Textual 全屏 TUI（minicode tui）
providers/           commandcode.py + zcode_config.py：OpenAI 兼容默认适配器
evals/               20 任务离线评测集 + 三基线 runner
```

### 5.2 上下文压缩（plan.md §6.2 → s08）

两段式：

1. **工具结果转存**（loop 内，逐调用）：成功输出超过 `spill_threshold_chars`（4000）时，
   完整内容写入 `ArtifactStore`（`<db 目录>/artifacts/<session>/<id>.txt` + 清单表），
   模型只看到预览 + `[artifact:<id>]` 归档引用，完整内容留存在会话归档中。
2. **轮前压缩**（`ContextCompactor`）：估算 token（字符/3，标注为估算）超过「当前模型
   prompt 预算 × `trigger_fraction`（0.8）」时依次执行——prompt 预算 =
   `context_window - max_output_tokens`（`AgentRuntime.prompt_budget_tokens()`，每次检查
   现读，`/model` 切换立即生效）。压缩层次：按完整交互单元归档早期历史（单元 =
   user + assistant + 其 tool_result；tool_use/tool_result 同进同退）→ 压缩保留区中较旧
   的工具结果 → 仍超时把最老单元合并为确定性结构化摘要。归档原文落 artifact，压缩后的
   消息列表通过 `store.replace_messages` 原子重写持久化，并发出 `CONTEXT_COMPACTED` 事件。

### 5.3 Goal 验收与证据（plan.md §6.4 → s17）

验收 YAML 三类检查项：`command`（退出码 0）、`artifact`（文件存在于工作区内，越界拒绝）、
`protected`（路径内容与会话开始时的快照一致）。执行顺序：

```text
模型回答（无工具调用）
  → 证据快路径：通过证据的指纹 == 当前工作区指纹 → 直接 COMPLETED
  → GoalChecker.run()：逐项检查 → GOAL_CHECK 事件
      通过 → 记录证据（EvidenceLedger，绑定指纹）→ COMPLETED
      失败 → 结构化失败报告回填为 user 消息 → 继续循环
             （超过 max_fix_attempts → GOAL_NOT_MET，独立退出原因）
```

受保护路径快照在会话首个回合开始时捕获（模型改动之前），随验收配置保存到 SQLite
`session_goals`（schema v3）；resume 恢复原基线，旧会话缺失基线时检查失败而非重新采样。
保护检查放在全部验收命令之后，最终指纹也在检查完成后计算。模型自述"完成"永远不会
绕过检查；证据与代码状态绑定——任何文件变化都会使旧证据失效。

### 5.4 会话恢复（plan.md §6.3）

`AgentRuntime.resume(store, session_id, ...)`：校验会话存在与工作区存在 → 加载消息、
用量、轮数及保存的 provider/model/workspace（CLI/TUI 共用恢复配置）→ 找出**有 tool_use 但无对应 tool_result** 的悬空调用（中断窗口）。悬空调用
在下次 `run_turn` 开始时结算：

| 悬空调用类型 | 结算方式 |
| --- | --- |
| 只读（read / ls / grep） | 重新执行，发出带 `recovered: true` 的新 START/RESULT 事件，结果回填原始 call id |
| 写 / Shell（edit / write / bash / 未知工具） | **不重放**：`SIDE_EFFECT_UNKNOWN` 事件 + 提示性 tool_result（"副作用状态未知，先核实再继续"） |

已落库的结果永不重复执行；后台任务无进程可继承（一律不凭旧 PID 管理）。

### 5.5 后台命令（plan.md §6.5）

- `bash(background=true)`：`BackgroundManager.start` 立即返回 job id；完成事件
  `BACKGROUND_JOB_COMPLETED`；结果在下一轮开始时作为 **user 消息**投递（每个 job 恰好
  投递一次，原 tool call 不产生第二个 tool result）；回合结束时 `cancel_all()` 清理进程树；启动记录 `BACKGROUND_JOB_STARTED`，
  被终止的任务记录 `BACKGROUND_JOB_LOST` 并向用户和模型投递，之后才发出 `SESSION_END`。
### 5.6 评测与报告

- `evals/`：20 个本地任务（repo fixture + task.yaml + FakeProvider 修复脚本），runner
  （`minicode eval`）在干净副本上运行，b0（基础循环，runner 自行验收）与 b2（验收失败
  续跑）对照；结果 JSON + Markdown 汇总 + 失败分析。诚实口径：FakeProvider 评测度量
  harness 机制（验收、预算、恢复路径），不度量模型智力，不冒充真实模型成绩。
- `minicode report <id> --format html`：单文件 HTML（零外链、可离线打开），时间线、
  工具记录与 diff、验收证据表、用量；所有动态文本经 HTML 转义。
- `minicode tui`：Textual 全屏界面——流式回复、工具卡片、审批 ModalScreen、
  斜杠命令与自动补全（/help /model /effort /permissions /clear /new /sessions /resume
  /compact /exit）、启动 Banner、状态栏（CWD · 权限模式 · 模型 · 上下文负载 · Token ·
  缓存命中率）、Ctrl+C 取消当前回合。

### 5.7 P1 → 实现位置映射

| plan.md §3 P1 能力 | 实现位置 |
| --- | --- |
| 分层上下文压缩 + 原始输出按需读取 | `context/`、`storage/artifacts.py`、`loop._maybe_spill/_compact_if_needed` |
| 会话恢复及未确认副作用处理 | `loop.resume/_settle_recovery`、`cli.py`（`resume` 命令） |
| Goal 验收器 + 失败续跑 + 证据绑定 | `goals/`、`loop._goal_gate`、`--acceptance` 装配 |
| 任务依赖、后台测试 | `tasks/`、`tools/command.py`（background）、`loop._deliver_finished_jobs` |
| 评测集、基线、失败分析 | `evals/`（20 任务 + run_eval.py 三基线 runner） |
| 可离线查看的 HTML 执行报告 | `reports/html.py`、`cli.py`（`report --format html`） |
| 类 Claude Code TUI | `ui/`（Textual App + Pilot 测试） |
