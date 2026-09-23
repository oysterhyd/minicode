# minicode 当前架构（P0 与已接入的 P1）

本文按 2026-09-22 的代码核对，描述模块划分、一次 run 的事件流与关键执行语义。
[plan.md](../plan.md) 是原始设计目标；后续优化见 [探索与优化方案](optimization-design.md)。
交互界面以中文为主，部分工具错误为英文。共享模型位于 `core/models.py`，运行时也直接依赖
工具、存储与 Goal 实现；目前没有插件加载器或独立的多代理调度层。

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
        │ anthropic adapter│ │ files.py       │ │ ModePolicy           │
        │ commandcode.py   │ │                │ │                      │
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

主调用方向：`CLI/TUI → runtime → {providers, tools, security, storage, goals}`，共享 `core` 契约。
`cli.py` 同时承担服务装配、provider 选择、REPL 命令和展示；`ui/app.py` 复用其中多个私有函数。
这部分耦合是当前状态，不能将两个前端视为已经完全独立。P1 模块见 §5。

## 2. 一次 run 的事件流

`minicode run "任务"` 的正常工具循环对应以下 `EventType` 序列（可从 `storage.get_events`
读取事件；事件中的工具输出仅为预览，不等于完整原始输出）：

```text
SESSION_START                          # 首个 run_turn 时创建会话行
│
│  每轮循环，直到模型不再调用工具或预算耗尽：
├─ ROUND_START {round}
├─ ASSISTANT_MESSAGE {text, tool_calls, usage, stop_reason}
├─ TOOL_CALL_START {call_id, name, arguments}     # 连续只读调用可并发
│    ├─（若权限门判定 ASK 且配置了审批处理器）
│    ├─ APPROVAL_REQUEST {call_id, tool_name, summary}
│    ├─ APPROVAL_DECISION {call_id, granted, reason}
│    └─
├─ TOOL_CALL_RESULT {call_id, name, success, exit_code, error, output_preview}
├─ ROUND_END {round}
│
└─ SESSION_END {exit_reason, rounds, total_usage[, error]}
```

对应关系：文本增量不产生事件（流式渲染走 `on_text_delta` 回调，只有装配完成的
`ASSISTANT_MESSAGE` 落库）；未知工具、参数不合法、审批拒绝、工具内部异常都会以
`TOOL_CALL_RESULT`（success=false）回填给模型并继续循环，而不是终止会话。

## 3. 关键语义

**权限门**。当前顺序为「记录 TOOL_CALL_START → 查找工具 → 对原始参数执行权限判定/审批
→ `BaseTool.run` 用 Pydantic 校验参数 → 执行」。因此参数错误可能先触发审批；原方案中的
“校验最终参数后审批”尚未实现。
`ModePolicy` 按三态权限模式判定：default 下只读工具（read / ls / grep）ALLOW、
edit / write / bash ASK、未配置工具默认 DENY；accept_edits 额外自动允许文件编辑；
bypass 全部 ALLOW。`/permissions` 命令可在运行时切换模式并同步到状态栏
（`--yes` 初始采用 bypass）。ASK 时审批处理器拿到的是调用的**原始参数**（`ApprovalRequest.arguments`），
CLI 用 Rich `Confirm` 展示工具名与摘要；拒绝则以错误 tool_result 回填模型，工具不执行。
参数变化即视为新调用，重新过权限门。

**预算顺序**。`run_turn` 是可恢复的执行切片，内部可包含多个模型轮次。墙钟 deadline
在每次执行开始时重置，轮数按本次执行计数；CLI/TUI 达到轮次切片时自动调用 `continue_turn`，
时长切片在持久化消息有进展时也会自动续跑，无进展则暂停；
token 预算在**助手响应已计入、但其工具尚未执行之前**检查——预算已耗尽时不再产生任何
副作用。未执行的 tool_use 会收到明确的错误 tool_result，避免恢复时误判为副作用未知。
正常完成和暂停均经由 `_finalize`，取消经由专门的清理与持久化路径。

