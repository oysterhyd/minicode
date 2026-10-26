# MiniCode 3.0.3：测试脚手架与运行时止血（批次 0A）

阶段 0「止血」第 1 批。目标是消除会话永久损坏、长任务必失败、进程崩溃后一直「运行中」以及遗留孤儿进程这几类不可恢复的问题，同时搭好后续阶段共用的测试脚手架。每个条目都先写失败测试，确认在 v3.0.2（e22464e）上失败，再修复。协议、存储和配置保持向后兼容：没有新的必填字段，持久化历史不改写。

## 具体修复

| ID | 原问题 | 根因 | 修复后的行为 |
| --- | --- | --- | --- |
| E4 | 运行时主循环没有集成测试，也没有可复用的 provider / SSE / bridge 测试工具，多数运行时问题无法写回归测试。 | 只有工具级单元测试和少量写死响应的 HTTP 夹具。 | 新增三件测试工具：Rust `Harness`（真实 Runtime + 脚本 provider + 事件捕获 + Anthropic payload 断言）；`tests/agent/sse-mock.mjs`（可控制帧间隔、首字节前静默、状态码、Retry-After、中途断流和挂起）；bridge helper `settle` / `expectSequence` / `pidAlive` / `until`。fake 脚本新增 `error` / `panic` 轮次。 |
| R40 | 截断续写、停滞提示、后台任务完成、恢复回填 / 失联通知这 4 处会紧接 user tool_result 再追加一条 user 通知。严格交替的 Anthropic 端点（Bedrock、部分网关）直接返回 400，消息已持久化，会话对该端点永久不可用。 | anthropic payload 逐条映射消息，没有合并同角色消息。 | anthropic 分支在 payload 末端合并相邻同角色消息，并把 tool_result 稳定地排在 user 消息最前。只作用于发送前的 payload，已经「损坏」的旧会话也能直接恢复。OpenAI 兼容分支不变。 |
| P1 | 输出超过 180 秒的流式请求一律失败，384k max_output 这类配置不可能跑完。 | reqwest `.timeout(180s)` 是整个请求的总时限，包含流式 body。 | Client 只保留 `connect_timeout`。等待响应头和每次读取 body 都使用空闲超时：默认 120 秒，可用 `MINICODE_PROVIDER_IDLE_TIMEOUT_S` 覆盖，收到数据即重新计时。错误文本为 `provider stream idle timeout`，与 deadline 到期区分。尚未收到 delta 时，空闲超时仍可重试。 |
| R7 | 与 P1 叠加：运行时把整个 `provider.stream()` 包在 `min(deadline, 180s)` 中，超时报 `provider request timed out`；已收到 delta 不会重试，整个 run 失败。 | 外层计时器限制的是总时长，不是空闲时间。 | 只用剩余 turn deadline 包住请求，到期报 `turn deadline exceeded`（exit_reason 为 time_budget）。没有 deadline 时不设总时限。 |
| R41 | 收尾时任一步存储出错就提前返回，跳过关闭 MCP、清理审批、写 session_end 和复位 running；剩余后台任务的取消 token 被丢弃，进程成为孤儿。 | 收尾代码逐步使用 `?`。 | 先取消全部后台任务，再逐个等待进程树结束并记录事件。关闭 MCP、清理审批、写 session_end、刷新快照、复位 running 每步都会执行。失败集中收集，连同原始错误以 Err 上报，bridge 据此发出 run_error。 |
| B16 | run 任务 panic 后不会发出 run_done / run_error，`running` 永远为 true，界面一直提示「当前任务仍在运行」，只能重启；后台任务与 MCP 子进程也被遗留。 | `tokio::spawn` 中的 panic 没有人观察。 | `Runtime::run` 对主循环使用 `catch_unwind`，panic 后照常走 R41 的收尾（session_end 记为 internal_error），再以 Err 上报。bridge 对整个 run 任务再兜底一次：panic 时取消 token、关闭 MCP、清理审批、复位 running / pending 并发 run_error。之后同一会话可以直接继续。 |

## 回归测试

| ID | 测试 | 覆盖场景 |
| --- | --- | --- |
| E4 | `harness_fake_provider_scripts_errors_tool_calls_and_usage`；`streaming.test.mjs` 中的 3 个 scaffold 用例 | 脚本错误先重试再完成，工具调用与用量累计；SSE 节奏与事件顺序；429（Retry-After）/ 503 重试；输出可见后断流时不重试。 |
| R40 | `anthropic_roles_alternate_after_{truncated_response,stall_notice,background_completion_notice,recovery_backfill_and_lost_job}`；`anthropic_normalization_merges_and_orders_blocks_without_touching_history`；`payload.test.mjs` | 4 个注入点的 payload 中没有相邻同角色消息，且 tool_result 位于 user 消息开头。归一化不修改输入历史，OpenAI 分支不变。通过 bridge 发送到 SSE mock 的真实请求同样严格交替。 |
| P1 / R7 | `streaming.test.mjs`（空闲超时设为 1 秒） | 4.5 秒内持续有字节的流正常完成；输出可见后静默，以「idle timeout」失败且不重试；首字节前静默，按空闲超时重试；turn deadline 单独报告为 time_budget。验收用例（每 2 秒一个 delta，共 200 秒，`MINICODE_SLOW_TESTS=1` 启用）在本机上：v3.0.2 约 180 秒时以 `provider stream interrupted` 失败，修复后约 207 秒完成。 |
| R41 | `teardown_continues_after_a_failed_background_job_event`；`teardown_reports_a_failed_session_end_and_resets_running`；`lifecycle.test.mjs` 存储错误用例 | 用 SQLite 触发器注入 background_job_lost / session_end 写入失败：后台任务已取消，running 已复位，审批已清空，MCP 已关闭，错误中包含注入原因；bridge 收到 run_error，后台任务进程和 MCP 子进程都已退出。 |
| B16 | `a_panicking_run_is_torn_down_and_reported`；`lifecycle.test.mjs` panic 用例 | 脚本 panic：返回 Err，session_end 为 internal_error，后台任务记为 lost；bridge 收到 run_error，running 为 false，两个子进程都已退出，同一会话的下一条消息正常完成。修复前 bridge 不会产生任何终止事件，后台任务进程被遗留。 |

