# minicode（Mini Claude Code）

一个面向本地代码仓库的轻量级 CLI Coding Agent：用户给出任务描述，它通过工具调用检索代码、
修改文件、运行测试，在权限与资源预算内迭代，并留下可审查的 diff、命令退出码和执行记录。
定位是**一个具备可靠执行语义的最小 Agent Harness**——模型决定如何解决任务，Runtime 负责
执行协议、权限、预算、取消与持久化；不将调用模型包装成模型训练能力，也不宣称完整复刻商业
Claude Code。

**当前状态（2026-09-22 核对）**：P0 最小闭环——单轮任务、交互会话、基础工具、权限审批、
预算与取消、SQLite 会话持久化与执行报告——已实现。P1 已接入分层上下文压缩与输出归档、
会话恢复与未知副作用处理、Goal 验收器与证据绑定、后台命令、20 任务离线评测集、HTML
执行报告和 Textual 全屏 TUI。压缩及归档仍有边界限制，见下方说明。
当前没有子代理、MCP、Skills 或 Plugins；`TaskStore` 有独立实现和测试，但尚未接入主循环及交互入口。

**交互与运行时增强**：启动 ASCII Banner（Oyster Harness + 版本/环境信息）、斜杠命令
自动补全（Tab 补全 / ↑↓ 选择 / Esc 关闭 / Enter 确认）、`/model`（z.ai/glm-5.3-flash 与
DeepSeek V4.1 Flash，模型目录配置约 1M 上下文，另含 Claude Sonnet 4.5）与 `/effort`
（off/low/medium/high/xhigh/max；`off` 省略请求字段、采用网关默认值，并不保证关闭推理）、三态权限
状态机 `/permissions`（default / accept_edits / bypass，运行时切换并实时显示于状态栏）、
`/clear` 仅清屏保留上下文、`/new` 彻底重置开启新会话、状态栏实时显示 CWD / Token 用量 /
缓存命中率 / 上下文窗口负载 / 权限模式 / 模型。

## 快速开始

要求 Python 3.11+（Windows / Linux 均可）。

```bash
git clone <repo-url> miniclaudecode && cd miniclaudecode
python -m venv .venv
source .venv/Scripts/activate        # Windows Git Bash；Linux 为 .venv/bin/activate
pip install -e ".[dev]"
```

### 模型 provider

`--provider auto`（默认）按以下顺序选择，无需 Anthropic key：

1. `commandcode`——OpenAI 兼容网关（默认模型 `deepseek/deepseek-v4.1-flash`）。凭证来源：
   环境变量 `COMMANDCODE_API_KEY`（可选 `COMMANDCODE_BASE_URL`），或自动发现本机
   ZCode 安装的 provider 配置（`~/.zcode/v2/provider_config.json` 中的 "Command Code"）。
2. `anthropic`——设置了 `ANTHROPIC_API_KEY` 时可用。
3. `fake`——确定性脚本回放，离线演示与测试用。

### 无密钥演示（FakeProvider 一键重放修复过程）

`examples/pagination` 是一个带分页边界 bug 的微型仓库；下面的命令用确定性的 FakeProvider
脚本重放「读文件 → 改文件 → 跑测试 → 总结」的完整修复过程，不需要任何 API key。
演示会**实际修改**工作区内的文件，建议先复制一份再运行：

```bash
cp -r examples/pagination /tmp/pagination-demo   # Windows Git Bash 同样可用
minicode run "修复分页 bug" \
  --workspace /tmp/pagination-demo \
  --provider fake \
  --script examples/pagination/scripts/fix_pagination.json \
  --yes
```