**token 默认不限制**。`Budget.max_total_tokens` 为 0（或负）时 token 检查恒不触发，
默认以轮数与时长切片保存进度。它统计**各轮输入与输出 token 的累计值**（每轮重发上下文，
所以 20 轮 × 20k 输入 ≈ 400k 输入 token，另加输出），不是当前上下文占用。
`--max-tokens N` 是 token 量上限，不区分缓存折扣，不能直接等同于货币成本上限。

**取消**。Ctrl+C 时 asyncio.run 取消主任务；AgentRuntime 在 `CancelledError` 处理中
**先**把会话落库为 `paused`、原因记为 `cancelled`（含 SESSION_END 事件），
**再**向上传播取消。CLI 捕获 KeyboardInterrupt 映射为退出码 130。chat 模式下每轮独立
`asyncio.run`，回合内 Ctrl+C 只取消当前轮并回到提示符。

**持久化与恢复边界**。消息与事件随流程写入，写事务内分配序号；轮数与用量逐轮更新，
恢复时从事件补齐可能滞后的计数。暂停任务通过 `continue_turn` 无需重复输入；`resume` 可跨进程续跑。
`report` 只展示已有记录，不执行工具。
不能将压缩后的消息表或事件预览当作完整原始历史。

## 4. 与 plan.md 的映射（P0 六条能力 → 实现位置）

| plan.md §3 P0 能力 | 实现位置 |
| --- | --- |
| CLI 单轮执行、交互会话、流式展示、Ctrl+C 取消 | `src/minicode/cli.py`（run / chat / `_run_one_turn` / `_StreamPrinter`） |
| 真实模型适配器 + Fake Provider | `src/minicode/providers/commandcode.py`、`providers/anthropic_provider.py`、`providers/fake.py`（契约 `providers/base.py`） |
| 六个内置工具：read / bash / edit / write / ls / grep | `src/minicode/tools/files.py`、`tools/search.py`、`tools/command.py`、`tools/registry.py` |
| 参数校验、工作区边界、审批、超时与输出限制 | `tools/base.py`（args 校验 + ToolLimits）、`core/paths.py`（越界解析）、`security/policy.py`、`tools/command.py`（超时/进程树终止）、`cli.py`（交互审批） |
| 会话持久化、事件追踪、diff 与退出码报告 | `storage/sqlite_store.py`、`runtime/events.py`、`cli.py`（`report` / `_print_diff_summary`） |
| 轮数 / token / 时长预算，退出原因可区分 | `core/models.py`（Budget / ExitReason / RunResult）、`runtime/budget.py`、`runtime/loop.py`（结束与取消路径） |

P0 演示闭环：`examples/pagination/`（带 bug 的仓库 + 失败测试）与
`examples/pagination/scripts/fix_pagination.json`（FakeProvider 脚本），
端到端测试见 `tests/test_cli.py`。

## 5. P1：可靠性能力（实际实现）

P1 服务主要由 `cli.py` 的 `_build_services` / `_attach_compactor` 装配，Runtime 驱动
压缩、验收、恢复及后台结果投递。部分接口采用鸭子类型，Goal 恢复与验收路径则直接导入
`goals` 中的类型并作类型判断。

### 5.1 模块增量

```text
runtime/loop.py      P1 钩子：压缩 / Goal 门 / 后台投递 / 恢复 / artifact 转存
context/             estimate（保守 token 估算）、compact（归档→压缩→摘要）
goals/               spec（验收 YAML）、checker（指纹/快照/检查）、evidence（证据账本）
tasks/               background（已接入）、taskstore（独立模块，尚未接入主循环）
tools/command.py     bash 增加 background 参数
storage/artifacts.py ArtifactStore：会话工件目录 + 清单表（当前数据库 schema v4）
reports/             render_session_html：单文件离线 HTML 报告
ui/                  Textual 全屏 TUI（minicode tui）
providers/           commandcode.py + zcode_config.py：OpenAI 兼容默认适配器
evals/               20 任务离线评测集 + b0/b1/b2 标签（b1=b0）
```

### 5.2 上下文压缩与归档

两段式：

1. **工具结果转存**（loop 内，逐调用）：工具在裁剪模型预览时同时保留原文；成功或失败输出超过
   `spill_threshold_chars`（4000）时，原文写入 `ArtifactStore`
   （`<db 目录>/artifacts/<session>/<id>.txt` + 清单表），模型看到前 1000 字符预览与
   `[artifact:<id>]`。`read_artifact` 只在当前 session manifest 中查找，并按字符 offset/limit 分页回读。