## 待确认决策

- 空闲超时默认 120 秒（建议范围 90–120 秒中较保守的一端），目前只能用环境变量 `MINICODE_PROVIDER_IDLE_TIMEOUT_S` 配置，服务配置中还没有对应字段。若某个兼容网关在首字节前静默超过 120 秒（以前 180 秒以内可以成功），现在会被判为超时并重试。
- panic 在收尾后以 Err（run_error，会话标记为 paused）上报，而不是 run_done 加 internal_error，这是为了让界面能区分程序缺陷和普通失败。session_end 仍记录 internal_error。
- fake provider 新增 `error` / `panic` 脚本字段，只用于测试。只有脚本出现这两个字段时，它们才计入恢复身份，旧脚本的恢复身份不变。

## 本批未处理的发现

- Windows 进程树：命令的 Job Object 在 spawn 之后才绑定。本机的 `pwsh` 解析到 Store 应用别名（`WindowsApps\pwsh.exe`）。在并行测试负载下，约 1/5 的运行会出现后台 `node` 进程逃出 Job，取消后仍存活并持有输出管道，导致 `cargo test` 不退出。测试已改用有限寿命的命令，问题本身留给后续批次：可以用挂起方式创建进程，绑定 Job 后再恢复执行。
- 通过 SSE 返回的 `max_tokens` 响应会丢弃工具调用，因此不会回填 tool_result；fake provider 则保留调用并回填。两者行为不一致，fake 测试覆盖不到这条真实路径。

# MiniCode 3.0.3：数据安全与客户端止血（批次 0B）

阶段 0「止血」第 2 批（最后一批），解决以下问题：会话恢复永久失败、静默覆盖用户文件、grep 让进程崩溃、打开仓库即执行任意命令，以及两个客户端的误操作与兼容性问题。与 0A 相同，每个条目先写失败测试，确认在修复前失败，再修复。协议、存储和配置都向后兼容：新增字段全部可选，旧会话直接恢复，旧配置无需迁移。

## 具体修复

| ID | 原问题 | 根因 | 修复后的行为 |
| --- | --- | --- | --- |
| R1 | 恢复会话时，AGENTS.md 被修改或删除、已激活 skill 内容变化或被卸载，都会 bail（`project instructions changed since session start`），会话再也无法继续。agent 自己改 AGENTS.md 也会触发。子代理恢复失败会让父 run 以 internal_error 结束。 | `Runtime::new` 把指纹不一致当作致命错误。 | 恢复时重新 pin 当前版本（文件删除即移除，新增即加入），在下一次请求前记录 `project_instructions_changed` 事件（列出 changed / deleted / added / skill changed / skill removed），并追加一条 `<system-reminder>` 告诉模型变化内容。系统提示中已经是新指令。之后的恢复以重新 pin 的指纹为准，不会重复提醒。子代理走同一路径。 |
| T3 | 普通大小文件的 edit / write 不要求先 read，也不检测读后外部修改，会覆盖用户在 IDE 里尚未保存的改动；write 可以直接覆盖从未读过的已有文件。并发编辑同一文件会丢失更新（Windows 上还会因 rename 冲突报拒绝访问）。 | 只有 edit_large 检查 mtime，并且只覆盖本次编辑期间的变化。 | 每个会话维护读取账本（路径 → 大小、mtime、内容哈希），以最近一次 read 或本会话成功写入后的状态为准。以下情况报错并要求重新 read：已有文件本会话从未读过（会话恢复后同样需要重新读取）；或者文件在读取后被外部修改（大小和 mtime 都相同时直接复用账本记录，否则重新计算哈希）。新建文件不受限制。edit_large 的 mtime 检查并入同一套逻辑。同一文件的写入按路径串行化，子代理和并行委派共享父会话的账本。Windows 上替换刚被替换过的文件时，rename 可能短暂返回拒绝访问，`atomic_write` 对此最多重试 20 次。 |
| T7 | grep 遍历时，单个文件的 `metadata()?` 或 `Mmap::map(...)?` 出错会中止整个搜索。对被并发截断的文件做 unsafe mmap，读取时触发 SIGBUS（Windows 上为 in-page error），整个 agent 进程崩溃。 | 错误用 `?` 直接上抛；mmap 工作区文件属于未定义行为。 | 单个文件的打开或读取错误都会跳过并计数，结果末尾注明「跳过 N 个无法读取的文件」。改为按行缓冲读取，不再 mmap 工作区文件；文件被并发截断时只会提前读到 EOF。行号和换行语义保持不变（CRLF 计为一行，同样支持各种 Unicode 行分隔符，文件末尾的换行符之后仍算一行空行）。 |
| SEC1 | 工作区 `.minicode/plugins/*/plugin.json` 中 `enabled: true` 的插件所声明的 MCP 服务会在每次 run 开始时自动启动，不需要任何确认。`plugins.lock.json` 位于工作区内，只有存在时才校验。攻击者只要在仓库里放一个插件，用户发送第一条消息时就会执行任意命令。 | 没有工作区信任边界。 | 插件声明的 MCP 服务只有在用户信任该插件当前指纹（整个插件目录的 fingerprint）之后才会进入 `catalog.servers`，未信任时不启动任何进程。信任库位于用户目录（`~/.minicode/trusted-plugins.json`，与 desktop-config.json 同目录），按「工作区路径 + 插件指纹」记录，插件内容变化后重新询问。run 开始时通过现有审批通道询问（`tool_name: workspace_trust`，列出插件、版本和要执行的命令）。bypass 权限模式和「本会话始终允许」都不算信任；拒绝或输入已关闭时只是不启动服务，并记录 `workspace_trust_required` 事件和提示。bridge 新增 `getWorkspaceTrust`、`trustWorkspace`（参数为 `plugins: [{name, sha256}]`、`trusted`；sha256 与当前指纹不一致时拒绝）。CLI 新增 `minicode plugins trust [--revoke]`，`plugins list` 会标出未信任的插件。TUI 和 Desktop 的审批卡片为信任请求使用专门的标题和按钮，不提供「始终允许」。 |
| D-UX1 | Desktop 任务运行中，在空输入框按发送键（Enter）会直接取消任务，误触或输入法上屏时的回车都会打断长任务。 | `submit()` 把「运行中 + 空草稿」当作取消。 | 运行中在空输入框按 Enter 不做任何事。取消只能通过停止按钮（现在直接调用 cancel）和已有的 Cmd+.（Windows 为 Ctrl+.）。 |
| B-COMPAT | TUI 收到未知事件类型时调用 `fail`，整个 bridge 被判为致命错误；agent 先升级、前端未升级时 TUI 直接不可用。握手中的 `protocolVersion: 2` 两端都不校验。 | 事件类型使用白名单，未知即致命；握手不包含版本协商。 | TUI 忽略未知事件类型，记入 `ignoredEvents`（设置 `MINICODE_TUI_DEBUG` 时写 stderr），连接保持可用；`event` 字段不是字符串时仍视为无效消息。`initialize` 新增 `protocolVersion`、`minClientProtocol`、`capabilities`（可选字段，旧客户端会忽略）。TUI 和 Desktop 都与自身支持的范围（2–2）比较：agent 过旧或要求更新的客户端时显示错误提示，agent 较新时显示警告，连接都不中断。缺少版本号时视为兼容。Desktop 原本就会忽略未知事件，本批补了回归测试。 |