PowerShell 用户请改用下面这段（续行符是反引号 `` ` `` 而非 `\`；`/tmp` 会被解析成当前盘符
根目录，建议用 `$env:TEMP`）：

```powershell
.\.venv\Scripts\Activate.ps1     # 激活后 minicode 直接可用（若报执行策略错误，见下方说明）
Copy-Item -Recurse examples\pagination $env:TEMP\pagination-demo
minicode run "修复分页 bug" --workspace $env:TEMP\pagination-demo --provider fake --script examples\pagination\scripts\fix_pagination.json --yes
```

> 若 `Activate.ps1` 报「在此系统上禁止运行脚本」，执行一次
> `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`；或跳过激活，直接用
> `.\.venv\Scripts\minicode.exe` 调用（此时脚本内的 `python` 用全局解释器）。

运行结束后会打印退出原因、轮数、token 用量与「修改摘要」（diff）；
`minicode report <会话ID>` 可查看执行记录。脚本中的测试命令假设 PATH 上的 `python`
已安装 pytest，详见 [examples/pagination/README.md](examples/pagination/README.md)。
若直接对仓库内 fixture 运行演示，可用 `git checkout -- examples/pagination/paginate.py`
恢复 bug 以便重放。

### 接真实模型

```bash
export ANTHROPIC_API_KEY=sk-ant-...        # Windows: set ANTHROPIC_API_KEY=...
minicode run "修复分页越界错误，并运行测试验证" --workspace examples/pagination --provider anthropic
```

未指定模型时，`--provider auto` 按 commandcode → anthropic → fake 的可用性顺序选择；
显式选择 Anthropic 时使用上例的 `--provider anthropic`。
不使用 `--yes` 时，`edit` / `write` / `bash` 会在每次执行前请求确认（y/N）。

## CLI 命令

| 命令 | 说明 |
| --- | --- |
| `minicode run "任务"` | 执行一个单轮任务：流式输出回复、`▸/✓/✗` 工具行、修改摘要与统计；`--acceptance` 挂验收配置后，模型自述完成不等于通过 |
| `minicode chat` | 交互式多轮会话（同一会话累积上下文）；`exit` / `quit` / Ctrl+D 退出，回合内 Ctrl+C 只取消当前轮 |
| `minicode tui` | 全屏交互界面（Textual）：流式回复、工具卡片、审批弹窗、斜杠命令自动补全（/help /model /effort /permissions /clear /new /sessions /resume /compact /exit） |
| `minicode sessions list` | 会话列表：ID、创建时间、工作区、模型、状态、轮数、token |
| `minicode resume <会话ID>` | 恢复历史会话并继续交互：已落库结果不重复执行；只读调用重新执行留痕；未知副作用标记 `unknown` 并要求模型先核实 |
| `minicode report <会话ID>` | 执行报告（text）；`--format html` 生成单文件离线 HTML（时间线、工具记录、diff、验收证据、用量） |
| `minicode eval` | 运行 `evals/` 的 20 任务评测集（FakeProvider 离线），提供 b0/b1/b2 标签；目前 b1 与 b0 相同、b2 增加外层验收失败续跑；输出 JSON + Markdown 汇总 |

常用选项（`run` / `chat` / `tui` 共享）：`--workspace`（默认当前目录）、
`--provider auto|commandcode|anthropic|fake`、`--model`（缺省按 provider 选择）、
`--script`（FakeProvider 脚本 JSON）、`--max-rounds`（默认 20）、`--max-tokens`（默认 0 =
不限制 token）、
`--max-seconds`（默认 600，**每个用户回合**的时长上限，覆盖其中所有模型轮次）、`--yes/-y`（自动允许全部工具）、
`--acceptance <yaml>`（Goal 验收：command / artifact / protected 三类检查项）、
`--db`（默认 `~/.minicode/sessions.db`）。

消息与事件随执行写入 SQLite，累计用量和结束状态在收尾时更新；Ctrl+C 取消时先落库 `cancelled` 状态再退出
（退出码 130）。预算耗尽（如 `max_rounds`）与验收不通过（`goal_not_met`）会打印明确的
退出原因，但进程退出码为 0；脚本化调用方如需区分，请解析退出原因或查询会话状态。

## P0 能力清单

- CLI 单轮执行、交互会话、流式文本展示、Ctrl+C 取消（取消前先持久化）。
- 模型适配器：CommandCode（OpenAI 兼容，默认）、Anthropic（流式）、FakeProvider（确定性脚本）。
- 基础工具（对齐 Pi Agent 的 4 核心工具）：`read`、`bash`、`edit`、`write`，外加只读辅助 `ls`、`grep`。
- 工具参数校验（pydantic schema）、工作区路径边界（含符号链接/junction）、修改与命令审批
  （ALLOW/ASK/DENY 权限门）、命令超时与输出截断。
- 会话持久化（SQLite）、逐事件执行追踪、diff 与命令退出码报告。
- 预算控制：默认限制**会话累计模型轮数**（20）与**每个用户回合时长**（600s），token 不限制；`--max-tokens N`
  可显式加一道累计输入及输出 token 上限（退出原因 token_budget）。它不等于上下文大小，
  也不是按缓存折扣计算的费用。压缩层按「模型目录窗口 − 目录最大输出」触发。退出原因可区分
  （completed / max_tokens / max_rounds / token_budget / time_budget / cancelled / goal_not_met /
  provider_error / internal_error）。

## P1 能力清单（可靠性）

- **分层上下文压缩**：超大工具输出转存为 artifact（模型看到预览 + 归档引用）→ 按完整交互单元归档早期历史
  （tool_use 与 tool_result 永不拆散）→ 压缩较旧工具结果 → 仍超阈值时生成确定性结构化摘要。
  触发阈值是模型目录中「上下文窗口 − 最大输出」的 80%，切换模型后立即生效；目录值是本地
  配置，不是向服务端实时发现的能力。压缩后的视图持久化，恢复后所见即所存。
  当前归档工具输出之前可能已发生截断，且没有模型可调用的归档回读工具；压缩不保证一定
  降到阈值内或完整保留用户约束。详细边界见 [架构说明](docs/architecture.md#52-上下文压缩与归档)。
- **会话恢复**：`resume` 加载消息/用量/轮数和保存的 provider/model/workspace；已落库结果不重复执行；只读调用重新执行并留痕
  （新事件记录）；写/Shell 类副作用不自动重放，标记「状态未知」并要求模型先核实。
- **回合限制**：wall-clock deadline 覆盖模型流、工具执行和验收命令；超时停止启动新工具，
  并等待进程树清理。模型因输出 token 上限截断时以 `max_tokens` 结束，不计为完成。
- **Goal 验收**：验收 YAML 定义命令 / 产物 / 受保护路径三类检查；模型回答后由宿主执行检查，
  通过才判定完成；失败回填结构化报告继续修复（`max_fix_attempts` 上限）；通过的证据绑定
  工作区内容指纹，代码再变即失效重验；受保护路径基线与验收配置随会话持久化，resume 不重新采样；指纹在所有检查完成后计算。
- **后台命令**：`bash` 支持 `background` 参数返回 job id，完成后作为用户消息投递。
  任务属于当前回合；回合结束或取消时终止仍在运行的进程树，记录并展示 lost 结果。
- **评测集**：`evals/` 20 个本地任务（分页边界、差一错误、除零保护等），FakeProvider 离线
  运行；b1 尚未接入压缩，与 b0 相同；b2 在 runner 外层验收失败后续跑，并非直接评测运行时
  Goal 门。结果含成功率、轮数、脚本用量与失败分析，不代表真实模型能力或 token 节省。
- **HTML 报告**：单文件、零外链、可离线打开；时间线、工具记录与 diff、验收证据表、用量。
- **TUI**：Textual 全屏界面，流式纯文本回复、工具预览卡片、审批弹窗、斜杠命令与全局
  LoadingIndicator。尚无 Markdown 回复渲染、可展开工具详情或分阶段动效；执行时输入框禁用。

## 当前实现边界

- 同一模型响应中的工具按顺序串行执行；后台命令只存活于当前用户回合，结束时清理。
- `TaskStore` 的依赖与认领能力尚未接入 Runtime，不能视为已有多代理调度。
- token 估算采用字符数 / 3，可能低估部分中文或混合内容，不是精确计数或严格上界。
- 已解析缓存读取用量，但 Anthropic 请求尚未主动配置 `cache_control`；缓存统计没有完整
  持久化，恢复会话后不能连续反映历史命中率。OpenAI 兼容协议也不保证网关支持相同缓存行为。
- 启动与 `/model` 切换采用不同的输出上限构造路径；目录最大输出与实际请求上限目前不总是一致。

## 明确未实现（P2）

- Subagents、MCP 工具接入、Skills 按需加载、Plugins 打包加载、项目指令自动加载与项目记忆。
- 多 worker worktree 协作、可续跑 workflow。
- 定时任务、Web 操作界面。

## 架构

一句话：`cli` 负责展示、配置与服务装配，`runtime` 的 AgentRuntime 驱动「流式响应 → 权限门 → 工具执行
→ 结果回填」循环，`core` 提供共享 pydantic 契约，事件与消息实时写入 SQLite。
模块图、事件流与关键语义见 [docs/architecture.md](docs/architecture.md)，
原始设计记录见 [plan.md](plan.md)；本轮功能扩展、TUI 与效率优化建议见
[探索与优化方案](docs/optimization-design.md)（提案，尚未实现）。

## 运行测试

```bash
PYTHONPATH=src python -m pytest tests        # 未安装时
pytest                                       # pip install -e ".[dev]" 之后
```

测试通过 FakeProvider、模拟 provider/client 与临时工作区/数据库验证机制，无需模型密钥，可离线运行。

## 致谢与许可

机制设计基于本地教学项目 [learn-claude-code](learn-claude-code/README-zh.md)
（s01–s17 教学主线）的阅读与改造，未直接复制其代码；项目以 MIT 许可发布。