2. **轮前压缩**（`ContextCompactor`）：估算 prompt 与 provider 实际 `max_tokens` 的合计超过当前模型
   窗口的 `trigger_fraction`（0.8）时依次执行；模型切换后两个值都立即重读。按 user 请求归档早期
   历史；若整个长任务只有一条初始 user 消息，则改按已闭合的 assistant/tool-result 交互组归档。
   归档摘要逐字带回 user 文本，原始初始请求保持独立；随后缩短较旧工具结果
   → 满足条件时合并早期单元为确定性摘要；归档/合并前检查调用结果配对。归档原文落 artifact，压缩后的
   消息列表通过 `store.replace_messages` 原子重写持久化，并发出 `CONTEXT_COMPACTED` 事件。缩短结果时
   会从全文提取并保留 artifact 引用；仍超限时归档单个超长文本并提供分页引用。服务端实际
   拒绝上下文时再进行有界缩减重试，无法容纳时以 `context_limit` 暂停并保留任务。

现有限制：

- `read_artifact` 按字符分页，读取页面时只保持有限缓冲；大型命令输出流式落盘归档。
- 固定系统提示或工具 schema 本身超出模型窗口时无法无损缩减；任务会暂停，需换模型或调整配置。
- 字符数 / 3 不是严格上界，服务端 tokenizer 仍可能与本地估算有偏差。

### 5.3 Goal 验收与证据（plan.md §6.4 → s17）

验收 YAML 三类检查项：`command`（退出码 0）、`artifact`（文件存在于工作区内，越界拒绝）、
`protected`（路径内容与会话开始时的快照一致）。执行顺序：

```text
模型回答（无工具调用）
  → 证据快路径：通过证据的指纹 == 当前工作区指纹 → 直接 COMPLETED
  → GoalChecker.run()：逐项检查 → GOAL_CHECK 事件
      通过 → 记录证据（EvidenceLedger，绑定指纹）→ COMPLETED
      失败 → 结构化失败报告回填为 user 消息 → 继续循环
             （超过 max_fix_attempts → GOAL_NOT_MET 暂停，可继续）
```

受保护路径快照在会话首个回合开始时捕获（模型改动之前），随验收配置保存到 SQLite
`session_goals`（schema v3）；resume 恢复原基线，旧会话缺失基线时检查失败而非重新采样。
保护检查放在全部验收命令之后，最终指纹也在检查完成后计算。配置了验收门时，模型自述
“完成”不会跳过验收逻辑；参与指纹计算的工作区内容变化会使旧证据失效。

### 5.4 会话恢复（plan.md §6.3）

`AgentRuntime.resume(store, session_id, ...)`：校验会话存在与工作区存在 → 加载消息、
用量、轮数及保存的 provider/model/workspace（CLI/TUI 共用恢复配置）→ 找出**有 tool_use 但无对应 tool_result** 的悬空调用（中断窗口）。悬空调用
在下次 `run_turn` 开始时结算：

| 悬空调用类型 | 结算方式 |
| --- | --- |
| 只读（read / ls / grep / read_artifact） | 重新检查当前权限后执行，发出带 `recovered: true` 的新 START/RESULT 事件，结果回填原始 call id |
| 写 / Shell（edit / write / bash / 未知工具） | **不重放**：`SIDE_EFFECT_UNKNOWN` 事件 + 提示性 tool_result（"副作用状态未知，先核实再继续"） |

已落库的结果永不重复执行；后台任务无进程可继承（一律不凭旧 PID 管理）。

### 5.5 后台命令（plan.md §6.5）

- `bash(background=true)`：`BackgroundManager.start` 立即返回 job id；完成事件
  `BACKGROUND_JOB_COMPLETED`；结果在下一轮开始时作为 **user 消息**投递（每个 job 恰好
  投递一次，原 tool call 不产生第二个 tool result）；轮次或时长切片自动续跑期间保持进程，
  其他暂停或取消时 `cancel_all()` 清理进程树；启动记录 `BACKGROUND_JOB_STARTED`，
  被终止的任务记录 `BACKGROUND_JOB_LOST` 并向用户和模型投递，之后才发出 `SESSION_END`。