## 回归测试

| ID | 测试 | 覆盖场景 |
| --- | --- | --- |
| R1 | `resume_repins_changed_instructions_and_skills`；`resumed_subagent_repins_changed_instructions`；`extensions.test.mjs`「changed project instructions are re-pinned on resume…」（替换了原来断言 bail 的用例） | 修改 AGENTS.md、删除已激活 skill 后恢复成功，模型收到 `<system-reminder>`，系统提示中是新内容，skill 已卸载；再次恢复不会重复提醒；删除 AGENTS.md 后的恢复记录 deleted，提示中不再包含旧指令。暂停的子代理在指令变化后恢复，父 run 正常完成，子会话中有变化事件和提醒。bridge 层 resetSession → getState → 继续发送消息均成功。 |
| T3 | `edit_and_write_require_a_current_read`；`concurrent_edits_of_one_file_are_serialized`；`ambiguous_edit_is_atomic` / `multiline_edits_preserve_crlf_for_small_and_large_files` 改为先 read | 未读过的已有文件，write、edit、大文件 edit 都被拒绝，文件不变；新建文件不受限，自己写过的文件可以继续 edit；read 之后外部修改（小文件和大文件）时 edit / write 报「外部修改」，重新 read 后可以编辑。8 个并发 edit 同一文件，20 轮全部成功且没有丢失更新（修复前 Windows 上报 `Access is denied`）。 |
| T7 | `grep_skips_unreadable_files_and_reports_them`；`grep_survives_concurrent_truncation`（仅 Unix） | Windows 上用强制锁、Unix 上用 0o000 权限让文件不可读：搜索正常返回其余匹配，并注明跳过 1 个文件。另一个线程反复截断一个 2.4 MB 文件，同时执行 50 次 grep，进程不崩溃。 |
| SEC1 | `untrusted_workspace_plugins_start_no_processes`；`trusted_workspace_plugins_start_until_they_change`；`extensions.test.mjs`「workspace plugin MCP servers start only after…」；TUI「workspace trust requests show the plugin commands and can be declined」；Desktop `TestWorkspaceTrustApprovalCard`；原有两个插件 MCP 用例改为先 trustWorkspace | bypass 模式加上「始终允许 workspace_trust」仍然会询问；拒绝后不产生子进程，也不写信任库。同意后服务启动，信任写入用户目录，下一次 run 不再询问；修改 manifest 后再次询问。bridge 层：未回答前没有子进程，拒绝后没有子进程，trustWorkspace 之后启动；修改 manifest 后 getWorkspaceTrust 重新标记 trustRequired，用旧 sha256 授信会被拒绝。TUI 展示插件和命令，按 n 拒绝；Desktop 卡片有「信任并启动」，没有「始终允许」。 |
| D-UX1 | `TestEmptyEnterDoesNotCancelARunningTask` | 运行中在空输入框按 Enter 或调用 submit 都不会取消任务；点击停止按钮仍然取消。 |
| B-COMPAT | TUI `unknown bridge events are ignored without failing the connection`、`protocol versions are compared with an upgrade hint instead of a crash`；`lifecycle.test.mjs`「initialize reports the protocol version…」；Desktop `TestProtocolMismatchIsAHintAndUnknownEventsAreIgnored` | 收到未知事件后后续请求正常，不产生致命 bridge_error；版本过旧、要求更新的客户端、agent 较新三种情况分别给出 error / error / warning 提示；agent 的 initialize 包含 protocolVersion、minClientProtocol、capabilities；Desktop 收到未知事件后会话不变。 |

## 待确认决策

