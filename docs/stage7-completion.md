# MiniCode 阶段 7 完成报告

版本 3.0.3；仓库 `oysterhyd/minicode`；分支 `main`。执行基线 `de0301b`，旧问题参考基线 `e22464e`。本报告覆盖 7A/7B/7C 全部 26 项。未推送、未创建 PR、未修改远程分支。

## 状态矩阵

“已修复”表示本阶段可实现部分已交付并具有回归证据；A6 明确允许保留 G3/S3 依赖，未将依赖命令伪装成成功。没有整个条目被跳过；既有正确的子能力直接复用。

### 7A：TUI 渲染与输入（9 项）

| ID | 最终状态 | 核查、根因与实现证据 | 回归测试 |
| --- | --- | --- | --- |
| T-UI9 | 已修复 | 全量 Yoga 树、备用屏/鼠标捕获影响重绘和滚屏。`tui/src/components/TranscriptLog.tsx:6` 通过公开 API 输出完成条目，活动区有界；`tui/src/terminal.ts:56` 不捕获鼠标、不进备用屏。真实 Ink 满高重绘曾擦除滚屏，现保留一行。Desktop 已有 `feed.go:68` 的 ui.List 和增量 AppendFeed，结构重建路径直接复用。 | `transcript-log.test.tsx` 1200条/10次更新不重渲染、前插历史、迟到修正；`native-scrollback.test.tsx` 非 debug TTY 连续流与长 reasoning 不发 CSI 3J；`transcript-bench.tsx` |
| T-UI12 | 已修复 | 原 feed 忽略 reasoning。`tui/src/feed.ts` 独立推理流，`App.tsx` Ctrl+R 有界尾部8行；可选真实 usage_delta、运行耗时；BridgeClient 16ms/64KiB 合并同 run/session delta，边界前 flush。 | `stage7-feed.test.ts`、`bridge.test.ts` 1000 delta 内容/顺序保持、缺元数据兼容 |
| TU5 | 已修复 | 工具展开只显示预览，未读 artifact。`tui/src/artifact-pages.ts:9` 复用 readArtifact，每页20000字符、只保留当前页/偏移栈，Unicode换行、重试、末页/返回边界。 | `artifact-pages.test.ts` 分页、Unicode、缺失归档/加载失败、长单行无损重组 |
| T-UI6 | 已修复 | 原输入缺历史/补全/导入/粘贴边界。`input-history.ts:1` 草稿恢复、PasteStore；`App.tsx` listFiles 防抖、带空格路径补全、importAttachments，失败恢复与附件隔离。 | `input-history.test.ts`、`input-ui.test.tsx` 10000行真实Ink粘贴无损发送、带空格图片路径；`file-completion.test.mjs` ignore/Unicode |
| TU7 | 已修复 | 原 Esc 无运行取消/编辑流程。`App.tsx` 单路由：审批/弹窗优先，忙时仅取消一次；空闲连续双Esc复制上条消息，不自动提交；其他按键重置双击状态。 | `stage7-keys.test.tsx` Esc优先级、取消不重复、编辑重发、队列错误恢复 |
| TU8 | 已修复 | 原字段固定，后续核查又发现分支未接线、轮数/时间预算误报不限。`StatusLine.tsx:19` 八种可配置字段；Rust Snapshot 提供真实可选branch，覆盖初始化/恢复/重置。缺费用不编造价格。 | `status-fields.test.ts` 缺失/未知/预算组合；`tests/agent/status-fields.test.mjs` 新Git仓库、run、恢复和清空 |
| T-INK | 已修复 | 原 clearTranscript 修改 Ink 私有缓存。`terminal.ts:70` 受控 CSI 2J/H，清屏 epoch 控制完成输出，不访问 Ink instances.js。 | `terminal.test.ts` 无渲染器的清屏流及输入模式 |
| TU3 | 已修复 | 原 stream-items.length 在截断后重号。`feed.ts` 进程内单调ID，流式追加保持身份。 | `stage7-feed.test.ts` 清空/截断、多run、追加不重号 |
| TU4 | 已修复 | 原窄表格无条件拼接冒号。`markdown.tsx` 缺失/空表头直接输出单元格。 | `markdown.test.tsx` 超出表头数量的窄表格 |

### 7B：运行中插话、任务与跨端语义（7 项）