### 5.6 评测与报告

- `evals/`：20 个本地任务（repo fixture + task.yaml + FakeProvider 修复脚本），runner
  （`minicode eval`）在干净副本上运行，b0 是基础循环；b1 与 b0 相同，尚未接入压缩；
  b2 在 runner 外层验收失败后续跑，没有装配运行时 Goal 门。结果为 JSON + Markdown。
  FakeProvider 的 usage 来自脚本/默认值，不度量真实模型能力、缓存或 token 节省；恢复等边界另由 tests 覆盖。
- `minicode report <id> --format html`：单文件 HTML（零外链、可离线打开），时间线、
  工具记录与 diff、验收证据表、用量；所有动态文本经 HTML 转义。
- `minicode tui`：Textual 全屏界面——流式回复、可展开工具卡片、审批 ModalScreen、
  斜杠命令与自动补全（/help /model /effort /permissions /clear /new /sessions /resume
  /compact /exit）、简短环境信息、状态栏（CWD · 权限模式 · 模型 · 上下文负载 · Token ·
  缓存命中率）、Ctrl+C 取消当前回合。文本增量合并刷新，回复结束后渲染 Markdown；
  工具结果按需展开，运行中可排队下一条输入，用户上滚后新消息不强制拉到底部。
  尚无命令实时输出或任务/子代理面板。

### 5.7 P1 → 实现位置映射

| plan.md §3 P1 能力 | 实现位置 |
| --- | --- |
| 分层上下文压缩 + 输出归档与分页回读 | `context/`、`storage/artifacts.py`、`tools/artifacts.py`、`loop._maybe_spill/_compact_if_needed` |
| 会话恢复及未确认副作用处理 | `loop.resume/_settle_recovery`、`cli.py`（`resume` 命令） |
| Goal 验收器 + 失败续跑 + 证据绑定 | `goals/`、`loop._goal_gate`、`--acceptance` 装配 |
| 后台命令；独立的任务依赖存储 | `tasks/background.py` 已接入；`tasks/taskstore.py` 尚未接入 Runtime；`tools/command.py`、`loop._deliver_finished_jobs` |
| 评测集、基线、失败分析 | `evals/`（20 任务；run_eval.py 的 b1 目前等同 b0） |
| 可离线查看的 HTML 执行报告 | `reports/html.py`、`cli.py`（`report --format html`） |
| 类 Claude Code TUI | `ui/`（Textual App + Pilot 测试） |

## 6. Provider、缓存及扩展边界

- `Provider.stream` 目前只提供 `TextDelta` 与终结 `ResponseDone`，没有工具参数增量、
  阶段反馈或细粒度 usage 事件。CommandCode 忽略 reasoning-only 文本增量。
- CommandCode 在同一事件循环内复用 `httpx.AsyncClient`；CLI 每个 `asyncio.run` 回合结束前
  显式关闭，TUI 退出或切换模型时关闭旧 client。Anthropic 持有 SDK client。
- 缓存统计：CommandCode 解析 `prompt_tokens_details.cached_tokens`；Anthropic 将普通输入、
  缓存创建和缓存读取合并到输入总数。`Usage` 单独记录 cache-read/cache-write 和 available；轮次事件、
  SQLite schema v4 与 resume 保存并恢复这些累计值，缺失 usage 不再冒充真实的零。
- Anthropic 请求没有设置 `cache_control`；兼容网关是否支持缓存、返回哪些 usage 字段，
  必须实际核验，不能由 API 格式兼容推断。
- `_build_provider` 与 `_provider_for_model` 都把目录最大输出传给 provider；Runtime 从 provider 的
  实际 `max_tokens` 计算 prompt 预算，压缩器使用同一值作为响应预留。
  `/effort off` 省略字段，表示采用网关默认行为，不等于明确关闭推理。
- 工具表固定为七个内置工具（含 `read_artifact`）；权限策略和恢复只读集合仍使用内置工具名。
  Subagents、Skills、MCP、Plugins、项目指令自动加载及长期记忆尚未提供。