- SEC1：询问发生在 run 内部，和工具审批一样会阻塞本次 run，直到用户回答、取消或 turn deadline 到期。CLI 在输入关闭时自动拒绝。没有界面的 bridge 客户端会一直等待。另一种方案是不阻塞：直接跳过，由客户端事后弹窗。
- SEC1：信任粒度是整个插件目录的 fingerprint，比只看 manifest 摘要更严格，插件代码的任何改动都会重新询问。只有声明了 MCP 服务的插件需要信任；插件里的 skills / agents 只是提示词文本，未信任时仍会加载（与 `.minicode/skills` 一致）。撤销信任目前只能用 CLI `--revoke` 或 bridge `trusted: false`，界面上没有入口。
- T3：账本只在内存中，进程重启或恢复会话后，必须重新 read 才能 edit / write 已有文件。错误信息里写明了这一点。另一种方案是把账本持久化到事件中。
- T3：账本采用快速路径：大小和 mtime 都相同时，不重新计算哈希。在 mtime 精度粗的文件系统上，如果内容在同一时间刻度内变化且大小不变，这类变化检测不到。阶段 3 的 T8 统一改为哈希时会消除这个缺口。
- D-UX1：空回车静默无反应，不弹提示，以免连按回车时刷出一串 toast。
- B-COMPAT：两端支持的协议范围都是 2–2，缺少版本号时视为兼容。新增的 `capabilities` 目前只是声明，客户端还没有据此开关功能。

## 本批未处理的发现

- 会话进行中，如果已激活 skill 的文件被修改，`Catalog::prompt()` 和 `Catalog::refresh()` 仍会 bail（`activated skill changed since session start`），之后每一轮都失败，直到恢复会话（R1 会在恢复时重新 pin）。`Catalog::load` 在同一会话内重新加载已变化的 skill 也会 bail。
- `edit_large`（超过 256 KB 的文件）仍然对工作区文件做 mmap，被并发截断时同样有 SIGBUS 风险。T7 只改了 grep，建议并入阶段 3 的 T8。
- 本地 `.mcp.json` 等导入的 MCP 需要手动启用（默认关闭），但启用记录的键只包含路径和名称，不包含内容摘要。启用之后再改命令，不会重新询问。
- bridge 和 CLI 在新建会话时沿用上一个会话的已激活 skill，遇到内容变化仍会 bail（`activated skill changed before session start`，`bridge.rs` 与 `cli.rs` 中各有一处）。这条路径不属于恢复会话，本批没有处理。
- 后台 bash 任务写文件不经过账本，只能在模型下次 edit / write 时通过哈希发现。这符合设计，但如果模型用 bash 改写自己读过的文件，下一次 edit 会要求重新 read。

# MiniCode 3.0.3：Bridge 并发、大会话与错误通道（批次 1A）

前置 R41/B16 已存在：`agent/tests.rs` 的收尾失败和 panic 用例，以及 `bridge.rs::start` 的 catch_unwind 守卫。本批保持这些守卫。

## B5：请求不再持有全局 Router 锁

- 问题与根因：serve 对完整 handle 加锁，会话锁外又套全局锁；cancelTurn / resolveApproval 还等待会话锁。
- 修复：Router 的读写锁只保护映射与 LRU；每个 Owner 独立串行化会话请求。取消 token、审批 oneshot 和共享 settings/snapshot 独立持有，控制请求不获取会话锁。加载前预留 session 映射，避免两个客户端同时构造同一会话；更新映射时仍持有会话锁，避免旧请求覆盖新映射。配置写请求独立串行化，保留原来的读改写顺序；缓存不驱逐正在处理请求的 Owner。关闭时先取消所有 run，再等待收尾。
- 测试：`controls_do_not_wait_for_the_conversation_lock` 修复前在 100ms 超时，修复后在会话锁被占用时取消和审批均完成；`one_locked_conversation_does_not_block_another` 验证另一会话仍可响应。

## B14：大会话按需分页

- 问题与根因：getSession 同时反序列化全部 messages/events，再序列化成一行；selectSession 为显示首屏也先构造完整 Runtime。
- 修复：getSession 默认每类最多 100 条，按 seq 倒序查询再按时间返回；每类原始 JSON 预算 2MB。新增可选 `cursor: {messages, events}`、`messageOffset` 和 capability `sessionPaging`；getSessionPage 接受两个独立的排他游标，null 表示该类已读完，可单独拉取消息或事件。保留 summary/messages/events 结构，旧数据库无需迁移。两端在 selectSession 传可选 deferRuntime，只加载摘要、统计和预算，Runtime 延迟到执行请求时创建；未传该参数保持原有行为。激活 skill 使用事件投影恢复，无需全量历史。TUI `/history` 和 Desktop「加载更早历史」显式拉取一页，合并时保留现场新增的消息；历史消息 ID 使用持久化 seq 偏移，避免分页重号；与原始快照比较后保留已显示工具的实时更新，避免迟到分页把结果改回旧值。
- 测试：`tests/agent/history.test.mjs` 写入 242 条消息、70MiB 事件，修复前首屏全量加载的断言失败；修复后验证首屏小于 4MiB、逐页完整且无重复、非法游标被拒绝、选择会话不加载完整 Runtime。TUI `history.test.tsx`、Desktop `TestHistoryLoadsEarlierPagesOnDemand` 均先复现无历史入口，再验证按需加载。`TestClearWhileHistoryPageLoadsDoesNotRestoreOrPanic` 先复现 /clear 与迟到分页响应导致越界，再验证清屏后忽略旧响应。两端另有迟到分页期间工具结果更新的回归测试。Desktop 新按钮补充英文翻译，沿用现有本地化检查。
- 取舍：每页最多 100 条，可用可选 limit 调小。单条记录超过字节预算时仍完整返回一条，保证历史不被静默截断；D-BR1 负责接受超大单行。要硬性限制单条消息体积，需要后续加入内容级分块。选择历史会话时 contextTokens 暂为 0，开始运行后刷新为完整上下文估算。

## D-BR1：Desktop 接受超大协议消息

- 问题与根因：stdout 的 bufio.Scanner 限制一行 64MiB，超限时将整个连接判为致命错误。
- 修复：bufio.Reader + json.Decoder 逐个解码协议对象，一次解码直接分发回复或事件，移除人为行长限制。JSON 损坏、EOF 和进程异常仍使用真实的 fatal 错误通道。
- 测试：`TestStreamAcceptsAnEventLargerThan64MiB` 流式生成超过 64MiB 的事件，旧代码报 token too long，修复后完整接受并交付紧随其后的响应；`TestStreamRejectsMalformedJSON` 保留损坏数据检测。