| ID | 最终状态 | 核查、根因与实现证据 | 回归测试 |
| --- | --- | --- | --- |
| B6 | 已修复 | 原忙时 sendPrompt 拒绝且无服务队列。`store.rs` 队列事件投影；`runtime.rs::consume_queued` 原子提交用户消息/消费事件，在工具结果后注入；Bridge 四个队列方法、CLI/Desktop共用，取消/失败/重启暂停。 | `queue.test.mjs` 工具边界、顺序/恰好一次、自动follow-up、取消/删除、跨进程恢复；`cli-approval-boundary.test.mjs` steer/follow各一次；Desktop `TestB6DesktopUsesServerQueueWithoutClientResubmission` |
| T-UI3 | 已修复 | 原客户端只排follow-up。`App.tsx` 能力协商后普通忙时输入为steer，/follow区分模式，服务端ID删除/恢复，禁止重复客户端drain；错误与迟到响应可恢复。 | `stage7-keys.test.tsx` 两模式、无重复send、失败/非法参数 |
| TU6 | 已修复 | 原TUI仅工具行、Desktop任务页隐藏时无常驻任务。两端对话独立真实任务卡，4条预览+`/tasks`全量；task_*刷新listTasks，不重建消息。 | `tasks-card.test.tsx`；Desktop `TestTU6TasksRemainVisibleInConversation` 隐藏work页仍显示标题/状态 |
| D6 | 已修复 | 原每卡注册快捷键，部分按键消费已有保护，但无法保证首卡优先。`feed.go::approvalShortcutTarget` 唯一首卡绑定，Deciding阻止fallthrough，按钮保留单卡操作。 | `TestD6ApprovalShortcutOnlyDecidesFirstPendingCard` 多卡、焦点、处理中保护 |
| D7 | 已修复 | 原每请求spawn goroutine。`state.go::request` 16普通+2控制活动槽，两路各128等待闭包，35秒deadline含排队；正常/错误释放，控制不被普通请求饿死。 | `TestD7DesktopBoundsRequestGoroutines` 80请求峰值受限；`TestD7CancellationBypassesSaturatedNormalRequests` |
| UX1 | 已修复 | 原匹配/重置/取消/审批语义散落。`store.rs:216` 共享完整ID/唯一前缀；reset参数先校验，取消采用平台映射；[交互规范](interaction-spec.md) 为实际契约。 | `interaction.test.mjs`；`commands-stage7.test.mjs` 无效clear保留会话；Desktop队列非法参数；三端输入测试 |
| A6 | 已修复 | /clear曾只清屏。/clear=/new、/cls保留旧能力；`commands.rs:33` 提供真实统计/诊断/init/现有memory/tasks/agents/mcp/export；G3命令明确依赖，不伪实现。 | `commands-stage7.test.mjs` 重置/保留历史、诊断、init不覆盖、G3错误、非法参数；Desktop迟到分页清屏回归 |

### 7C：CLI 与提示词（10 项）

| ID | 最终状态 | 核查、根因与实现证据 | 回归测试 |
| --- | --- | --- | --- |
| L1 | 已修复 | 原clap必填子命令。`cli.rs` 可选子命令，默认走现有TUI启动器，显式子命令不变。 | `l1_no_subcommand_is_accepted` |
| L7 | 已修复 | 原完整output_detail刷屏。`cli_output.rs:4` 默认10行/2000字符、错误独立保留，-v/--verbose全量，过滤终端控制。 | Rust输出单测；`cli-output.test.mjs` 真实进程摘要/verbose |
| L11 | 已修复 | 原仅run监听首次Ctrl+C，idle不消费，取消中无法强退。`cli.rs` idle/run均处理信号及编辑器Interrupted；首次取消、再次exit130，空闲双击退出。 | `l11_editor_interrupt_cancels_during_approval`；`cli-pty.py` 原生Windows空闲/运行/强退 |
| L12 | 已修复 | 原read_line消费预输入y。`cli_approval.rs:7` 当前请求专属确认码；A/P按可记住规则限定，EOF拒绝、首卡优先，普通文字保留/持久暂停队列。 | Rust预输入/错误码/参数/记住范围；`cli-approval-boundary.test.mjs` 真实审批、预输入不写文件、连续队列、退出后待提交消息可恢复 |
| L13 | 已修复 | 原stdin无编辑历史，空resume传空前缀。`console.rs:49` rustyline持久历史/补全/多行，管道兼容；空resume列20条及完整ID。 | console单测；`cli-resume-list.test.mjs`；`cli-pty.py` 方向键/光标/多行历史 |
| L14 | 已修复 | 原只检查off空格前缀。`commands.rs:17` split_whitespace共享解析，on/off与名称简写严格计数。 | `skill-arguments.test.mjs` 缺名、多参、Tab、多空格、激活/停用 |
| L15 | 已修复 | 原恢复两路径预算优先级不同且全量events。`commands.rs:6` 统一restored_budget→`Store.saved_budget`索引查询；Option保留显式0。 | `l15_resume_keeps_explicit_cli_budget`、`l15_old_sessions_missing_and_explicit_zero` |
| L17 | 已修复 | 原[..8]可跨UTF8/短ID panic。`util.rs:15` short_id安全截取失败回退完整ID，匹配仍用真实ID。 | `l17_short_ids_never_split_utf8_or_panic` 空/短/中文/emoji |
| A2 | 已修复 | 原两平台模板要求中文总结。双平台改为用户语言，显式输出语言优先。 | `prompt-language.test.mjs` 两平台提示契约；未用fake输出冒充真实模型语言评测 |
| A3 | 已修复 | 原已有全局/项目/作用域指令加载，缺环境/工程行为。`environment.rs:77` 注入日期、OS/Shell版本、cwd、分支/改动布尔，包含未跟踪工作；OS/Shell缓存、Git2秒/32项；保留Catalog约定，不转储环境/密钥。 | `a3_prompt_has_environment_and_preserves_project_instructions` 环境、约定、.env不泄露；`a3_environment_counts_untracked_work_without_disclosing_names` 仅未跟踪文件状态；既有Provider截断契约回归 |