## B15：阻塞工作与 stdout 离开异步 worker

- 问题与根因：主运行时固定两个 worker；Catalog 扫描、SQLite、附件导入、JSON 编码和 stdout 同步写入会占用它们，取消请求即使没有 Router 锁也无法调度。
- 修复：异步 worker 从 2 提高到 4，避免每个 bridge 按主机全部核数展开过多线程。bridge 非控制请求和 run 在 spawn_blocking 中由 Handle::block_on 驱动，网络等待仍由 Tokio 驱动；取消和审批留在异步快路径。同步 SQLite 与扫描在阻塞线程执行。stdout 使用专用 blocking writer，生产者仅入队，按 FIFO 编码、写入与 flush；EOF 时先取消，再排空请求、收尾会话、排空 writer。没有新增依赖。
- 测试：`concurrency.test.mjs` 用 E4 helper 并发扫描两个含 15MB 文件的插件目录，同时取消另一会话；修复前取消确认约 3795ms，修复后小于 100ms。`slow_stdout_is_queued_and_drained_in_order` 用每次写入延迟 150ms 的 writer 验证入队不阻塞及退出前顺序排空。
- 取舍：阻塞线程的上界沿用 72 个在途请求，并另有 run/writer；阻塞请求启动后不能强制 abort，EOF 等待其完成，客户端已有的关闭超时仍可兜底。writer 队列保持无界以免日志生产者堵住取消；极慢或不读 stdout 的客户端可能积压内存，后续可设计带优先级的背压。

## B18：会话新鲜度比较消息版本

- 问题与根因：ensure 每次反序列化全量 messages 并深比较；Runtime::run 的所有权检查也重复同样工作。
- 修复：新增可重建的 message_revisions 表和消息 INSERT/UPDATE/DELETE 触发器。版本与消息写入同事务提交/回滚；同长度替换、较早消息修改以及旧程序直接写数据库也会递增。旧库缺失版本时从 0 开始，保留 user_version=7，旧程序仍能写入并触发版本更新。Runtime 首次加载在同一 SQLite 读事务中取得消息与版本；之后 ensure/run 只查询一个版本号。自身写入、压缩和卸载后更新版本缓存；失败写入使缓存失效，下一次加载不复用可能已改动的内存副本。删除会话同时删除版本元数据。
- 测试：`ensure_checks_metadata_without_reading_message_bodies` 在加载后把正文列改成读取即报错的测试视图，旧 ensure 报 malformed JSON，新 ensure 不碰正文。`message_revision_detects_same_length_changes_and_rolls_back_atomically` 验证跨连接修改较早消息、相同消息数的替换、写事务回滚。原有 stale in-memory 协议测试验证另一进程写入后能重新加载。

## B-STDERR：TUI 诊断日志与错误分离

- 问题与根因：每个 stderr chunk 都被包装成 bridge_error，包含 Error 的普通日志会变成界面错误。
- 修复：stderr 只追加到 `~/.minicode/logs/agent-stderr.log`，可通过 MINICODE_BRIDGE_LOG 覆盖；关闭连接时排空日志。新增可选协议事件 `log` / `warning`（text 为字符串），capability 为 diagnosticEvents，协议版本仍为 2。log 写文件，warning 显示警告 notice，不改变 running 或 error；协议损坏、进程退出与 run_error 仍报错。无法写日志时仍排空 stderr，不中断 bridge。
- 测试：TUI bridge 子进程同时输出含 Error 和中文的普通 stderr、协议 log/warning，再返回响应；修复前出现 bridge_error，修复后只有 warning 进入界面且日志文件保存完整文本。feed 测试验证 warning 不结束正在运行的任务，log 不改变会话状态。

## D-ERR：Desktop stderr 只记录日志

- 问题与根因：每行 stderr 变成 bridge_error；stderr 的 Scanner 还有 1MiB 行长限制，超长日志会停止排空管道，阻塞 agent。
- 修复：io.Copy 持续把 stderr chunk 写入与子进程 profile 对应的日志（同样支持 MINICODE_BRIDGE_LOG），进程结束后关闭文件；stdout 的 log 也写日志，warning 作为非致命 notice。日志不可写时用 io.Discard 持续排空。诊断不再猜测 Error / Traceback 等字样来决定是否报错。
- 测试：`TestStderrIsDrainedToLogWithoutErrorEvents` 子进程先输出普通日志和一条 2MiB 日志，再发 log/warning 和响应；旧代码超时，修复后日志完整、响应正常、没有诊断错误事件。`TestProtocolWarningIsNonfatal` 验证 Desktop warning 保留 busy 且不填 error。

## 批次 1A 的待确认决策与边界

- 分页默认每类 100 条、2MB 原始正文预算（limit 可调小）；保留超大单条完整内容。旧客户端仍可读最近历史，新客户端使用游标访问更早历史。硬性限制单条内容需要内容级分块，未纳入本批。
- 两端延迟选择 Runtime 是显式可选参数，选择后完整上下文 token 估算延迟到执行请求；不改变旧 selectSession 的行为。
- log 与 warning 是可选协议事件；warning 只提示，运行错误仍通过 run_error / bridge_error。日志路径可配置，日志轮转暂未加入。
- 本批未扩展处理旧文档中已记录的 Windows 后台进程树偶发退出问题。多个完整检查尝试中 lifecycle/runtime 的后台进程用例出现过超时；最终重跑通过，见下方验证记录。另遇到过并发启动桥接进程时 SQLite WAL 初始化 database is locked，属于既有启动路径，未修改。
- 已有的工具状态边界留给下一批：恢复运行中会话时，缺少结果的历史工具被标为 interrupted，后续 tool_call_result 更新未清除该标记（TUI types/feed 与 Desktop model/feed、state 中的现有行为）。
- 建议下一批优先处理 run 生命周期/客户端状态竞态，以及并发压缩使历史游标失效时的刷新策略；随后跟进 Windows 进程树绑定和 stdout 背压。

## 批次 1A 最终验证（Windows / PowerShell 7）

- cargo fmt --check、clippy --locked --all-targets -- -D warnings、test --locked（67 个库测试）、build --locked 均通过。
- MINICODE_AGENT 指向 target/debug/minicode.exe：node --test tests/agent/*.test.mjs 最终 80 通过、1 个 200 秒慢测按 CI 默认跳过。并发 Catalog 扫描时 cancel 确认 0.8ms；大会话分页测试通过。
- generate-tasks --verify、generate-trace 与生成文件 git diff 检查通过。b0/b1/b2、concurrency=3 的两组离线评测通过 verify-results：60 次任务运行、9 次场景运行符合预期。
- npm ci、TUI build、MINICODE_TUI_BRIDGE_TEST=1 npm test 全部通过（104 个测试）。
- Desktop go test -count=1 ./...、go vet ./...、go test -count=1 ./vendor/github.com/egoist/mygo/ui 全部通过；vendor 包当前无测试文件。
- 本地日志保存在 output/checks/：batch-1A-verified-*（Rust）、batch-1A-protocol-complete.log、batch-1A-rest-*（评测）、batch-1A-clients-*（TUI / Go）。保留了失败复现和偶发失败日志，没有修改或跳过这些既有失败用例。
- 没有新增依赖，没有修改生成的评测任务/trace，没有升级发布版本。

## 批次 1A 修改文件

- `agent/bridge.rs`
- `agent/main.rs`
- `agent/runtime.rs`
- `agent/store.rs`
- `agent/tests.rs`
- `desktop/native/feed.go`
- `desktop/native/history_test.go`
- `desktop/native/internal/bridge/client.go`
- `desktop/native/internal/bridge/large_stream_test.go`
- `desktop/native/internal/bridge/stderr_test.go`
- `desktop/native/internal/model/feed.go`
- `desktop/native/internal/model/types.go`
- `desktop/native/localization.go`
- `desktop/native/state.go`
- `desktop/native/stderr_ui_test.go`
- `docs/architecture.md`
- `docs/fixes-3.0.3.md`
- `tests/agent/audit.test.mjs`
- `tests/agent/concurrency.test.mjs`
- `tests/agent/history.test.mjs`
- `tui/src/App.tsx`
- `tui/src/bridge.ts`
- `tui/src/feed.ts`
- `tui/src/types.ts`
- `tui/tests/bridge.test.ts`
- `tui/tests/feed.test.ts`
- `tui/tests/history.test.tsx`

# MiniCode 3.0.3：run 生命周期与多端状态（批次 1B）

## B20：run 事件身份与迟到终止事件

- 核查：原 bridge::start 的终止事件只有 sessionId，Runtime::run 才生成 run_id；两端直接处理所有匹配会话的终止事件，均已用失败测试复现。
- 修复：在 bridge 注册 run 时生成 runId，sendPrompt 响应、run_started、delta、工具/审批及终止事件携带同一可选字段。Hooks 捕获该次 runId，不随下一次快照变化；Runtime 复用 bridge 注册的身份，CLI 每次仍生成新身份。旧 run 的异常收尾只复位属于自己的快照。两端过滤过期 run 的事件及终止副作用，等待新发送身份期间忽略上一轮的终止事件。缺 runId 按旧协议处理，协议版本仍为 2，新增 runIdentity capability。
- 测试：run-state.test.mjs 验证两轮响应/事件/快照身份一致且不重复；TUI feed 验证迟到终止与发送等待窗口；Desktop TestOldRunTerminalDoesNotFinishNewRun 先复现旧事件清空 Busy，再验证新事件结束和旧协议兼容。

## D3：提前受理与发送超时对账

- 核查：原 sendPrompt 等待 Runtime::new/扫描后才响应，Desktop 在 DeadlineExceeded 后直接删除气泡并 Busy=false；Rust 锁占用测试与 Desktop 超时测试均先失败。
- 修复：新增可选 asyncStart 请求参数与 capability，两端显式启用，旧客户端保持同步错误约定。预检后先创建/选定会话、登记 runId/preparing 并返回，后台准备任务复用该身份与取消 token；getState 可在准备锁被占用时读取共享快照。准备异常统一发带 runId 的 run_error，EOF 等待准备与 run 收尾。两端请求超时先 getState 对账，接受/运行中保留气泡和 Busy，确认空闲且没有新 run 才回滚；状态查询也失败时保持待确认。发送回调和状态刷新使用代次拦截迟到响应。
- 测试：accepted_prompt_and_state_do_not_wait_for_preparation_lock 验证持有准备锁时 100ms 内受理和查询；run-state.test.mjs 覆盖后台准备失败和下一次发送；Desktop 覆盖超时已接受/未接受两分支，另有可选真实 35 秒 Deadline 测试（MINICODE_SLOW_TESTS=1）。
- 待确认决策：asyncStart 为显式可选，准备失败通过 run_error 而非同步 RPC error 告知；状态未知时维持忙碌并提示待确认，不自动重发。
- 验证：完整 CI 命令通过；真实 35 秒测试已单独启用并通过（约 36.5 秒），TUI 同样覆盖超时后保留已接受发送。

- 兼容性补测：提前受理的新会话继续继承首条消息前激活的技能；该测试先复现技能缺失，再修复后台准备的新会话标识。

## B19：超限请求关联原 ID

- 核查：服务端超过 4MB 后只保留若干片段，完整 JSON 解析失败而丢失 id。TUI bridge.request 与 Desktop bridge.Client.Request 已有发送前大小检查，本部分沿用，补测试而不重复实现。
- 修复：只保留固定上限的连续前缀，继续排空该请求行；使用 JSON visitor 从前缀提取完整的顶层字符串/整数 id，正确处理转义并跳过嵌套 id，回复带 id 的 size limit 错误。
- 测试：5MB 协议请求修复前未收到关联错误，修复后在 750ms 内拒绝且连接仍可用；Rust 前缀测试覆盖转义、嵌套 id、负整数与不合法 id；两端本地预检测试验证不写入超限请求。
- 边界：id 本身必须在保留的 4MB 前缀内可完整解析；两端都把 id 放在行首。无法识别 id 的损坏请求仍通过全局协议错误报告。

## B21：轻量保留被淘汰会话的设置

- 核查：第七个空闲会话触发 LRU 后删除 Owner，权限模式、effort 和本会话授权随内存运行时丢失；协议回归先复现 bypass 退回 default。
- 修复：淘汰前将轻量 Settings 按 sessionId 留在进程内映射；选择和 Runtime 重载时恢复对应设置，仅跨会话创建时清理继承授权，避免 ensure 再次清空已恢复授权。设置只在成功加载后绑定，失败选择不把另一会话的权限带入当前会话。异步受理路径也执行同一 LRU 清理；删除会话删除其缓存设置。
- 测试：session-cache.test.mjs 创建八个会话，确认首个 Owner 已被淘汰，验证延迟选择与完整 Runtime 重载均保留 bypass/high/write 授权。
- 待确认决策：设置仅在当前 bridge 进程内保留，重启不恢复授权；显式 clearAlwaysAllow 撤销当前会话授权；resetSession、/new 清理新视图的继承授权，其他视图仍保留其原会话设置。没有存储迁移或新增依赖。


## B22：运行中会话支持多视图订阅

- 核查：busy 时不更新 client_key，而 Sink 已捕获旧 key；协议失败测试确认第二个视图收不到当前 run 快照或后续输出。
- 修复：事件 Sink 动态读取会话订阅者，同一 run 的输出、审批和终止事件各向每个视图发送一次。新增订阅立即收到带 runId 和 state 的 run_started 快照；重复查询不重复订阅。视图切换时从旧订阅解绑，/new/resetSession 单独创建该视图的新 Owner，避免重置其他订阅者的会话。原会话继续遵守 B21 的设置保存。历史只读请求不自动订阅无关默认视图。
- 两端：恢复运行中会话时绑定当前 runId 与 busy；历史工具等待实时结果时，结果到达会清除旧 interrupted 标记。
- 测试：subscriptions.test.mjs 验证第二视图即时快照、两个视图均继续收到流/终止，以及 /new 互不干扰；TUI/Go 先复现恢复时的身份/假空闲及旧工具标记，再修复。
- 待确认决策：采用多订阅者而非独占接管；协议只补当前状态，已产生的完整历史仍走 B14 分页，不重复推送历史正文。
- 兼容性验证：resetSession 在分离请求视图时仍使原 Runtime 失效，后续恢复继续重载并重新固定变更后的项目指令（R1 回归通过）。
- 验证：完整 CI 命令最终通过；协议 86 通过、1 个既有慢测默认跳过，TUI 110 通过，Desktop 测试/vet/vendor 与两组评测验证均通过。日志见 output/checks/1B-B22-final-*。

## 批次 1B 暂停点

按用户要求，在完成 B22 后暂停。已完成并分别提交 B20、D3、B19、B21、B22；剩余 TU1、TU2、D1、D2、D4、D5 尚未修复。

TU1、TU2 已有本地失败复现，分别保存在 output/1B-TU1.test.mjs、output/1B-TU2.test.mjs（日志在 output/checks/1B-TU1-before.log、1B-TU2-before.log）；它们是未提交的本地测试草稿，不属于 B22。后续继续时先将相应用例纳入 TUI 正式测试，再实现修复。D1/D2/D4/D5 的当前处理分支已核对，仍需各自补失败测试。

## TU1：新会话拒绝旧会话迟到事件

- 核查：/new 清空 sessionId 后，feed 将首个事件的 sessionId 当作当前会话；Ink 集成测试先复现旧输出重新出现、后续发送携带旧 ID。
- 修复：保留当前 TUI 生命周期内已离开的会话 ID；reducer 与事件副作用共用过滤条件。初始化、发送响应和显式 resume 作为权威绑定；显式恢复会移除对应屏蔽，旧协议无 runId 的迟到事件同样被过滤。
- 测试：run-state.test.tsx 覆盖 /new 后迟到输出与下一次发送参数；feed.test.ts 覆盖新发送等待期间的旧终止事件及显式恢复。用户已要求继续，前述暂停点为历史记录。
- 验证：完整 Rust、协议、两组评测及 TUI 检查通过（1B-TU1-final-*）；首次协议检查的 Windows 子进程超时已保留日志，复跑通过。

## TU2：拒绝发送后只保留草稿

- 核查：send 的异常分支恢复草稿但未移除乐观用户气泡；Ink 回归先复现同一句提示出现两次。
- 修复：每次发送分配唯一气泡 ID，确认拒绝时只移除该气泡并恢复草稿；已接受或状态未确认的超时分支继续遵守 D3，不误删运行中的输入。
- 测试：run-state.test.tsx 覆盖拒绝后只剩草稿、重发成功只有一个气泡；既有 D3 测试同时验证超时已接受时保留气泡且不恢复草稿。
- 验证：本项全部要求检查通过，日志见 output/checks/1B-TU2-*。

## D1：审批决定只更新对应卡片

- 核查：Desktop approval_decision 遍历清空全部待审批卡片；双卡片测试在带显式身份与旧事件顺序两种情况下均先失败。
- 修复：审批卡片保存 CallID，决定按 call_id 匹配。审批 request 新增可选 call_id（包括工作区信任），支持请求交错；旧 bridge 缺少该字段时沿用前置 approval_request 的身份。
- 测试：approval_state_test.go 覆盖请求交错、未知 call、旧协议顺序；approval-identity.test.mjs 先复现缺字段，再验证请求与决定身份一致。
- 兼容性：仅增加可选协议字段，没有存储迁移与新增依赖。
- 验证：全部 CI 命令最终通过（1B-D1-final-*），包括 87 项协议测试、两组评测与 Go 全量测试/vet/vendor。启动锁与后台时序的不稳定失败记录在同项初次/重试日志中。

## D2：恢复补全插入完整会话 ID

- 核查：composer 的 /resume 候选只插入前八位；相同前缀的两个会话测试先复现歧义。
- 修复：候选值直接使用完整 ID，保留标题显示与前缀筛选。
- 测试：TestResumeCompletionKeepsFullIDsWithTheSamePrefix 验证同前缀两个候选均插入各自完整 ID。

## D4：恢复缓存外会话时刷新并报告失败

- 核查：resume 的返回 ID 只在本地 sessions 中查找，缓存缺失时无任何操作；缓存外会话、找不到目标、刷新失败三个回归均先失败。
- 修复：命中缓存直接打开；未命中时请求 listSessions，再打开目标或显示带 ID 的错误，刷新错误也明确展示。列表版本防止旧响应覆盖新缓存；导航代次避免异步恢复抢回用户随后打开的页面。
- 测试：resume_state_test.go 覆盖刷新后成功打开、刷新报错、刷新后仍不存在，以及延迟刷新不能撤销后续 /new 导航。
- 取舍：只在缓存缺失时补查，保持已缓存会话的原有打开延迟；没有新依赖或协议变化。
- 验证：D2 与 D4 的最终代码共同通过全部 CI 命令（1B-D2-retry-*）；分别提交，协议 87 通过、1 个既有慢测默认跳过，TUI 114 通过，Desktop 全量/vet/vendor 与两组评测均通过。TU2 补充验证等待期间新草稿与失败内容共同保留，未引入输入丢失。

## D5：全局 bridge 错误只提示一次

- 核查：全局 bridge_error 遍历全部视图写入 Error，按 Error/exited/invalid 等关键字判断终止；三个视图的失败测试复现三次通知、两条 toast，非致命与会话错误也误伤其他视图。
- 修复：全局错误保存在 desktop 的连接状态，重复报告去重；fatal 明确标记连接失败，只收尾运行中或仍有实时输出的视图，保留队列与原会话错误，不逐视图通知。当前页面显示共享连接错误，后续发送保留草稿；拒绝迟到事件与发送回调，避免假恢复和重复回滚。会话范围内的错误正常路由，仅 fatal 终止对应视图，不再按文本关键词猜测。
- 已修复部分证据：internal/bridge/client.go 的 stderr 持续 io.Copy 到 diagnostics，普通 stderr 不再生成协议错误（批次 1A D-ERR）；本项保持该日志通道，没有重复改动。
- 测试：bridge_errors_test.go 先复现全局重复通知、非致命误终止、会话错误越界、迟到发送回调四种问题；修复后覆盖通知去重、共享状态可见、空闲视图不受影响、排空活动控件、队列/原错误保留、失败连接保留草稿与迟到事件过滤。原收尾/推理回归同步改为断言全局状态及实际客户端发送的 fatal 标记。
- 取舍：fatal 后阻止继续发送，连接状态保持到应用重新建立 bridge；没有新增自动重启或自动重发行为，没有新增依赖。

## 批次 1B 完成记录（恢复执行后）

B20、D3、B19、B21、B22、TU1、TU2、D1、D2、D4、D5 均已修复，各自独立提交；前面的暂停点仅保留为执行记录。没有跳过整个条目或留下本批未实现功能。D5 的 stderr 日志部分沿用批次 1A 已有实现（desktop/native/internal/bridge/client.go:86、:250），普通日志不作为错误事件。

主要改动文件：
- agent/bridge.rs、agent/runtime.rs：run 身份、提前受理、超限请求 ID、设置保留、订阅与审批身份。
- tui/src/App.tsx、bridge.ts、feed.ts、types.ts：run/会话事件过滤、超时对账、发送回滚与恢复运行状态。
- desktop/native/state.go、composer.go、internal/bridge/client.go、internal/model/types.go：状态、会话恢复、审批、完整 ID、请求预检和全局连接错误。
- tests/agent/run-state.test.mjs、request-size.test.mjs、session-cache.test.mjs、subscriptions.test.mjs、approval-identity.test.mjs。
- tui/tests/bridge.test.ts、feed.test.ts、run-state.test.tsx。
- desktop/native/run_state_test.go、send_timeout_test.go、approval_state_test.go、completion_ids_test.go、resume_state_test.go、bridge_errors_test.go、audit_regression_test.go、ui_test.go、internal/bridge/large_stream_test.go。

待确认决策保持保守默认：asyncStart 可选；未知发送结果保持待确认而不自动重发；授权仅在当前进程保留；运行会话支持多订阅者；连接致命错误不自动重启。显式 initialize 握手成功可以清除失败状态，旧握手迟到不能抹掉新的失败。失败发送期间用户继续输入时，失败内容与新草稿一起保留。

批次外风险仍保留：Windows 后台进程收尾/完成通知时序与 SQLite 启动锁竞争偶发导致协议测试失败，本批没有扩展修改这些路径。失败日志与最终通过日志均保存在 output/checks。建议下一批先给这些已知不稳定测试做独立定位，再增加多视图切换、进程重启和长时间会话压力场景。
- 最终验证：1B-D5-final-* 的全部 Rust/协议/生成器/两组评测/TUI 检查通过；协议 87 通过、1 个既有 200 秒慢测默认跳过，TUI 114 通过，60 次任务评测与 9 个场景结果通过。恢复边界补改后，Go 全量测试、vet、vendor UI 检查再次通过（1B-D5-recovery-go-*）。先前真实 35 秒发送超时测试已启用并通过。