测试文件无目录前缀时，`.test.ts(x)` 位于 `tui/tests/`，`.test.mjs` 位于 `tests/agent/`；Go 测试位于 `desktop/native/stage7_test.go`。

## 执行顺序与设计

先核查工作区与现有阶段1 runId、getSession分页、stderr通道，建立 UX1 初版契约；未重复实现这些前置能力。实际按依赖调整顺序：TU3/TU4/T-INK → T-UI9/T-UI12/TU5/T-UI6/TU7/TU8 → B6核心 → D6/D7/T-UI3/TU6 → UX1共享匹配/A6 → 7C → 三端集成及性能/信号回归 → TU8/UX1/A3最终状态审计。这样避免在队列基础未就绪时让客户端各自实现steering，也避免滚屏依赖私有清屏缓存。

直接复用的核查证据：Desktop `state.go:647` rebuildRows为结构变化路径，`state.go:672`和`internal/model/feed.go:258` AppendFeed仅更新尾部，`feed.go:68`使用ui.List虚拟化；`optimization_regression_test.go:10::TestIncrementalFeedMatchesHistoryGrouping`验证增量结果。A3复用`agent/extensions.rs::Catalog::prompt`（约1237行）的全局/根项目约定、已有作用域注入，`extensions.rs:1289`回归保证约定仍在。它们是条目内既有能力复用，不将整个条目标记为“已跳过”。

- 终端输出选择公开的 renderToString/useStdout.write，而非 Static 的保留缓存：完成项脱离活动树，/cls不触碰私有实现；稳定ID+浅属性比较处理分页前插及迟到修正。仍存在轻量列表扫描，未声称整个应用所有操作均为O(1)。首次恢复/输出1200条本身仍有线性成本。
- 队列复用持久化事件和checkpoint，不新增数据库schema；新方法/事件/字段可选、协议仍为2。按模式定义FIFO，控制请求不持有会话运行锁，取消/失败的剩余消息须显式恢复。
- 任务、附件、归档、预算复用现有核心。Desktop的完整文件树和模糊补全继续使用已有工作区缓存（`composer.go:71`、`internal/workspace/workspace.go:144`）；TUI使用共享listFiles，未用仅200条查询结果替代Desktop完整树。这是保守兼容取舍，Desktop尚未迁移查询适配器。
- 唯一新增Rust直接依赖 rustyline 18.0.1，用于跨平台TTY行编辑/粘贴/历史；关闭不必要默认feature，仅开启file-history/custom-bindings。Python pywinpty仅本机测试前提，不是产品运行依赖。
- CLI审批选择携码确认，牺牲裸单键的便捷性换取可靠的预输入边界和不丢普通消息。平台按键映射、/clear迁移、费用不可用和G3依赖均在交互规范明确。

## 验证、性能与限制

最终验收命令、退出码、耗时、工具链及日志SHA256见 [机器记录](stage7-acceptance.json)。本地原始日志位于 `output/stage7/release-acceptance/`（忽略的可再生产物），评测使用新目录，避免与已有配置混用。

环境：Windows 11（10.0.26200.9711）、PowerShell 7.6.9、rustc 1.99.0、Node 22.22.0、Go 1.27.1、Python 3.12.4；原生终端测试使用本机 pywinpty 2.0.10/ConPTY。

| 检查 | 最终结果 |
| --- | --- |
| cargo fmt/clippy/test/build（locked，Clippy -D warnings） | 通过；Rust **226通过、2忽略、0失败** |
| node --test tests/agent/*.test.mjs | 最后独立整轮 **307通过、3跳过、0失败**；310测试，83.14秒；此前两轮有数据库初始化竞争，见下文 |
| 生成任务/trace、fixture git diff | 通过，未修改评测夹具 |
| 离线任务 b0/b1/b2 | **60/60**（各20/20） |
| 场景 b0/b1/b2 | **7/9**（2/3、2/3、3/3），符合预期基线；verify-results退出0，不声称9/9 |
| npm ci、TUI build、真实Bridge TUI tests | 通过；**142/142** |
| Desktop go test/vet、vendored mygo/ui tests、Windows build.ps1 | 全部通过；生成output/native/MiniCode.exe |
| Windows原生ConPTY | **4/4**，4.939秒；行编辑/多行历史、run后空闲中断、运行取消、取消中强退130 |

最后完整验收的初次协议步骤为304通过/3失败/3跳过，复跑为306通过/1失败/3跳过，错误分别为`database is locked`和`duplicate column auto_title`，均在旧测试的并发首次建库阶段。最后独立整轮307通过，但**此前失败日志保留，不能认为该既有竞争已修复或CI已经稳定**。阶段7未修改Store::open（对比de0301b仅新增后面的匹配/队列函数），也未削弱这些旧测试。停止重复验收后以现有结果交付。

Rust忽略项是已有T2 ignored-tree、U5 path-depth手工基准；Node跳过项为两个Unix专用发现/进程组测试和可选200秒流式慢测。这些不计为通过。实际CI来自`.github/workflows/tests.yml`，额外执行Desktop构建、原生PTY和本阶段基准；评测输出目录换成独立新目录，参数不变。移除宿主FORCE_COLOR并把真正的pwsh7加入PATH；PTY设TERM=xterm-256color。

| 可重复基准（同一公开Ink渲染器、80列、每组20次平均） | 修复前模型 | 修复后模型 |
| --- | --- | --- |
| 1200条完成消息+活动delta，每次树渲染 | 100.276415ms（全树） | 0.232620ms（活动树），降低约99.77% |
| 10000行文本的输入布局promptRows | 194.333035ms（原始草稿） | 0.020940ms（9字符折叠引用） |

复现：`tui/node_modules/.bin/tsx.cmd --tsconfig tui/tsconfig.json tui/tests/transcript-bench.tsx`。这是**隔离的树渲染/输入布局比较，不是完整App帧率或网络延迟**；真实Ink回归另外证明1200完成条目不重复渲染和不擦除滚屏，真实输入回归证明10000行提交无损。首次回放成本未被隐藏。Desktop请求原80并发样例收敛为普通≤16，控制另≤2，等待闭包每路≤128。

## 风险、待确认决策和后续依赖

- 本机为Windows；未执行Linux/macOS原生PTY或Linux CI任务，不将跨平台实现等同于跨平台实测。
- 原生终端复制/滚屏通过模式字节及非debug Ink无CSI3J回归验证；没有通过人工鼠标选择/真实系统剪贴板验收。图片支持路径导入，未实现任意终端的二进制剪贴板协议。
- 长reasoning保留真实流，但展开显示有界尾部；全量推理历史查看不是本阶段新增能力。预算/token来自真实状态，缺定价则费用不可用；未调用付费真实模型验证输出语言。
- Desktop完整文件缓存保持既有行为；后续可扩展listFiles分页/模糊评分后统一适配，不将200条候选当作完整工作区。
- G3负责checkpoint回滚/undo/rewind/fork；G4负责高级任务编辑/依赖；S3负责高级记忆管理。当前命令明确边界，没有虚构成功或数据。
- 既有跨进程首次建库可出现 `database is locked` / `duplicate column auto_title`；本轮实际遇到且保留失败记录。新CLI测试先等待Bridge初始化，不扩大本阶段至存储迁移改造；建议下一阶段优先处理迁移互斥与幂等性。
- 队列原子性保证用户消息/消费事件不重复插入，不承诺崩溃前外部工具副作用的全局恰好一次。
- 初始用户文件 `desktop/native/internal/bridge/home_priority_test.go.pending`、`tests/agent/windows-home.test.mjs.pending` 保持未跟踪且未修改。
- 每项先建立回归并运行定向检查；完整CI集中于集成阶段执行，**没有在此前每一条历史提交前重复完整CI**，这一过程与原要求有差异。最终集成以实际日志为准，失败未通过弱化断言掩盖。

## Git 提交

以下按实际顺序列出每个条目的独立提交和集成补充；同ID可有多次后续修复。最终文档提交不改变运行代码。

| Commit | 提交说明 |
| --- | --- |
| 8a496c2 | fix(tui): TU3 use monotonic streaming IDs |
| 2b4103c | fix(tui): TU4 omit empty narrow table headings |
| add8497 | fix(tui): T-INK clear with controlled terminal output |
| 58ddc12 | fix(tui): T-UI9 stream sealed transcript to native scrollback |
| 8ccd271 | fix(tui): T-UI12 display collapsible reasoning and streamed usage |
| 674e9a5 | fix(tui): TU5 read complete tool artifacts by page |
| 972b552 | fix(tui): T-UI6 add history file completion and folded paste |
| 986b18e | fix(tui): TU7 prioritize escape cancellation and message editing |
| 8ff27c3 | fix(tui): TU8 configure status fields with safe fallbacks |
| 91d44a9 | fix(bridge): B6 persist steering and follow-up queues |
| dc0503d | fix(desktop): D6 scope approval shortcuts to first pending card |
| 9e06c11 | fix(desktop): D7 bound bridge request workers and pending work |
| 472e094 | fix(tui): T-UI3 use server steering and follow-up queues |
| c82e208 | fix(ui): TU6 keep task progress visible in conversation |
| 098f922 | fix(interaction): UX1 share session prefix resolution and document semantics |
| 24d0071 | fix(commands): A6 unify context reset and supported slash commands |
| 691da9c | fix(cli): L1 launch TUI when no subcommand is provided |
| a76cbb5 | fix(cli): L7 bound tool previews and add verbose output |
| 5205818 | fix(commands): L14 normalize and validate skill subcommands |
| 42ec3e0 | fix(cli): L17 display short and Unicode session IDs safely |
| 1652c97 | fix(prompts): A2 follow the user requested language |
| 6d550e9 | fix(cli): L15 restore budgets through indexed saved state |
| 5908453 | fix(prompts): A3 inject bounded environment and engineering guidance |
| 792046d | fix(cli): L13 add line editing history and resume selection |
| b98c33c | fix(cli): L11 handle interrupts during runs and idle input |
| 9d8ce35 | fix(cli): L12 require request-specific approval confirmation |
| 002ce55 | fix(tui): T-UI12 batch stream deltas without reordering boundaries |
| 450135c | fix(tui): TU5 keep long artifact lines readable within the viewport |
| 6037c62 | fix(desktop): D7 reserve bounded capacity for cancellation and approvals |
| 3c9434f | fix(desktop): B6 adopt server-owned prompt queues in capable clients |
| 496edb5 | fix(runtime): B6 recover durable queues and share CLI steering semantics |
| b5d6413 | fix(tui): T-UI6 preserve compact paste drafts and pending attachments |
| be359f9 | fix(ui): TU6 show task status and expose the full task list |
| 697fd11 | fix(prompts): A3 retain provider truncation contract with equivalent guidance |
| 8ba7df5 | fix(cli): L15 place budget regression tests after implementation |
| c054990 | fix(desktop): A6 test display-only clearing through the migrated alias |
| 13fa0ed | fix(cli): L7 synchronize CLI fixture database initialization |
| d961afb | fix(tui): T-UI9 retain prepended history and corrected completed rows |
| 9df4dbf | fix(cli): L13 verify line editing and interrupts in native ConPTY |
| bdf3131 | fix(interaction): UX1 preserve selection when reset arguments are invalid |
| b45241b | fix(tui): T-UI3 recover queue failures and reject malformed controls |
| a5d4313 | fix(tui): T-UI9 preserve scrollback across real Ink redraws |
| b9f14b8 | fix(cli): L12 persist unsubmitted ordinary input from single-task runs |
| f3bf071 | fix(tui): UX1 describe native scrollback in shortcut hints |
| 52b9aaa | fix(tui): T-UI6 benchmark large pasted input layout |
| 8a51d9d | fix(status): TU8 supply real branch and complete budget constraints |
| 2a360ce | fix(desktop): UX1 reject malformed queue controls |
| a4b1210 | fix(prompts): A3 include untracked work in environment status |
