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

## R5：按错误类型判断重试

- 问题与根因：运行时从错误文字中查找 `connection`、`HTTP 5` 等片段，普通错误也可能被误重试；HTTP 408 和未产生输出就断开的流则无法正确分类。
- 修复：provider 以结构化错误区分 HTTP 状态、网络错误、空闲超时、流中断和协议错误，运行时按类型及状态判断；保留 fake 脚本既有 `provider HTTP <status>` 约定。服务端错误详情及 SSE 错误类型分别在 P2、P14 处理。
- 测试：`provider_retry_does_not_use_error_message_keywords` 先复现含 `connection` 的普通错误被错误重试；`provider_error_retry_classification_uses_variants_and_status` 覆盖 408、429、5xx、400/401/403/404、空闲超时和误导性协议错误文本；既有 fake 重试测试覆盖向后兼容。
- 验证：`2A-R5-final-*` 全部通过，协议测试 87 通过、1 个既有慢测跳过，60 个离线任务和场景验收通过，TUI 构建及桥接测试通过。旧评测目录因二进制哈希不同被保存在 `output/checks/2A-preexisting-*`。

## P2：保留 HTTP 错误正文与重试提示

- 问题与根因：非成功响应只报告状态码；`Retry-After` 被丢弃，网络错误的原因链也没有贯通到用户可见信息。
- 修复：最多读取 4 KiB 响应正文并标记截断，读取等待最多五秒；解析秒数或 HTTP 日期格式的 `Retry-After`，将其作为可选重试事件字段保留；识别常见上下文溢出代码及消息，401 提示检查凭据。网络错误保留 source 链并移除请求 URL。按该提示调整等待时间由 R8 处理。
- 测试：协议测试先复现 400 丢失正文及 429 丢失重试提示，并验证 4 KiB 上限、本地连接失败及不可用代理的底层原因；Rust 单测覆盖两种 `Retry-After` 格式、过期日期、无效值及上下文溢出分类。
- 验证：`2A-P2-final3-*` 全部通过；协议测试 91 通过、1 个既有慢测跳过，离线评测结果和 TUI 构建及桥接测试通过。

## P14：分类并透传 SSE 错误

- 问题与根因：Anthropic 和 OpenAI 风格的 SSE 错误被统一替换为 `provider stream error`，服务端类型、消息与重试语义全部丢失；有名称的 `event: error` 也可能被误当作普通事件。
- 修复：解析 SSE 错误对象及有名称的错误事件，保留类型与消息；overloaded、rate limit、api/server error 可重试，invalid request 和未知错误不重试；上下文溢出保留独立类型。已经输出后的重试和撤回由 R8 处理。
- 测试：协议测试先复现 Anthropic overloaded、具名 SSE 错误、OpenAI api_error 及 invalid request 丢失消息；Rust 单测覆盖错误类型、消息及 SSE 上下文溢出分类。
- 验证：`2A-P14-*` 全部通过，协议、离线评测及 TUI 检查均通过。

## R8：退避、截止时间与流式重放

- 问题与根因：最多两次固定退避、不遵守服务端等待提示，且可见输出后断流会直接结束；Desktop 的重试提示也不会移除半截文本和推理。
- 修复：默认最多重试六次，使用基数 1 秒、上限 30 秒的指数 full jitter；优先按 `Retry-After` 等待，默认上限 60 秒。回合截止时间与取消均可中断等待。可用 `MINICODE_PROVIDER_MAX_RETRIES`、`MINICODE_PROVIDER_RETRY_BASE_S`、`MINICODE_PROVIDER_RETRY_MAX_S`、`MINICODE_PROVIDER_RETRY_AFTER_MAX_S` 覆盖（重试次数上限 8，时间上限 60 秒）。断流或可重试 SSE 错误发生在可见输出后时，重试事件携带可选 `reset_output`，Desktop 丢弃当前文本与推理后重放；TUI 已有相应清理行为。补齐 provider 协议错误、回合截止和取消的类型，避免按错误文字猜测截止原因。
- 测试：先复现 529 忽略 5 秒提示、部分流不重放、等待不受截止约束以及 Desktop 保留半截内容；协议测试覆盖断开、流内 overloaded、空闲超时、六次上限、取消及截止；Rust 单测覆盖抖动与等待上限、误导性截止文字；TUI 和 Desktop 分别验证保留历史而清除本次流。
- 取舍：重试次数和等待上限采用保守有界默认值，提供环境变量以适配网关；旧事件仍可处理，未新增依赖。
- 验证：`2A-R8-*` 全部通过；Rust 76 项、协议 100 项通过（1 个既有慢测跳过），两组离线评测、TUI 构建与桥接测试、Windows Desktop 全量测试/vet/vendor UI 检查通过。

## P15：按已报告的响应统计用量可用性

- 问题与根因：失败重试直接将用量标记为不可用，累计与子代理恢复使用逻辑与，任何缺失值都会永久覆盖其他有效响应。
- 修复：空累计从不可用开始，至少一个响应提供有效数字用量时标记可用，失败尝试与空 usage 对象不再污染累计；会话恢复时从历史响应和子代理记录恢复有效标记，并修正旧会话摘要。新会话显式写入初始不可用；旧 JSON 缺少 `available` 字段仍按原默认值读取，无需数据库迁移。
- 测试：先复现失败后成功、混合有无 usage、空对象、零 token 旧会话恢复和旧空会话错误标记；另覆盖子代理旧摘要恢复、旧 JSON、全部缺失和持久化结果。
- 取舍：可用表示至少有一条实际用量，并不表示缺失响应的费用已经补估；本批不新增占比字段。
- 验证：`2A-P15-final3-*` 全部通过，协议 103 通过、1 个既有慢测跳过；首次全量协议测试遇到既有 SQLite 并发启动锁（`2A-P15-final2-protocol.log`），单独复测及完整重跑通过。

## P3：Anthropic 提示缓存断点

- 问题与根因：Anthropic payload 没有缓存断点，服务端无法复用工具、系统指令和对话前缀。
- 修复：默认在最后一个工具、非空 system 和最近两条规范化后的消息末尾标记 `ephemeral`，每次最多四个断点；不改变持久化消息或 OpenAI 兼容 payload。设置 `MINICODE_ANTHROPIC_PROMPT_CACHE=off`（也接受 `0`、`false`、`no`）可为不支持该参数的网关关闭。
- 测试：协议测试先复现缺失断点，再验证跨轮 system/tools 字节相同、断点数量和位置、模拟 cache read/write 用量、关闭开关及 OpenAI payload；Rust 用例验证消息规范化与原始历史不受影响。
- 取舍：默认开启以满足降本目标，兼容网关可关闭；缓存是否命中还取决于服务端支持、最小长度及缓存有效期。首写计费可能高于普通输入，参见 [Anthropic 定价](https://platform.claude.com/docs/en/about-claude/pricing)。会话中动态指令导致的前缀变化由 X7 处理。
- 验证：`2A-P3-final2-*` 全部通过；早期全量协议测试遇到两项既有 Windows 后台进程时序失败（`2A-P3-final-protocol.log`），两项单独复测及完整重跑通过。

## X7：冻结系统前缀并追加动态提醒

- 问题与根因：每轮重新拼接 system，已激活 skill 和触达的嵌套 AGENTS.md 都会改变最前面的提示，破坏已有缓存前缀。
- 修复：第一次模型请求前冻结基础指令、环境、全局及根目录 AGENTS.md 和工具清单；将冻结内容及指令摘要保存到本地内部表，不作为公开事件发送。嵌套作用域、skill 状态及恢复时更新/删除的根规则以 `<system-reminder>` 快照追加；工具结果之后立即同步，仅在内容变化或历史压缩移除快照后追加。后台任务通知、截断与停滞提示也使用提醒；后台命令输出仍作为提醒外的数据。代理名称排序、去重，工具定义顺序稳定。
- 测试：协议测试先复现 system 随 skill 加载而变化，并验证 Anthropic/OpenAI 下加载、卸载、嵌套规则发现、根规则修改/删除后恢复时 system 和 tools 字节不变；Rust 测试覆盖旧会话迁移、中断后找回嵌套规则、压缩后恢复完整 skill、代理顺序、冻结快照不可覆盖且随会话删除，以及系统全文不出现在会话事件中。
- 兼容与取舍：旧数据库自动创建可选内部表；旧会话没有冻结快照时，以升级后首次请求时的基础内容建立前缀，已有动态指令从历史和当前目录重建。冻结的 system 文本保存在本地数据库并随会话删除；根规则变更时旧文本仍在 system，提醒要求采用新版本。实际工具集合被用户修改时仍会改变工具前缀，排序只能保证同一集合的顺序。
- 验证：`2A-X7-final-*` 全部通过；Rust 86 项、协议 107 项通过（1 个既有慢测跳过）、TUI 115 项通过；60 个离线任务和 9 个场景结果通过验收。最终代码另通过 Windows Desktop 全量测试、vet 和 vendor UI 测试。

## P4：推理模型参数兼容

- 问题与根因：所有 OpenAI 风格请求都发送 `max_tokens`，且将内部 effort 原样发送；GPT-5 / o 系列会拒绝部分参数和档位。
- 修复：识别 GPT-5 与 o 系列后发送 `max_completion_tokens`；GPT-5.2 起将 `max` 限制为 `xhigh`，较早的 GPT-5 / o 系列将 `xhigh` 和 `max` 映射为 `high`。其他网关保持原参数。模型配置可选 `parameterCompat.maxCompletionTokens` 与 `parameterCompat.reasoningEffort` 覆盖推断，旧配置无需迁移。
- 测试：`reasoning_models_use_compatible_output_and_effort_parameters` 先复现 gpt-5 的错误参数，覆盖 GPT-5、GPT-5.1、GPT-5.4、o3、o4-mini、未知网关及用户覆盖。
- 取舍：未识别模型维持现有网关行为；可通过配置显式指定兼容参数。参考 [OpenAI 推理文档](https://developers.openai.com/api/docs/guides/reasoning?api-mode=chat)。
- 验证：`2B-P4-final-*` 中的全部 Rust、协议、生成器、两组评测和 TUI 检查通过。

## P5：保存并回放推理块

- 问题与根因：thinking 仅作为流式显示文本，签名和脱敏块被丢弃；工具结果返回后无法向模型回传原始推理。
- 修复：会话消息新增向后兼容的 thinking 和 redacted_thinking 块，流式及 JSON 响应保留顺序、签名、脱敏数据和来源；同一 Anthropic 模型工具循环原样回传，DeepSeek / Kimi / Moonshot / GLM 兼容接口回传 `reasoning_content`，并可通过 `parameterCompat.reasoningContent` 覆盖。Anthropic 请求在开启 effort 时发送受输出上限约束的手动 thinking budget；配置开关由 C4 接通。缓存断点不会写入已签名的 thinking 块。
- 测试：先复现 Anthropic 流式签名和脱敏数据丢失、DeepSeek reasoning_content 丢失及空 content 不兼容；新增工具循环持久化/重放、非流式响应与预算边界测试。
- 取舍：默认 effort 仍为 off；thinking 至少需要 1024 token，输出预算不足时明确报错，不发送无效请求。来源不同的内容交接由 P8 处理；仅用本地 mock 验证，未调用真实服务。
- 验证：`2B-P5-final-*` 全部通过；协议测试 110 通过、1 项既有慢测跳过。首次全量及隔离复测出现既有 Windows 后台进程/通知时序问题，失败记录保留在 `2B-P5-protocol.log`，完整重跑通过。

## C4：允许 Anthropic 模型启用思考

- 问题与根因：保存服务时按接口格式强制清除 supportsEffort，provider 又对 Anthropic 无条件关闭 effort；桌面设置也禁用了开关。
- 修复：保留用户为模型声明的思考能力，允许在桌面设置中启用；Claude 4.6 及已知较新模型使用 adaptive 模式，旧模型使用 P5 的手动预算；兼容网关可通过 `parameterCompat.thinkingMode` 覆盖。未知未配置 Anthropic 模型仍默认关闭。
- 测试：先复现配置被改写、桌面复选框不可用以及新模型错误使用手动 thinking；桥接测试验证启用 high 后带签名的 thinking 在工具循环回放，Rust 测试验证 adaptive 参数，Go UI 测试验证开关可操作。
- 取舍：adaptive 的 xhigh/max 暂时映射到各已识别模型普遍支持的 high；effort=off 保持不额外发送 thinking 参数，服务端本身的默认行为可能仍会启用 thinking。
- 验证：`2B-C4-*` 全部通过；协议 111 通过、1 个既有慢测跳过；Windows Desktop 全量测试、vet 与 vendor UI 检查通过。

## P8：跨 provider 的工具 ID 与推理交接

- 问题与根因：历史工具 ID 原样传给 Anthropic，带点号、冒号或过长的 ID 会被拒绝；不同来源的推理签名不能安全地当作目标模型的原始推理回放。
- 修复：仅在发送 Anthropic payload 时，为不合法 ID 生成稳定的 SHA-256 派生标识，并避开历史中的合法 ID；tool_use 与 tool_result 使用同一映射，持久化历史不变。来源不兼容的可读推理转为普通文本，脱敏块只发送不可用提示，不传原始签名或密文。
- 测试：先复现 OpenAI → Anthropic 会话切换时报出的非法 ID；协议测试验证实际工具循环和历史不变，Rust 测试覆盖超长、标点、保留 ID 碰撞、重复 payload 稳定性及反向交接。
- 取舍：同一配置 ID 和模型视为同一推理来源；如果用户在原配置中更换账号，部分模型的签名仍可能不兼容，需要服务端反馈后处理。
- 验证：`2B-P8-*` 全部通过；协议 112 通过、1 个既有慢测跳过，两组离线评测和 TUI 检查通过。

## X8：发现模型时使用真实元数据

- 问题与根因：模型发现把上下文、输出与思考能力全部写死为 200k / 8k / false；桌面手动添加也假定 200k，编辑服务时会丢失兼容参数。
- 修复：优先读取服务端的 context_window/context_length、max_output_tokens、输入模态与 effort 支持档位，其次查询内置目录，未知模型回退 128k / 8k；输出上限小于上下文窗口。发现的档位生成兼容映射，已保存模型的数值仍由用户覆盖；桌面序列化保留可选 parameterCompat，手动未知模型使用 128k。
- 测试：先复现发现结果硬编码、未知 provider 和桌面默认窗口错误，以及桌面序列化丢失参数；桥接测试验证服务端优先、目录回退、未知模型、实际请求中的用户覆盖与 effort 映射，Go 测试验证默认值和数据保留。
- 取舍：没有可靠元数据时使用保守值；服务端可能返回过时或错误的限制，用户仍可按实际网关能力调整。
- 验证：`2B-X8-*` 全部通过；协议 114 通过、1 个既有慢测跳过；两组离线评测、TUI 与 Windows Desktop 测试/vet/vendor UI 检查通过。

## B17：连接测试不依赖模型列表

- 问题与根因：testServiceConnection 与 fetchServiceModels 共用 GET /models，缺少该接口的网关无法测试；HTTP 错误只显示状态码。
- 修复：连接测试向所选模型发送最小非流式聊天请求，Anthropic 使用 /messages、OpenAI 兼容接口使用 /chat/completions，并复用推理模型输出参数规则；模型发现仍使用 /models。错误正文最多保留 4 KiB 并屏蔽密钥，网络错误保留原因。桌面提供独立的测试和获取列表按钮。
- 测试：先复现两类无 /models 网关失败和桌面缺少独立入口；协议测试覆盖两个接口、模型参数、缺少模型、错误正文截断和密钥屏蔽；Go UI 测试验证两个按钮分别执行测试和发现。
- 取舍：聊天探测需要输入或选择模型 ID，并会产生一次很小的实际模型请求及可能的费用；未选模型时给出明确提示。
- 验证：`2B-B17-*` 全部通过；协议 116 通过、1 个既有慢测跳过；离线评测、TUI 和 Windows Desktop 测试/vet/vendor UI 检查通过。

## A5：更新内置模型目录

- 问题与根因：目录只有三个模型，Claude Sonnet 4.5 错误标为不支持思考，常见新模型及带日期的别名无法获得正确元数据。
- 修复：依据官方资料更新 Claude 4.5 / 4.6 / 5.5、GPT-5 / 5.4 / 5.6、DeepSeek Flash / V4 Pro 的窗口、输出、模态和思考信息；识别官方前缀和日期快照别名。GPT-5.6 的 max 档位与 DeepSeek 的有限档位使用目录映射，已保存的用户模型字段仍优先。
- 测试：先复现 Sonnet 4.5 能力错误和新模型回退为未知值；桥接测试覆盖新旧模型、日期别名及内置 Claude 思考请求，Rust 测试覆盖推理强度与工具的 API 限制和网关覆盖。
- 取舍与限制：根据 [OpenAI 文档](https://developers.openai.com/api/docs/guides/migrate-to-responses?lang=javascript&tool-use=chat-completions&update-item-definitions=chat-completions&update-multiturn=responses)，GPT-5.4 及后续模型的 Chat Completions 不支持同时启用工具和非 none 推理；本实现使用工具时默认发送 none，选择更高强度时在发送前提示需要 Responses API，支持该组合的兼容网关可设置 `parameterCompat.reasoningWithTools=true`。完整 Responses API 支持仍待后续批次。目录参考 [Claude 模型](https://platform.claude.com/docs/en/models/overview)、[OpenAI 模型](https://developers.openai.com/api/docs/models/gpt-5.6-sol)、[DeepSeek 模型](https://api-docs.deepseek.com/api/list-models/)；没有后台自动更新，用户可通过服务配置覆盖。
- 补充：已知只支持 Responses API 的 GPT/o Pro 模型，在普通请求和连接探测前明确报错；确实提供 Chat 兼容接口的网关可设置 `parameterCompat.chatSupported=true`。对应 Rust 和协议测试先复现错误发送，再验证拒绝及覆盖。
- 验证：`2B-A5-final-*` 全部通过；Rust 93 项、协议 118 项通过（1 个既有慢测跳过），60 次离线任务和 9 个场景通过验收，TUI 115 项及 Windows Desktop 测试/vet/vendor UI 检查通过。

## P6：非法工具参数返回可恢复结果

- 问题与根因：流式或非流式工具参数不是合法 JSON 对象时，provider 直接终止整个回合，模型无法修正。
- 修复：工具调用新增可选 `input_error`，保存原始非法参数与错误；执行前返回失败 tool_result，并在恢复未完成调用时同样先识别参数错误，不执行工具。回传 payload 使用空对象占位，错误详情通过 tool_result 提供；旧会话缺少新字段仍可读取。
- 测试：先复现流式和非流式非法 JSON、非对象参数直接失败；协议测试验证失败结果、后续模型恢复和文件未写入，Rust 测试验证中断后恢复不会误执行写入。
- 取舍：不自动猜测修复 JSON，避免误执行；占位参数可能与不规范网关最初返回的原始参数不同。
- 验证：`2C-P6-final-*` 全部通过，含空字符串参数边界；Rust、协议、生成器、两组离线评测及 TUI 检查均通过。

## P7：归一化空或重复的工具 ID

- 问题与根因：provider 返回缺失、空或重复 ID 时解析或校验直接失败；部分网关会复用计数器。
- 修复：解析边界接受缺失 ID；校验前对空、超长或重复 ID 合成唯一 ID，保留可用原 ID，并将历史和归档 ID 纳入冲突集合。调用、结果及持久化日志统一使用新 ID；事件中记录归一化数量。
- 测试：先复现流式缺失和重复 ID、非流式缺失 ID；协议测试覆盖同轮与跨轮冲突、超长 ID 和结果配对，原归档压缩测试改为验证旧 ID 保留且新调用使用不同 ID。
- 取舍：复用 ID 的新响应按新调用处理，仍受现有权限控制；复用 ID 不再视为阻止副作用的信号。带签名 thinking 的异常 Anthropic 网关若返回重复 ID，改写 ID 可能使该服务拒绝签名回放。
- 验证：`2C-P7-final-*` 全部通过；首次全量测试发现旧契约使用重复 ID 触发拒绝，已改用空工具名保留拒绝/用量/恢复覆盖并通过重跑。

## P9：合并缺少索引的流式工具分片

- 问题与根因：缺少 `index` 的 OpenAI 兼容流式片段以当前调用数量作为新索引，导致同一调用的参数被拆成多个调用。
- 修复：优先使用显式索引；缺索引时根据已知 ID 找回调用，出现新 ID 时建立调用，无 ID 的后续片段追加到最近调用，支持先收到名称或参数、后收到 ID。
- 测试：先复现缺索引的交错工具调用失败；协议测试以五种字节分片大小验证完整工具循环，Rust 测试遍历所有 SSE 字节切分点并覆盖多字节字符、延迟 ID 和交错调用。
- 取舍：没有索引、ID 和其他区分信息的相邻片段无法无歧义地区分新调用，保守地归入最近调用；相同 ID 的无索引片段按同一调用处理。
- 验证：`2C-P9-*` 全部通过，包含 Rust、协议、生成器、两组离线评测和 TUI 检查。

## P11：允许正常结束的空助手回复

- 问题与根因：provider 校验无条件拒绝空内容，即使模型在工具结果之后以正常结束原因返回空回复；Anthropic 非流式空文本块也会形成空历史消息。
- 修复：接受以 `end_turn` 或 `stop_sequence` 正常结束的空回复，仍拒绝没有调用内容的 `tool_use`；记录完成事件和用量，但不把空助手消息写入历史，并忽略 Anthropic JSON 的空文本块。
- 测试：先复现工具结果后空回复被拒绝；协议测试覆盖两类 provider 的流式/非流式、无内容/空文本、用量保留和同一会话下一轮历史，Rust 测试覆盖空工具调用仍被拒绝。
- 取舍：空回复按正常完成处理，不额外重试或伪造文本，避免重复请求及额外费用。
- 验证：`2C-P11-final2-*` 全部通过；首次全量及单独复测遇到既有 Windows 后台进程清理测试偶发超时，再次单独复测和完整重跑通过。

## P13：接受对象形式的工具参数

- 问题与根因：OpenAI 非流式解析只接受字符串参数；流式解析忽略对象，可能用空参数执行调用。
- 修复：流式和非流式都接受 JSON 对象及 JSON 字符串；非对象值和不同对象/非空字符串片段混用时产生失败 tool_result，不猜测合并或执行；重复的相同对象可接受。
- 测试：先复现对象参数丢弃、非流式报错和混合片段可能误执行；协议测试覆盖对象、非对象、混合顺序、不同对象和重复对象。Rust 属性测试遍历 SSE 的所有字节切分，联合覆盖无索引、空及重复 ID、对象参数、非法 JSON 和多字节字符；P6/P7/P9 回归同时通过。
- 取舍：不同对象或对象与非空字符串混用按不明确输入拒绝执行，要求模型重新发出完整调用。
- 验证：`2C-P13-final-*` 全部通过；首次协议全量遇到既有 Windows 后台进程清理测试偶发超时，该测试单独复测及整套重跑通过。

## R17：明确处理截断的工具调用

- 问题与根因：输出达到 token 上限时，provider 丢弃工具调用后伪造助手文本，runtime 又要求续写，容易让模型继续输出不完整的工具 JSON。
- 修复：只保留模型实际生成的文本和推理，截断调用不进入历史或执行；从流式和非流式响应记录安全且有长度上限的工具名称，在下一轮通过提醒说明调用因截断未执行并建议拆分 write / edit。无工具的截断使用单独提示；允许空的 max_tokens 响应保留用量和结束事件。
- 测试：先复现 OpenAI / Anthropic 流式与非流式、有无正文共八种场景；验证未执行写入、不伪造历史、明确提醒、保留真实文本；另覆盖纯文本截断、工具名清理、三次截断停止和 Anthropic 角色顺序。
- 取舍：不自动提高输出 token 上限，避免增加未配置费用；沿用最多三次截断的现有上限。异常网关未提供合法工具名时提示“未知工具”。
- 验证：`2C-R17-final2-*` 全部通过；早期全量分别遇到既有 SQLite 并发启动锁和 Windows 后台清理偶发超时，对应用例单独复测及完整重跑通过。

## ATT：发送前缩放和压缩图片

- 问题与根因：附件只检查原始大小和文件头，发送时原样 Base64 编码；20 MB 图片超过部分服务的单图限制，每轮重发也增加成本。
- 修复：保留并校验原始附件；发送前按需解码、等比例缩放和压缩，默认长边不超过 2000 像素、Base64 不超过 4,900,000 字节，可分别用 `MINICODE_IMAGE_MAX_EDGE`（32–8000）和 `MINICODE_IMAGE_MAX_BASE64_BYTES`（1024–10000000）覆盖。小图保持原字节；JPEG 转换时应用 EXIF 方向，其他格式转换为保留透明度的 PNG。对图片尺寸、像素数及解码分配设置上限；含图片的请求准备在后台线程执行，取消和回合截止不必等待压缩。
- 测试：先复现实际 20 MB PNG 的请求超限；协议测试覆盖 Anthropic 两轮图片字节一致、缓存断点和 cache_read 用量、原文件不变、OpenAI 配置覆盖、大图准备时取消。Rust 测试覆盖 PNG/JPEG/GIF/WebP、小图原样通过、透明度、方向、体积上限、不放大、损坏文件及解码前像素限制。
- 依赖：新增 `image 0.25.10`，只启用现有四种图片格式；跨平台安全解码、缩放与编码需要相应编解码实现，锁文件包含其传递依赖，最低 Rust 版本与项目的 1.88 一致。
- 取舍与限制：Anthropic 提供的缓存断点可减少重复前缀计费，但历史图片仍随请求传输，是否命中由服务端决定；[Anthropic 图片限制](https://platform.claude.com/docs/en/build-with-claude/vision)因平台而异，默认按较低的 5 MB Base64 限额留出余量。超过限制而需要转换的 GIF/WebP 使用首帧并转为 PNG；转换会移除原始元数据，JPEG 使用质量 85。旧附件曾仅校验文件头，较小但不可解码的文件保留原有透传行为；过大且不可解码时返回错误。取消后后台压缩任务会自行结束，但不会再发送该请求。
- 补充限制：多张历史图片仍可能使整个请求超过服务端总请求大小限制；目前未引入跨轮图片文件引用或本地转换结果缓存。
- 验证：`2C-ATT-*` 全部通过，包括 Rust、协议、生成器、两组离线评测和 TUI 检查。

# MiniCode 3.0.3：MCP 启动与 Windows Shell（批次 3A）

## MC-WIN：Windows MCP 命令解析

- 问题与根因：以 `npx` 等无扩展名配置启动 MCP 时，Windows 的进程查找只补 `.exe`，找不到 `.cmd` / `.bat` shim。
- 修复：按服务器子进程的 PATH 和 PATHEXT 顺序查找可执行文件及批处理脚本；绝对或相对路径同样支持省略扩展名。批处理交由 Rust 的 Windows 进程启动器转义参数并通过禁用 AutoRun 的 `cmd` 启动，保留现有配置格式。
- 测试：Windows 协议测试先复现带空格目录下的 `.cmd` 找不到，以及同目录存在无扩展名 Unix 脚本时误选脚本；修复后检查发现工具以及空格、中文、引号、`&`、`%`、`!` 等参数原样到达服务进程。
- 验证：真实 `npx -y @modelcontextprotocol/server-filesystem .` 成功发现 14 个工具；Rust、协议、离线评测和 TUI 检查通过，评测使用独立临时输出目录以保留已有结果。

## MC1：MCP stderr 与连接诊断

- 问题与根因：MCP 子进程的 stderr 直接丢弃，本地服务启动失败只显示通用错误，也无法检查已连接服务的日志。
- 修复：异步持续读取 stderr，保留最多 200 行、60,000 字节的内存尾部，同时写入用户目录下的独立日志，单个日志超过 1 MB 时保留最近内容。失败状态追加尾部；发现结果增加可选的 `stderr_tail` 和 `log_path`，旧客户端可忽略。关闭服务后排空读取任务；日志不可写不影响 MCP。常见凭据名称对应的环境变量值在写入日志和状态前遮盖，覆盖跨读取边界的值。
- 界面：Desktop MCP 连接状态页可查看最近日志和文件路径；TUI 新增 `/mcp`，手动测试服务并显示状态、最近 20 行和日志路径。
- 测试：协议测试先复现失败信息缺少 stderr；覆盖连接失败、成功、2 MB stderr 不阻塞、日志大小和尾部、凭据遮盖；Rust 测试覆盖跨块及前缀重叠的凭据遮盖；Desktop 测试验证新旧状态结构；TUI 测试验证 `/mcp` 结果显示。
- 取舍：每次连接产生一个限长日志，目前没有跨文件的自动清理；遮盖只识别常见凭据环境变量名且值至少 4 字节，服务可能输出其他敏感内容，日志目录应由用户控制访问权限。
- 验证：Rust、协议、两组离线评测、TUI 和 Windows Desktop 全套检查通过；评测使用独立临时输出目录。

## SH1：Windows Shell 类型和退出码

- 问题与根因：Windows 的 `bash` 实际调用 PowerShell；PowerShell 默认把失败的原生命令退出码归一为 1，Windows PowerShell 5.1 也不支持 bash 的 `&&` 语法。
- 修复：从实际子进程 PATH 中的 Git 安装查找 Git Bash，找到时 `bash` 使用真正的 bash；否则 `bash` 继续作为 PowerShell 兼容别名。Windows 新增 `powershell` 工具，保留同样的审批、后台任务、cwd 指令预检和子助手工具限制。PowerShell 保留成功命令、cmdlet 错误、显式 `exit` 和失败原生命令的退出状态；命令原文不做语法翻译。已有 Windows 验收配置继续使用 PowerShell 执行。TUI 与 Desktop 使用命令审批和终端输出界面显示新工具。
- 测试：先复现 bash 语法和退出码错误及未知 `powershell`；协议测试覆盖 Git Bash、缺少 Git Bash 时回退、PowerShell 兼容别名、后台任务与嵌套指令；Rust 在本机分别运行 Windows PowerShell 5.1 与 PowerShell 7，检查原生命令、cmdlet 失败和显式退出；TUI / Desktop 测试覆盖命令展示。
- 待确认决策：已安装 Git Bash 的 Windows 默认让 `bash` 使用真正的 Bash，会改变旧会话中同名工具原来的 PowerShell 语义；设置 `MINICODE_WINDOWS_SHELL=powershell` 可以恢复旧行为，并继续通过 `powershell` 明确选择 PowerShell。
- 验证：Rust、协议、两组离线评测、TUI 和 Windows Desktop 全套检查通过；评测使用独立临时输出目录。

## A1：提示词与实际 Shell 一致

- 问题与根因：旧 Windows 提示词只笼统写 PowerShell，不区分 Git Bash、PowerShell 5.1 和 7，也没有说明 5.1 不支持 `&&`；旧会话还会保留冻结的历史提示词。
- 修复：根据实际启动的可执行文件写明 `bash` 和 `powershell` 的类型与版本，并说明对应的链式语法。恢复旧会话时保留原冻结提示，通过动态 system reminder 更新命令环境；环境未变化时不重复追加。PowerShell 7 从可执行文件版本信息读取版本；必要时执行有两秒上限的版本探测，并避开 Windows 应用执行别名。
- 测试：协议测试先复现缺少版本和恢复时缺少更新；覆盖真实 PowerShell 5.1、PowerShell 7 / Git Bash、旧冻结提示恢复和重复恢复不产生重复提醒。
- 验证：Rust、协议、两组离线评测和 TUI 全套检查通过；评测使用独立临时输出目录。

## SH2：中文 Windows 命令输出解码

- 问题与根因：命令输出和截断后的完整归档始终按 UTF-8 解码，Windows 原生程序的 GBK / CP936 等本地代码页字节会变成替换字符。
- 修复：合法 UTF-8 继续原样处理；Windows 上遇到非法 UTF-8 时，依次参考系统 OEM / ANSI 代码页转换。归档先流式判断编码，再按代码页流式转换，保留跨块的双字节字符。PowerShell 的控制台输出使用无 BOM UTF-8；未显式设置时为子进程提供 `PYTHONUTF8=1` 和 `PYTHONIOENCODING=utf-8`，保留用户已有的非空设置。
- 测试：Rust 单测先复现 CP936 乱码，覆盖 CP936、UTF-8、代码页回退和 64 KiB 块边界；协议测试覆盖 Bash / PowerShell 的 Python 默认值、显式覆盖，以及 PowerShell 无 BOM 输出。
- 依赖与取舍：复用已有 `windows-sys` 并启用 Windows 国际化 API，未引入新依赖。不同代码页的字节可能同时合法，无法可靠推断时优先 OEM；单次输出混合多种编码仍可能存在歧义。
- 验证：Rust、协议、两组离线评测和 TUI 全套检查通过；评测使用独立临时输出目录。
## 批次 3B：命令输出与进程治理

### U4：保留 Windows 元数据并同步原子替换

- 核查证据：原实现已有 PermissionDenied 的有限重试，并非完全不重试；但 persist/rename 会丢失 Windows DACL、命名流和属性，Unix 未同步父目录。
- 修复：Windows 关闭临时文件句柄后使用 ReplaceFileW（不忽略 ACL 合并错误），复制文件标志并保留命名流/创建时间/DACL；新文件用不覆盖目标的 MoveFileExW，处理创建竞态。对 access denied、sharing/lock violation 等瞬态冲突有限退避重试，最长约 1.05 秒。ReplaceFile 的部分失败保留备份并尝试恢复；无法恢复时明确返回原文件备份路径。Unix 在已同步临时文件并 rename 后 fsync 父目录，保留原权限。
- 测试：先复现命名流丢失和硬链接被悄悄断开，确认已有共享锁重试测试能通过；修复后验证命名流、创建时间、隐藏属性、受保护 DACL、共享锁释放重试、只读失败不改原文件且不漏临时文件、多硬链接拒绝。Unix 覆盖权限保留并通过 syscall trace 核对文件与父目录均 fsync。
- 待确认决策：ReplaceFile 的最终文件 ID 来自替换文件，不能保证其他硬链接一起更新，因此默认拒绝多硬链接文件的原子写入，保留原文件和链接；不自动退回非原子覆盖。参考 [Microsoft ReplaceFileW 文档](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)。不增加 crate 依赖。
- 验证：`output/check-3b.ps1 U4` 全套检查，日志 output/3B-U4-*.log；额外 Unix 验证见 output/3B-linux-final.log、output/3B-dir-fsync.log。

### 批次验证补充与范围外发现

- 每项的全套检查使用独立输出目录 output/3B-ID/eval 与 scenarios，避免覆盖已有 output/eval 中不同配置的结果；生成的任务与 trace.log 无变更。60 次主任务与 9 个场景保持既有验收结果（场景的 counter_retry b0/b1 为预期失败）。Desktop 源码未修改，不触发 Go 检查条件。
- Windows 检查均按 CI 命令执行；额外 Linux 全量 Rust 与本批协议回归验证也执行。WSL 全协议重跑仅剩 MCP 取消进程存活判定失败，已在 c21578d 隔离基线全协议中复现；streaming Retry-After 最初的耗时阈值失败重跑通过。记录为本批范围外，未改实现。SH7 回归自身的数据库锁竞争通过 busy_timeout 与显式测试同步修正。
- 最终 U4 检查全部通过：Windows Rust 121 项，协议 193 项通过、1 项既有慢测跳过，TUI 117 项；60 次主任务与 9 个场景通过结果校验。额外 Linux Rust 116 项、本批协议 11 项通过，strace 确认两次原子写入分别同步临时文件与父目录。Linux 全协议不宣称全部通过。
- 另以本地 HTTP 401 Git 远端验证：禁用交互提示后 ls-remote 在 3 秒内失败并报告 terminal prompts disabled，没有访问真实远端或提交数据。
- 修改文件：agent/tools.rs、agent/util.rs、agent/command_process.rs、agent/models.rs、agent/runtime.rs、agent/store.rs、agent/configuration.rs、agent/extensions.rs、agent/mcp.rs、agent/assets/tools.json、agent/tests.rs；协议回归 command-output.test.mjs、artifact-retention.test.mjs、tool-timeouts.test.mjs（均在 tests/agent/）；以及本记录。

### U3：终端文本完整剥离 ANSI 序列

- 问题与根因：terminal_text 只滤掉 ESC 等控制字符，留下可见的 `[31m`、OSC URL 等片段。
- 修复：复用 SH3 的终端状态解析，CLI 显示/文本报告同步清理 CSI、OSC、DCS、C1 控制序列、未完成序列和 CR 进度覆盖，不改原始持久消息。
- 测试：先复现 `[31m` 片段残留；修复后覆盖中文 SGR、清屏/光标、OSC 8 链接、C1、DCS、未完成 CSI、CRLF 与制表符。
- Linux 全量检查还发现 SH3 单测错误假定宿主 TERM 未设置；已改为验证显式环境优先，默认值仍由独立 bridge 环境测试覆盖。
- 验证：`output/check-3b.ps1 U3` 全套检查，日志 output/3B-U3-*.log。

### R27：按工具与 MCP 服务配置超时

- 问题与根因：runtime 将所有非 shell 调用统一包在 300 秒限制中；插件 MCP 的 timeout_s 上限 300 秒，本地导入的 MCP 则忽略 timeout_s 并固定 30 秒。
- 修复：desktop-config.json 新增可选 `toolTimeouts` 对象，键为完整工具名，值为秒数（>0 且 <=3600）；未配置普通工具仍为 300 秒。插件/本地 MCP 都读取 `timeout_s`，默认仍 30 秒，上限放宽至 3600 秒，runtime 同步使用服务限制。保留回合 deadline，shell 继续使用原有 timeout_s。工具限制与 MCP 服务内部请求限制同时生效，取较早到期者；工具覆盖不会绕过服务限制。
- 配置示例：`"toolTimeouts":{"grep":60,"mcp__service__slow":120}`；MCP 服务定义中使用 `"timeout_s":600`。不增加配置 UI，既有配置与会话不变。
- 测试：先复现本地 MCP 服务限制和工具覆盖均被忽略、非法配置被接受；修复后以真实延迟 stdio MCP 覆盖服务超时、工具更短超时、正常完成和 >300 秒服务配置；Rust 覆盖非法范围、普通工具配置读取与缺省。
- 验证：`output/check-3b.ps1 R27` 全套检查，日志 output/3B-R27-*.log。

### SH7：输出归档保留策略

- 问题与根因：command_output/tool_output 只在删会话时删除，没有跨会话容量或过期清理。
- 修复：数据库打开和输出归档写入后执行清理；默认输出归档总量 1GiB、保留 30 天，按创建时间过期、按最近访问顺序淘汰。`MINICODE_ARTIFACT_MAX_BYTES` 和 `MINICODE_ARTIFACT_MAX_AGE_DAYS` 可独立设置，0 关闭相应限制；非法值使用默认值。readArtifact/read_artifact 更新访问时间。新增独立 artifact_access 表，旧 schema 7 数据无需迁移，删会话同步清理。
- 测试：真实 bridge 先复现超限仍保留三个归档；修复后验证跨写入容量淘汰、访问后保留最旧但刚读过的条目、按天过期、关闭策略。Rust 验证旧数据库自动补表、会话可继续读取、压缩历史与未登记文件保留。
- 待确认决策：仅清理可丢弃的命令/工具输出，history 压缩历史不计入此容量上限，避免破坏旧会话。当前刚写入的单个归档即使大于配置上限仍保留；被 Windows 锁定的归档留待下次清理，因此容量是软上限。已过期输出的历史引用会报告不可用；不扫描删除未知文件。
- 验证：`output/check-3b.ps1 SH7` 全套检查，日志 output/3B-SH7-*.log。

### SH8：命令预览同时保留头尾

- 问题与根因：捕获只读取末尾 256KB，预览再保留尾部 2000 行/50KB，编译器首条错误会丢失。
- 修复：读取捕获头部，长输出另读尾部；预览在 2000 行/50KB 内双端保留，中间明确说明省略并引用完整 artifact。UTF-8 截取遵守字符边界，内存读取仍有界；普通末尾换行不产生归档。
- 测试：先复现 50,002 行中文输出缺少首条错误；修复后验证首错误、末行、字节/行数预算及完整 artifact 的所有行。
- 边界补验：11 种中文样本偏移中 4 种会在 Windows 上因 UTF-8 窗口被截断而误选代码页。扩展 Rust 回归先复现 padding=4 失败；修复为以完整捕获确定 UTF-8 编码，样本仅剔除首尾未完成字符，避免误解码整个头尾。该补修再执行 `output/check-3b.ps1 SH8-final` 全套检查。
- 验证：`output/check-3b.ps1 SH8` 全套检查，日志 output/3B-SH8-*.log。

### SH6：取消后保留部分输出与终止信号

- 问题与根因：命令清理后直接 bail，runtime 也在结果落盘前检查取消；两层均会丢弃已有输出。信号退出的 code=None 被显示为 -1。
- 修复：取消返回失败 Outcome，保留输出与捕获状态；runtime 先 checkpoint 工具结果再结束取消回合。可选 `signal` 字段记录 Unix 信号名称，错误文本区分信号退出、普通退出与无状态退出，旧记录缺字段仍可读取。
- 测试：Rust 先复现 `tool cancelled` 丢输出；bridge 先复现取消后不存在 tool_call_result，修复后断言事件和历史均包含取消前内容；Unix 覆盖 SIGTERM 名称和部分输出；既有取消释放工作目录测试更新为验证失败 Outcome。
- 验证：`output/check-3b.ps1 SH6` 全套检查，日志 output/3B-SH6-*.log。

### SH5：按输出管道 EOF 判断后代占用

- 问题与根因：捕获已改为临时文件，但 shell 退出后仍用整个进程组/Job 是否存活判断输出占用，并无条件关闭所属进程树。
- 修复：stdout/stderr 通过有界块实时排入磁盘，EOF 独立判断输出是否关闭。正常退出且两个输出管道已关闭时释放进程树所有权；持有输出超过 2 秒、取消、超时或配额超限时继续终止进程树。future 被丢弃时仍自动清理 reader 和进程树。
- 测试：先复现后修复，真实 Node 后代覆盖已重定向输出的存活、继承输出的 2 秒清理及无泄漏。Windows fixture 使用 detached 避免 Node 自身在父进程退出时清理子进程；Unix 保持同一进程组。
- 取舍：已明确重定向输出的后代可独立存活；需由 MiniCode 管理生命周期的后台工作仍应使用 `background:true`（现有 run_in_background 接口语义）。stdout/stderr 的并发顺序不作跨流保证。
- 验证：`output/check-3b.ps1 SH5` 全套检查，日志 output/3B-SH5-*.log。

### SH4：避免命令交互提示

- 问题与根因：stdin 已关闭，但 Git 凭据、编辑器、分页器仍可通过终端或外部程序等待用户。
- 修复：补齐 GIT_TERMINAL_PROMPT=0、GIT_EDITOR=true、GIT_SEQUENCE_EDITOR=true、PAGER=cat、GIT_PAGER=cat、CI=1 默认值；保留显式环境配置。调用者仍可在命令中覆盖。
- 测试：真实 bridge 回归先失败（输出 `,,,,,`），修复后验证六个默认值；已有 stdin=null 与 PowerShell NonInteractive 保留。没有使用真实远程 git push，避免网络凭据和远程副作用。
- 验证：`output/check-3b.ps1 SH4` 全套检查，日志 output/3B-SH4-*.log。

### SH3：清理命令终端序列与进度行

- 问题与根因：`decode()` 只匹配 SGR，其他 CSI、OSC 链接和 CR 覆盖行进入模型上下文。
- 修复：使用无依赖的状态解析剥离 CSI、OSC、DCS 等终端序列（含未完成序列），折叠 CR 进度行并保留 CRLF。shell 默认设置 NO_COLOR=1、TERM=dumb、CARGO_TERM_COLOR=never；显式环境值优先。
- 测试：先运行 `cargo test --locked sh3_`，两项失败，分别复现残留控制序列与缺少颜色环境默认值；修复后验证 CSI/OSC、CRLF、进度覆盖、未完成序列，以及真实 bridge 的默认值/显式覆盖。原始 artifact 保留原始终端记录，预览经清理。
- 验证：提交前执行 `output/check-3b.ps1 SH3` 全套 Rust、协议、离线评测与 TUI 检查，日志位于 output/3B-SH3-*.log。

## 批次 3C：文件与搜索工具

### T1：文本编码、PDF 与 Notebook 读取

- 核查：read 的图片分支原已支持 PNG/JPEG/GIF/WebP 和 image block（原 tools.rs:399–423），该子项不重做，新增实际 PNG 回归。文本仍只接受 UTF-8、保留 BOM；PDF 会作为原始文本/二进制处理，ipynb 只显示 JSON。
- 修复：BOM 优先、UTF-16 零字节分布识别、严格 UTF-8 与 GBK/GB18030 回退；剥离展示 BOM，edit/write 保留原编码、字节序和 BOM，无法编码时拒绝并保留原文件。编码探测读取不超过可配置解码上限的前缀；UTF-8 大文件仍走原有有界分页与大文件编辑路径。PDF 抽取文本层，无文本层明确提示，不做 OCR；ipynb 渲染单元格、代码及纯文本输出，不展开 HTML/图片数据。分页游标保持 opaque，编码文本/渲染文档按解码后的 UTF-8 字节定位。
- 依赖理由：新增 encoding_rs（跨平台 GB18030 严格编解码）及 pdf-extract（PDF 字体/CMap/压缩流文本抽取）；不手写 PDF 解析器，不引入概率编码探测库。lock 新增 31 个传递/直接包，按 Rust 1.88 兼容版本解析。
- 测试：先复现 UTF-8 BOM 残留、Notebook 未渲染、PDF 输出原始对象；Rust 覆盖 GBK、UTF-16 双字节序、无 BOM UTF-16、UTF-8 BOM 的 read/edit/write 与分页，GB18030 四字节字符、无法 GBK 编码时原文件不变，以及实际 image block、PDF 文本层、Notebook 文本输出；file-tools.test.mjs 验证 bridge 循环与落盘原始字节。
- 待确认决策：非 UTF-8 文本、PDF、Notebook 默认输入上限 20 MB，MINICODE_TEXT_DECODE_MAX_BYTES 可设置为 1–100,000,000 字节。无 BOM 的编码存在固有歧义，优先 UTF-8；未启用通用西文编码猜测。空 UTF-16 BOM 视为合法空文件，旧“二进制”夹具改为不完整 UTF-16。
- 验证：output/check-3c.ps1 T1 全套检查；日志 output/3C-T1-*.log，评测输出独立保存，避免覆盖既有 output/eval 配置。
- 最终 Windows 全套通过：fmt、clippy、Rust 125 项、build、协议 194 项通过/1 项既有慢测跳过、生成器与生成文件无差异、60 次主任务及 9 个场景结果校验、TUI 安装/构建/117 项测试。Desktop 未修改，不触发 Go 检查条件。

### 3C 恢复交接记录

- 前次按用户要求在 T1 收尾；本次获准继续后从 T2 恢复，逐项补失败回归、修复和提交。各条目的最终行为及验证见下文。
- 原始定位：T2 fancy_regex/固定目录遍历；T4 精确匹配；T5 默认 20000 与 schema 4000；T6 外部 schema 每次编译；T8 长度/mtime 复用；T9 无列名 INSERT；U5 逐组件 canonical。
- T1 全量检查曾暴露路径类型检查位于同文件写锁外的竞态；现已纳入锁内，既有 concurrent_edits_of_one_file_are_serialized 回归通过。两个原有 .pending 测试文件保留不动。

### T2：忽略规则与无回溯搜索

- 核查：grep 已使用缓冲读取，mmap 崩溃子问题不再存在；但仍通过 util::walk 固定排除目录，并用 fancy_regex，两个失败回归复现忽略规则无效及 lookaround 被接受。
- 修复：仅 grep 使用 ignore crate 遍历，尊重 .gitignore/.ignore、父目录与嵌套规则、否定规则，保留稳定排序、隐藏文件和原固定目录排除；显式文件仍可搜索，新增可选 include_ignored 关闭 ignore 文件规则。util::walk 保持原语义，避免改变插件指纹、信任和验收范围。匹配改用已有 regex 的无回溯引擎，不依赖宿主 rg 的版本/可用性。
- 依赖：新增 ignore 及三个 crossbeam 传递包；删除只用于该搜索路径的 fancy-regex 直接依赖（JSON Schema 所需的传递依赖仍保留）。这是实现标准忽略规则所需的专用依赖。
- 测试：Rust 覆盖 target/dist/build/.next、node_modules、.ignore、嵌套/父级/否定规则、显式文件与 include_ignored、嵌套量词及不支持语法；bridge 验证默认过滤、覆盖及可恢复语法错误；手动 t2_ignored_tree_benchmark 对比 2002 个候选与过滤后的 2 个文件，记录耗时而不设置易抖动的时间断言。
- 待确认决策：lookaround/backreferences 明确返回错误；不静默降级到回溯引擎。生成目录依赖仓库的 ignore 规则，不硬编码排除所有 build/ 等可能存放源码的目录；include_ignored 不解除原固定目录排除。
- 验证：output/check-3c.ps1 T2 全套检查，日志 output/3C-T2-*.log；基准日志 output/3C-T2-bench.log。

### T4：可控归一化与原子批量编辑

- 核查与复现：旧 edit 在空白、智能引号、Unicode 组合形式或缩进不同的情况下只报 not found；edits[] 缺 old_text 直接失败，错误不含候选。三项先失败的 Rust 回归覆盖这些路径。
- 修复：保留精确匹配优先，缺失时尝试行尾空白、智能引号与 Unicode 兼容分解的映射匹配，再尝试保持相对缩进的整行窗口；使用原文 byte range，避免拆开一个 Unicode 字符。默认仍拒绝歧义，match_mode=exact 可恢复严格匹配。edits[] 的 1–100 项基于同一原始快照规划，重叠、任一失败均不写入；普通/大文件使用同一编辑计划，保持 CRLF、原编码与 BOM。
- 候选：只返回最相近的三行及真实行号，不自动应用模糊猜测。候选扫描前 20,000 行、每行比较前 500 字符，限制错误路径成本。
- 依赖与取舍：unicode-normalization 已由 T1 PDF 依赖引入，本项将其声明为直接依赖，不新增包。自动归一化默认限 2 MiB，MINICODE_EDIT_NORMALIZE_MAX_BYTES 可设置到 20 MiB；更大文件仍支持精确编辑并给出明确提示。缩进仅允许一致的基准变化（tab 按 4 列比较），不自动改变相对缩进，也不把单双引号或拼写错误视为等价。
- 测试：归一化匹配、CRLF 与缩进保持、规范等价字符、批量偏移/失败/重叠、歧义拒绝、严格模式、大文件编辑、Unicode 局部匹配拒绝，以及 bridge 的批量与失败不落盘。检查日志 output/3C-T4-*.log。
- 完成审查补验：TUI/Native Desktop 原审批预览只读取 old_text/new_text，edits[] 会变成空 diff。两端先补失败测试再支持批量预览，并显示编辑数量/边界；实际成功结果继续优先使用 Rust 输出的真实 diff。日志 output/3C-T4-ui-red.log、output/3C-T4-desktop-red.log 与 output/3C-T4-UI-*.log。

### T5：一致的归档默认值与行视图

- 问题与复现：工具 schema 默认 4000 字符，执行默认 20000；先失败测试得到 6024 字符，证实不一致。行相关参数此前被忽略。
- 修复：模型 read_artifact 默认统一为 4000，保留已有字符 offset/limit；新增 start_line/end_line（1-based，含末行）、pattern/case_sensitive、tail_lines（先取最后 N 行再过滤）。结果带原始行号和 next_start_line 提示；同样通过会话 artifact_path 授权，不能读取别的会话。
- 兼容与取舍：Bridge 的 readArtifact 是已有 UI 固定 20000 字符页协议，保持不变；本项统一的是工具 schema 与执行。行视图不接受非零字符 offset，tail 不与行范围混用。单个参与匹配的行默认最多 1 MiB，MINICODE_ARTIFACT_LINE_MAX_BYTES 可配置到 16 MiB；更长行明确引导字符分页，范围前的超长行直接跳过。尾部只缓存受输出预算限制的匹配前缀，先在完整行上判断 regex，避免截断漏匹配。
- 测试：默认字符数、中文行范围、搜索、尾部、非法范围、长行与字符分页回退、范围跳过、尾部远端命中、跨会话拒绝；真实 bridge 四种模式与现有 UI 分页契约。全套日志 output/3C-T5-*.log。

### T6：MCP Schema Validator 缓存

- 问题与复现：内置 schema 已缓存，外部/MCP 分支每次 validator_for 编译。先用保持原行为的私有取值函数暴露 validator 身份，回归确认相同工具/schema 两次取值不是同一 Arc。
- 修复：外部 validator 按工具名与 schema 内容 SHA-256 缓存，命中复用 Arc；内容变更不会复用旧验证器。全局互斥保证并发首次调用也只编译一次；内置缓存、参数错误格式和 MCP 协议不变。
- 保留策略：最多 256 项且累计原始 schema 8 MiB，按访问顺序 LRU 淘汰，超大 schema 可执行但不缓存；不新增依赖。
- 测试：先失败的实例复用、同名 schema 变化与不同工具隔离、参数接受/拒绝；另覆盖容量淘汰与八线程共享实例。全套日志 output/3C-T6-*.log。

### T8：用内容哈希检查外部修改

- 问题与复现：Seen::of 在长度/mtime 相同时复用旧哈希，edit_large 开始及提交前也仅检查这两项。分别在小文件和 400 KB 文件上同长度改写并恢复原 mtime，先失败测试证明修改被漏检。
- 修复：读取账本刷新及编辑预检始终计算文件内容 SHA-256；大文件对实际映射内容核对原摘要，提交前再次从路径读取并哈希。仍保留已有同文件写锁与元数据快速拒绝，但元数据相同不再跳过内容校验。
- 测试：Rust 精确恢复 SystemTime（模拟 FAT/exFAT 粗粒度时间戳），覆盖大小两条路径拒绝、原文保持、重新 read 后正常编辑；bridge 覆盖真实外部命令改写大文件并恢复时间戳后的拒绝。代价是读取/编辑多一次线性内容扫描，不新增依赖。
- 范围外记录：edit_large 仍使用原有 mmap，外部同时 truncate 的进程级风险没有在本项改写；大文件的 temp.persist 路径也尚未接入 U4 的 Windows 元数据保留实现。位置见 tools.rs/edit_large，不扩大本项到另一次文件写入架构修改。
- 验证：output/check-3c.ps1 T8 全套检查，日志 output/3C-T8-*.log。

### T9：归档 INSERT 显式列名

- 问题与复现：store.spill 与 command capture 都使用无列名的 INSERT。两项失败回归将 artifacts 列重排并新增带默认值的列，旧代码报“6 columns but 5 values supplied”。
- 修复：两处明确列出 session_id、artifact_id、kind、relpath、created_at，不改变现有 schema、版本号与旧记录。
- 测试：迁移前归档仍可读取，迁移后 spill/命令截断归档成功，新增列保留默认值；覆盖两条实际落盘路径。全套日志 output/3C-T9-*.log。

### U5：一次解析已有前缀

- 问题与复现：原 canonical 逐个组件调用 symlink_metadata 和 canonicalize；32 层路径的回归记录到 38 次 canonicalize，预期一次而失败。
- 修复：先寻找最长已有前缀并 canonicalize 一次，再拼接不存在的正常组件；遇到 .. 先解析当前前缀再取真实父路径，连续 .. 不重复解析已知路径。保留 Windows extended/UNC 输出转换、断链错误与工作区越界拒绝，不用纯字符串消除 .. 来改变 junction/symlink 语义。
- 测试：已有深路径仅一次解析，六层缺失后缀最多七次 metadata 检查；Windows 实际 junction 与 Unix symlink 覆盖链接后 ..、缺失组件、断链与越界；基准测试独立标记 ignore，手动运行，不添加依赖。
- 基准：Windows 100 次调用、深度 4/16/64，旧 canonicalize 次数 1000/2200/7000，修复后均 100；64 层耗时约 500 ms 降到 12.6 ms。基准日志 output/3C-U5-bench-*.log。Linux 补充 strace 对照统计实际 readlink/readlinkat 系统调用，见 output/3C-U5-syscall-*.log。
- 取舍：普通路径与缺失后缀避免重复解析；含 .. 的路径在必要的分段边界解析，以正确处理链接，未为追求一次调用而牺牲路径安全语义。
- 验证：output/check-3c.ps1 U5 全套 Windows 检查；额外 Linux fmt/clippy/Rust/本批协议与 syscall 基准日志 output/3C-linux-final.log。

### 批次 3C 最终验证与边界

- 条目状态：T1、T2、T4、T5、T6、T8、T9、U5 均已实现并补失败回归；T1 的图片读取、T2 的非 mmap 读取属于核查后保留的已有能力。没有跳过整个条目。
- Windows 每项提交前均执行 fmt、clippy --locked --all-targets -D warnings、test/build --locked、Node 全协议、任务/trace 生成校验、两组离线评测及结果验证、npm ci/build 与真实 Rust bridge TUI 测试。最终 Rust 141 项通过，2 个手动基准常规跳过但均已实际运行；协议 198 项通过、1 项既有慢测跳过；两组评测保持 60 次主任务与 9 场景的预期结果；生成任务和 trace.log 无变化。
- T4 审批预览补修额外执行 Windows Desktop 的 go test -count=1 ./...、go vet ./...、vendor UI 测试；TUI 118 项（含真实 bridge）验证批量预览。日志 output/3C-T4-UI-*.log。
- Linux fmt/clippy、Rust 136 项（另 2 个手动基准）通过；file-tools 与 artifact-retention 共 9 项通过。附加 extensions 协议中既有 MCP 初始化取消后的进程存活判定仍失败（3B 已在 c21578d 基线复现），未把 Linux 全协议宣称为全部通过。系统调用基准深度 4/16/64 的 readlink/readlinkat 从 21/171/2211 降为 6/18/66，确认普通深路径由平方增长降为线性。
- 修改范围：Cargo.toml/lock；agent/textfile.rs、editing.rs、tools.rs、paging.rs、util.rs、store.rs、lib.rs、tests.rs、assets/tools.json；tests/agent/file-tools.test.mjs；TUI ApprovalBlock/claude-style 测试；Native markdown/file_preview_regression 测试；本修复记录。两个原有 .pending 文件不变。未推送或发布。
- 待确认产品取舍汇总：无 BOM 编码采用确定性顺序且解码上限可配置；搜索不再支持回溯语法，显式 include_ignored 可关闭 ignore 文件规则；归一化可通过 match_mode=exact 关闭且限制输入大小；批量编辑基于原始快照，拒绝重叠；归档行视图有可配置单行上限，保留原 UI 字符分页兼容性。
- 下一步建议：单独处理 Linux MCP 取消回收及 edit_large 的 mmap/元数据保留路径；继续路线下一阶段，不把这些范围外问题并入本批。

### 3.0.3 发行 CI 收尾

- 发行时 GitHub 的 Linux agent 检查复现 MCP 初始化取消后进程仍可见；/proc 确认其已被 kill 但长期保持 Z 状态。Connector Drop 显式提交 child.wait 回收，Unix MCP 设置独立进程组使后代一并终止；现有失败契约转绿，并补后代延迟写入被阻止的回归。此项在 3C 后作为发行阻塞单独修复，不再仅列为后续建议。
- Windows CI 的 metadata-only 回归失败源于测试把含短文件名别名的临时路径直接传入内部 ensure，而实际 dispatch 已 canonicalize。统一测试工作区输入后仍以 malformed JSON view 检查禁止正文读取，不修改正式会话恢复行为。



## 批次 4A：结构化压缩与 token 估算

### X1：保留进度与决策的结构化摘要

- 核查：原 context::compact 只抽取用户原话、input.path 和错误预览，summarized_units 恒为 0；assistant 结论及测试状态未进入工作集。已用原实现复跑新增测试，摘要包含关键决策的断言失败（output/4A-X1-behavior-red.log）。
- 修复：工具禁用的独立 LLM 请求使用 Goal / Constraints & Preferences / Progress（Done / In Progress / Blocked）/ Key Decisions / Next Steps / Critical Context（Read Files / Modified Files）模板。旧摘要随下一批历史输入，增量替换而非堆叠；最初用户要求继续完整保留，原文存入 history artifact。
- 有界与兼容：可选配置 compactionModel 指定摘要模型，缺省使用当前模型；摘要按窗口分块，累计 30 秒或剩余任务时限，输出上限 4096 token。模型不可用、无效/截断/过大摘要、请求失败回退抽取式；取消仍生效。摘要 usage（包括已完成但格式无效的响应）记入会话和共享预算，预留请求上界避免透支；FakeProvider 用独立可选 summary 字段，不推进任务回放位置。协议和 schema 7 保持兼容，无新依赖。
- 测试：结构化摘要保留已改文件、关键决策、测试结果、下一步；两次压缩只保留一份摘要且任务回放位置不变；摘要失败仍有原文 artifact。摘要的事实正确性仍依赖模型，artifact 可用于回查。
- 验证：提交前完整检查脚本 output/check-4a.ps1 X1，日志 output/4A-X1-*.log。
- 待确认决策：默认当前模型生成摘要会增加少量调用成本；可配置便宜模型。无配置迁移或界面新增项。

### X2：回合内切分与大块历史输入归档

- 问题与根因：切点只来自非工具结果的用户消息，单个自主回合无法归档；offload_large 只处理 ToolResult，大块 write 参数和 assistant 文本始终占据窗口。两项回归均先失败，见 output/4A-X2-red.log。
- 修复：自动压缩支持消息边界切分，保留最初用户消息和最近约 20k token（小窗口最多三分之一）；通过 paired_prefix 验证完整工具组，不拆开并行调用与结果。统计新增可选 split_turn，摘要提示明确保留未完成回合。
- 大块数据：逼近上下文硬上限时，已完成工具调用的参数和旧 assistant 文本也归档，保留预览、路径及 artifact 引用。工具 ID、调用与结果块保持不变；没有结果的待执行调用不改写。工具参数原 JSON 可从 tool_input artifact 完整恢复；无新依赖。
- 测试：单个十轮工具回合触发归档、上下文下降超过一半、首尾内容保留；原文 artifact 和保留工作集分别验证配对；大块参数/assistant 归档、原 JSON 回读、待执行参数保持原文、重复 offload 不再改写。
- 验证：提交前完整检查 output/check-4a.ps1 X2，日志 output/4A-X2-*.log。
- 取舍：工具历史参数仅用于重放给模型，归档后须通过 read_artifact 回查完整正文；不改变已执行工具的实际输入或持久执行报告。初始要求和固定 system/tool schema 仍不可压缩。

### X3：少量用户提示也可手动压缩

- 问题与复现：force 仍要求三个用户单元，且 shrink 必须超过自动阈值。新增 Rust 一/二提示长任务、单个长回答测试均先失败；真实 /compact 契约返回“当前没有可压缩的历史”，见 output/4A-X3-*-red.log。
- 修复：手动模式允许完整工具组内的消息切分，目标保留最多一半历史、20k token 与窗口三分之一中的较小值，并尽可能保留最新请求。force 在自动阈值以下也能 shrink；没有可归档前缀时，对超过 1 KiB 的 assistant 文本/工具结果保留 400 字节预览和完整 artifact。最初用户要求始终原样保留。
- 已有能力核查：CLI 与 Bridge 已经分别在 cli.rs 的 /compact 分支、bridge.rs 的 runSlash 分支报告“当前没有可压缩的历史。”，保留原反馈，不重复修改协议或界面。
- 测试：一/二提示、低于自动阈值、回合内配对、单回答归档、初始约束完整、再次压缩返回无变化；真实 bridge 单次长回复 /compact 成功，再次调用明确报告没有可压缩内容。
- 验证：完整检查 output/check-4a.ps1 X3，日志 output/4A-X3-*.log。
- 范围外观察：首次全协议运行中，windows-prompt 的 PowerShell 7 探测短暂返回“版本未知”；相同二进制单独重跑该文件 3/3 通过，随后完整协议 200 项通过、2 项既有跳过。失败原始日志 output/4A-X3-protocol-flake.log，未修改 Shell 探测逻辑。

### 4A 首次暂停时的交付范围（X3）

- 已完成并分别提交：X1 结构化增量摘要、X2 自动回合内切分与大块输入归档、X3 少量提示下的手动压缩。修改仅涉及 agent/context.rs、provider.rs、runtime.rs、tests.rs，tests/agent/context.test.mjs 与本修复记录；没有新增依赖或修改 Desktop。
- 每项提交前均完成 fmt、Clippy -D warnings、Rust test/build --locked、全协议、任务/trace 一致性、两组 B0/B1/B2 离线评测及结果验证、npm ci/build 和真实 bridge TUI 测试。最新 X3：Rust 147 通过、2 个既有手动基准跳过；协议 200 通过、2 个既有跳过；TUI 118 通过。主任务 60/60；9 个场景保持 7 个预期通过、2 个预期失败。原始日志 output/4A-X1-submit-*.log、output/4A-X2-*.log、output/4A-X3-*.log。
- 当时尚未实现（现已在下文完成）：X4 的 CJK 估算、usage 基准加增量及 ContextOverflow 后压缩重试；X5 的渐进 shrink 与 bash/grep 文件引用提取；P12 的模型校准持久化。这些条目的失败测试草稿仅在忽略的 output/ 目录中，未并入交付或宣称通过。当前 tokens 仍按字符数 / 3；prompt_scale 仍初始化为 1.0；运行时仅保留原有通用重试语义。
- 此暂停点尚未达到“超过两倍窗口的中文会话持续运行”及“provider 溢出恢复成功”验收，后续已完成 X4、X5、P12，最终结果见本节末尾。
- 风险与待确认：默认摘要使用当前模型，增加调用成本与最多 30 秒等待；可选 compactionModel 可选便宜模型。摘要事实准确性依赖模型，原文 artifact 提供回查。最初用户要求与固定提示仍不可压缩；未完成 X5 前旧工具结果仍沿用一次性 shrink 策略。
- 保持原有两个 .pending 文件不变；未推送或发布。


### X4：中文估算、usage 基准与上下文溢出恢复

- 核查：tokens 与 JSON/schema 字符数均按 /3 计算；只有 SSE 分支更新 EMA，JSON 响应不更新；Runtime 将 ContextOverflow 作为不可重试错误直接结束。前置的错误分类与有界响应正文保留已存在，继续使用。
- 修复：ASCII 保留 /3 估算，CJK 等非 ASCII 字符按约 1 token/字，覆盖文本、工具参数、工具 schema。成功的 SSE/JSON 响应统一更新校准；以实际 input usage 为已发送前缀基准，只估算追加内容。前缀、system、工具、模型/端点/effort 变化自动使基准失效，回退校准估算。Fake 默认固定 usage 不参与校准，只有显式 input_tokens 才作为回放测量。
- Runtime 的窗口判断、输出 token 余量和 UI 上下文总数统一使用 usage 基准估算。摘要分块也随校准调整输入预算。ContextOverflow 在有可压缩历史时强制压缩并立即重试当前轮一次；不增加逻辑轮次，保留取消、时限和 token 预算，失败/无缩减/二次溢出保留原错误，不做无限重试。provider_retry 新增可选 reason=context_overflow，兼容旧消费者。
- 测试：先在 X3 提交的实现上复跑新增回归，覆盖 CJK 文本/JSON/schema 估算、usage 基准与增量、前缀改写失效、溢出后当前轮仅重试一次、中文累计超过两倍窗口的 FakeProvider 回放；真实 HTTP mock 验证 context_length_exceeded 后请求缩小及恢复。既有 Unicode/JSON 估算测试同步更新权重。提交前执行完整 CI 检查，日志 output/4A-X4-*.log。
- 验证结果：全套检查通过，Rust 151 项、协议 201 项、TUI 118 项通过（保留既有跳过）；60 次主任务与 9 个场景保持预期。Fake 可选 context_window 用于回放时模拟窗口拒绝，旧脚本未设置该字段时身份哈希保持不变。


### X5：渐进 shrink 与文件引用保留

- 问题与复现：旧循环一次性缩掉全部旧工具结果；归档切点也不考虑仍相关的读文件。新增测试先失败，已读 src/keep.rs 消失；bash 参数内 agent/provider.rs 与 grep 输出内 src/runtime.rs 均未进入归档引用。日志 output/4A-X5-red.log。
- 修复：按时间与重要度排序，优先收缩普通旧输出，再处理读文件、错误与相关读文件；保留最后两个工具结果，先保留 2000 字符预览，预算仍不足再降到 200 字符，每步达到目标即停止。预览保留路径与原文 artifact，二次缩短复用完整原文的 artifact，不把预览重新作为原文。只有确实减少 token 才写入预览，统计按实际改变的结果数计数。
- 相关文件：初始/最新请求中提到的路径，其最近一次成功 read 优先留在工作集，归档不越过这些读取；默认最多 4 个，可用 MINICODE_CONTEXT_RELEVANT_READS 配置 0–16（0 关闭）。这是有界路径匹配启发式，不宣称理解所有任务依赖；固定预算不足时仍保留硬上限 offload 的兜底行为。
- 引用：收集 path/file_path、bash/PowerShell 命令中的路径、grep/命令结果中的文件名，保留历史摘要中的引用，并补到结构化摘要旁。沿用 regex 依赖，引用提取限量且仅作回查提示，不改变工具执行或路径授权。带空格、无扩展名及动态拼接的 shell 路径仍可能提取不全。
- 测试：相关读文件和最近两次结果保持原文、预算满足后保留其余旧结果、第一档仍有可用文件预览、artifact 回读与调用配对；bash/grep 引用完整进入抽取式归档。提交前完整检查 output/check-4a.ps1 X5，日志 output/4A-X5-*.log。
- 验证结果：完整检查通过，Rust 153 项、协议 201 项、TUI 118 项通过；两组离线评测与生成文件一致性通过，保留既有跳过及场景预期失败。


### P12：按模型持久化 token 校准

- 问题与复现：EMA 仅保存在 Provider 实例，重新构造 Provider 或恢复 Runtime 都回到 1.0。两个失败回归分别得到 1.0 而非已观察到的 2.0，日志 output/4A-P12-red.log。
- 修复：校准保存到配置文件旁的 <配置文件名去扩展名>.token-calibration/<模型身份哈希>.json，按配置模型 ID、实际模型、API 风格、端点与估算器版本隔离。EMA 仍限制 0.5–3，新 Provider（含切回模型、Runtime 恢复）读取该模型的系数。文件只存格式版本与系数，不存提示词、用量前缀或凭据；usage 基准按新请求重新建立。
- 兼容与失败策略：可选缓存，无配置/数据库迁移；原子替换避免并发写出半个文件，多个实例最后一次有效观察覆盖。读取最多 257 字节，拒绝超出 256 字节、损坏、未知版本、非有限数和越界系数；缓存不可用回到 1.0，写失败不丢弃成功的模型响应。
- 测试：Provider 重建、模型/端点隔离、Runtime 重新加载；跨会话没有前缀基准；损坏/过大/越界/未知版本缓存与写入失败降级。提交前完整检查 output/check-4a.ps1 P12，日志 output/4A-P12-*.log。


### 批次 4A 最终交付与验收

- 条目状态：X1、X2、X3、X4、X5、P12 全部实现，无整项跳过。X3 的“无内容可压缩”提示沿用已存在的 CLI / Bridge 实现。前面的 X3 暂停点保留为历史记录，不代表当前仍有未完成条目。
- 独立提交：X1 ab4f522；X2 f204bda；X3 867e5ba；X4 605487d；X5 3b3c293；P12 与本收尾记录同一提交。未推送或发布。
- 文件范围：agent/context.rs、provider.rs、runtime.rs、tests.rs，tests/agent/context.test.mjs 与 docs/fixes-3.0.3.md。没有新增依赖、修改 TUI/Desktop 源码或改变 SQLite schema 7；旧会话、旧配置、旧脚本和协议继续可用。原有两个 .pending 文件保持不变。
- 回归覆盖：新增 15 项 Rust 测试及 3 项 bridge/HTTP 契约测试。覆盖结构化进度/决策/已改文件/下一步、增量更新与失败回退、回合内配对切分、大块参数与原文回读、少量提示下手动压缩、CJK text/JSON/schema、usage 基准加增量与前缀失效、当前轮仅一次溢出恢复、超过两倍窗口的中文 FakeProvider 回放、渐进收缩/相关读文件/bash/grep 引用、Provider/Runtime 校准恢复、模型/端点隔离及缓存损坏/写失败降级。每项均有修复前失败证据，长中文回放同时作为集成验收。
- 验证方式：每项提交前执行原要求的完整 Windows CI 检查，包含 fmt、Clippy -D warnings、Rust test/build --locked、全协议、任务/trace 生成一致性、两组 B0/B1/B2 离线评测及结果验证、npm ci/build 和真实 bridge TUI 测试。原始日志位于 output/4A-X1-submit-*.log、output/4A-X2-*.log、output/4A-X3-*.log、output/4A-X4-*.log、output/4A-X5-*.log、output/4A-P12-*.log。
- 风险与待确认决策：摘要默认当前模型、最多 30 秒，可用 compactionModel 指定便宜模型；摘要事实准确性依赖模型，原文可回查。相关读文件按初始/最新请求中的路径匹配，默认最多 4 个、可配置 0–16；文件引用提取不覆盖全部 shell 动态路径。预算仍不足时允许硬上限兜底 offload。校准缓存是可选启发式，写入失败不会影响模型响应，多实例并发以最后一次原子写为准。初始用户要求与固定提示仍不可压缩，持续溢出会有界结束。
- 范围外观察：一次既有 Windows Shell 版本探测测试返回“版本未知”；单独重跑及随后完整检查通过，未改写 Shell 探测。本批未处理存储重复写入/全量恢复及系统通知类型隔离。
- 下一批建议：继续 4B 的系统通知与用户输入分离、消息增量写入和分页恢复；另行跟踪 Shell 版本探测的偶发不稳定，不混入本批上下文实现。
- 最终结果：Rust 156 项通过（2 个既有手动基准跳过），Node 协议 201 项通过（Windows 下 Unix MCP 用例及 200 秒慢流用例共 2 项按原条件跳过），TUI 118 项通过。主任务 60/60，9 场景保持 7 个通过与 counter_retry/b0、b1 的 2 个预期失败；结果验证、生成文件一致性及其余 CI 命令全部通过。Desktop 无改动，未触发其条件检查。


## 批次 4B：消息注入、恢复语义与存储

### R-N：系统通知与用户消息区分

- 核查：后台完成/失联、截断、停滞和动态指令变化已带 system-reminder 标签（runtime.rs 对应通知分支），保留原有实现；验收失败的 Goal::failure 未包裹。TUI historyItems 与 Native History 仍把所有 user 文本映射成用户气泡，因此恢复历史会混淆来源。
- 修复：验收失败同样使用 system-reminder 标签；TUI/Desktop 将以该标签开头的通知恢复为已有 notice 行，去除外层标签、保留全部正文（含旧版标签外的后台输出），加“系统提示”标识。真实用户文本保持 user 行；模型侧沿用原有 Anthropic 角色归一化，不新增消息角色或破坏旧会话。
- 回归：Rust 验收通知标签、HTTP mock 验收失败后的模型 payload 与相邻角色、TUI 与 Go 历史通知分类/正文保留，均先失败后修复。已有 live 通知通过 goal_check/background_job 事件生成 notice，不另加重复通知。
- 验证：完整检查 output/check-4b.ps1 R-N -Desktop；日志 output/4B-R-N-*.log。历史识别以标签为约定，不作为权限或授权依据。

### R22 取消与恢复区分未执行和副作用未知

- **问题与根因**：工具批次取消后未开始的调用没有结果；恢复把缺少结果的写操作全部视为副作用未知。`tool_call_start` 在校验与授权之前发出，不能证明执行已经开始。
- **修复**：新增兼容的 `tool_execution` journal；assistant checkpoint 原子登记排队调用，授权和参数校验通过后、实际执行前记录 `tool_started`。取消收尾立即补齐未开始调用的“用户已中断，未执行”，并接回已经落库的结果；恢复仅将可能执行过的写操作视为副作用未知。旧会话没有状态记录时保留原有保守行为。
- **测试**：`r22_cancel_backfills_unstarted_calls_before_resume` 复现两项写入第一项完成后取消，验证第二项未执行且恢复无虚假未知提示；`r22_recovery_distinguishes_queued_from_started` 验证排队与已开始写入的恢复差异。
- **验证**：完整 CI 检查通过，日志 `output/4B-R22-*.log`；Rust 159 项通过、2 项既有手动基准忽略；协议 202 项通过、2 项按既有条件跳过；TUI 119 项通过；离线评测结果验证通过。

### R25 首次只读访问嵌套目录不再要求重试

- **问题与根因**：公共 preflight 在发现嵌套 AGENTS.md 时无条件返回失败；同批后续只读调用也被 pending scope 拦截。
- **修复**：只读 read/ls/grep 继续执行并在结果前附带本轮新加载的适用目录指令，以 system-reminder 标记；指令仍通过原有动态快照进入后续模型请求。写入、编辑及 shell 保留原有先看指令再重试的屏障，不改变授权策略。
- **测试**：`r25_readonly_nested_instructions_succeed_without_an_extra_turn` 先失败后通过；覆盖同批 read/ls/grep 的首次访问、指令只发现一次、写入屏障及下一轮成功重试。完整检查日志 `output/4B-R25-*.log`。
- **验证说明**：首轮完整协议检查的既有 MCP service 超时用例在并行负载下出现 unknown tool，单独 4 项复跑通过；完整复跑还纠正了新测试对并行完成顺序的假设，改为按工具名查找。最终结果以 `output/4B-R25-final-*.log` 为准，不改动本批之外的 MCP 超时实现。

### R42 artifact 引用使用结构化字段

- **问题与根因**：runtime 对整段输出取首个 artifact 正则匹配，正文里的历史标记会覆盖本次实际归档链接，普通文件也会被误判为有归档。
- **修复**：Outcome 新增可选、默认空的 artifact_id；大块输出 spill 和命令 capture 直接赋值，record_result 直接传递字段，删除文本反解析。旧 Outcome JSON 可以继续反序列化；展示用文本标记保持兼容。
- **测试**：`r42_artifact_references_come_from_the_outcome_not_output_text` 先失败后修复，覆盖普通文件伪标记、skill 大输出 spill、命令大输出归档及正确分页回查。完整检查日志 `output/4B-R42-*.log`。

### R28 结果事件去重、结构化截断与轻量恢复

- **问题与根因**：tool_call_result 同时存 output_detail 与 journal，完整结果之后还进入 messages；截断依赖正文子串，失败时主动清空 images。Runtime/new、prepare、set_script 和 bridge 预算恢复全量读取事件。
- **修复**：新结果事件只保存 1000 字节预览、result_ref、artifact 引用和状态；完整结果仍在 journal/messages 中，维持崩溃恢复及模型历史语义。实时协议与有界历史页按引用补回旧 output_detail/images，旧事件原样可读。Outcome.truncated 由实际截断、归档和分页分支赋值，普通文本不再误判；失败结果也保留图片。恢复使用按类型过滤、按字段投影的事件数据，跳过输出流和正文；预算直接查询最新值，历史详情仍分页读取并按还原后的实际大小计入页预算。
- **测试**：Rust 两项覆盖无关损坏输出不影响恢复、控制状态/证据字段保留且 300KB 输出不进入恢复数据；协议两项先失败后通过，直接查 SQLite 验证事件没有完整输出、详情/实时协议兼容、普通 truncated 文本不误报、失败 MCP 图片保留。完整日志 `output/4B-R28-*.log`。
- **取舍**：不迁移重写已有事件；旧库体积不会立即减少，新写入不再重复存储。模型 messages 与 journal 的两份结果分别承担上下文与原子恢复职责，本批保留。恢复仍需读取相关控制/计费元数据，其数量随会话增长，但不再载入全部历史输出。
- **追加回归**：恢复与取消收尾也只查询未配对调用的 journal，空列表不查结果正文；用无关损坏 journal 记录复现并验证不会全量载入。

### S1 会话列表单条查询、标题缓存和可选分页

- **问题与根因**：sessions 对每行分别读取首条用户消息、元数据和最近活动，反复解析消息 JSON；压缩还会改变从历史即时推导的标题。
- **修复**：新增可空 auto_title 冗余列，首次用户消息同事务缓存，旧库添加列时从原首条用户消息回填；已有标题不随压缩重写。列表由单条 SQL 联结 session_meta，借助事件活动索引取更新时间；自定义标题/置顶语义保持。listSessions 新增可选 offset（非负整数）与 limit（正整数、最多 1000），返回数组不变，省略参数仍返回全部会话。
- **测试**：两项 Rust 回归先失败后通过，覆盖压缩后标题稳定、重命名与还原、旧库迁移，以及把消息正文替换成不可读视图后仍可列出 1001 个会话；本机 debug 测量 18.36ms，回归门槛 500ms 以容纳 CI 抖动。协议回归先失败，覆盖分页、末页及无效参数。完整检查日志 `output/4B-S1-*.log`。
- **待确认决策**：分页默认不启用，保留现有客户端行为；侧边栏后续可采用增量加载。标题保留最初任务含义，不再跟随压缩摘要变化。未涉及 B18 的 ensure 比较问题。

### S2 专用写线程、读写分离和进度批量提交

- **核查**：单个 Mutex<Connection> 与 FULL 同步写入确实仍在；main.rs 当前已是 4 个 tokio worker，“只有 2 个”子问题跳过，不修改线程配置。
- **修复**：新增不依赖外部库的专用 SQLite writer，通过拥有参数的任务串行执行生产写入；独立连接读取 WAL 已提交快照，写锁等待不会占住读连接。多线程 tokio 中同步等待通过 block_in_place 让出 worker，SQL/fsync 在 writer 线程完成。普通写入与 assistant/tool_started/结果/round_end/session_end 等 checkpoint 必须提交后返回；每个任务使用 savepoint，单项失败不回滚其他任务。
- **批量策略**：只有不携带消息/结果/计数器的 tool_output 预览允许先发实时事件、在 writer 中合批，默认空闲 10ms、最多 64 项，关键 checkpoint 或同实例读请求立即刷盘，关闭时也排空。MINICODE_STORE_BATCH_MS 可设置 0–1000，0 禁用等待合批。并发排队的写入可共享一次提交，保持各项回滚隔离。
- **持久化取舍 / 待确认决策**：保持 WAL + synchronous=FULL；NORMAL 能减少同步成本，但会弱化断电耐久性，本批通过预览合批减轻 fsync，不默认改变关键 checkpoint 语义。进程在合批窗口崩溃时可能丢失尚未提交的流式预览；已确认的工具执行状态、结果和模型消息不受此窗口影响。提交错误会向 checkpoint 调用方返回，不报告虚假成功。
- **测试**：先用两项失败测试复现读写互阻和单 worker 被 SQLite busy 等待饿死；修复后 5 项覆盖 WAL 读快照、async 响应、预览合批/关键提交、单任务回滚、关闭刷盘及 BEGIN 临时失败后重试。明确验证 writer 线程名与 FULL 模式；旧的 journal/原子 checkpoint 测试继续运行。完整检查日志 `output/4B-S2-*.log`。

### S4 删除会话级联清理后代与锁文件

- **问题与根因**：delete 只删除一个 session_id 的各表数据及 artifact 目录；未遍历 subagent_start 关系，也不删除 activation 锁文件。
- **修复**：用递归 SQL 收集后代（UNION 去重避免旧关系环），先取得父会话和全部存在后代的 activation 锁，活动后代令整次删除失败且保留全部数据；单事务删除各会话的消息、事件、journal、指令快照、任务与元数据，再清理已校验位于 artifact 根目录内的目录和锁文件。activation 在持锁后校验会话仍存在，已删除 ID 的迟到请求不能启动任务或留下新锁文件。不删除无关会话及工作区级记忆。
- **测试**：两项回归先失败后通过，覆盖父/子/孙级联、循环关系、artifact 与冻结提示/journal 清理、无关会话保留、活动后代阻止删除、删除后迟到 activation 不残留锁。完整检查日志 `output/4B-S4-*.log`。

### S5 artifact 尾部残缺 UTF-8 正常结束

- **问题与根因**：字符分页为跨缓冲区的标量调用 read_exact，文件末尾残缺的 2/3/4 字节序列触发 UnexpectedEof；行模式同样严格解码而失败。
- **修复**：最多读取该标量需要的字节，只有 EOF 的不完整序列输出一个 U+FFFD；行模式对同样尾部情况补替换字符。页游标按实际消耗字节回退，避免替换字符的 UTF-8 长度与残缺字节数不同造成错位。文件中部非法字节仍报错。
- **测试**：两项回归覆盖残缺 1/2/3 字节尾序列、空/中文/8191 字节前缀、零长度页、后续页/总字符数、行模式、合法 emoji 跨缓冲区及非法中部字节；原失败为 failed to fill whole buffer，修复后通过。完整检查与最终 Desktop 兼容性检查日志 `output/4B-S5-*.log`。

### R28 补充：清理已完成子会话后保留父会话回放进度

- **复核发现**：轻量投影省略了 subagent_result.provider_progress；如果父会话恰在子任务完成后因轮数预算暂停、随后清理该子会话，恢复会失去已消费的子任务脚本游标。
- **修复与回归**：在父会话的子任务完成元数据中保留 provider_progress，不加载子任务结果正文。`r28_parent_resume_keeps_progress_after_completed_child_is_deleted` 先复现恢复 index=1（应为 2），再验证删除子会话后父任务直接继续下一轮，不重复回放。单独补充提交，完整检查日志 `output/4B-R28-progress-*.log`。

### 4B 最终验证与兼容性记录

- 9 个条目全部修复，分为 9 个主体提交和 1 个 R28 恢复边界补充提交；每次提交前均运行用户列出的完整检查，最后一轮包含 Desktop 全部条件检查。未引入依赖，未改写原有两个 .pending 文件。
- 最终日志 `output/4B-R28-progress-*.log`：fmt、Clippy --locked --all-targets -D warnings、cargo test/build --locked 通过；Rust 175 通过、2 个既有手动基准忽略；Node 协议 205 通过、2 个既有条件跳过（Unix MCP 进程组、200 秒慢流）；npm ci/build 与真实 bridge TUI 119 项通过；Desktop go test ./...、go vet ./... 通过，vendor UI 包检查成功（包内无测试文件）。
- 生成的任务与 trace 无差异；主任务 60/60，9 个场景仍为 7 通过、counter_retry 的 b0/b1 两个预期失败，verify-results 验证通过。未为通过检查修改评测基线。
- 已存在而保留的部分：runtime.rs:689/833/874/1164/1456/1710 的动态、后台、截断、停滞提醒已经带标签；本批补齐 Goal::failure 与两端历史分类。main.rs:1 已有 4 个 worker，跳过“只有 2 个”子问题。
- 存储只增加兼容的 journal 表和可空标题列；旧事件读取仍兼容，输出详情通过引用补回，旧事件不批量重写。恢复保留相关控制/计费元数据，不加载无关输出；messages 与 journal 仍分别承担模型历史和原子恢复职责。
- 待确认决策：保留 FULL；预览默认空闲 10ms 合批、可设 MINICODE_STORE_BATCH_MS=0；分页参数可选，旧客户端默认仍取全量列表；首次写入新指令目录仍保留重试屏障。
- 下一批建议：客户端采用会话分页；建立长会话/多会话并发的存储压力基准；另定旧事件离线瘦身与保留策略。单独跟踪 R25 首轮遇到的既有 MCP 0.6 秒发现超时用例负载波动，不混入本批实现。

## 阶段 5A：后台任务与 MCP 生命周期

### R3 会话级后台任务、增量输出与显式终止

- 核查：`Runtime::run` 原先在局部 `jobs` 中保存 JoinHandle，收尾无条件取消全部任务；`prepare` 将没有完成事件的任务一律标记失联。现有 R-N 的 `<system-reminder>`、R28 的事件/checkpoint 与 R41 的尽力收尾机制均可复用；SEC1 插件信任流程保留。先运行新测试，正常结束后出现 `background_job_lost` 的断言失败，日志 `output/5A-R3-red.log`。
- 实现：`agent/background.rs` 的任务表由会话 Runtime 持有，保存命令、PID、独立取消令牌、磁盘输出、字节游标、完成结果和清理句柄。正常 run 结束不再取消；输出查询工具 `bash_output` 支持隐式递增游标和显式分页，`kill_shell` 等待整个进程树退出，并发/重复终止均可安全等待。任务表最多保留 128 项，每个 capture 沿用 100 MB 限额、长输出 artifact 和显式 timeout。后台命令完成时也收回仍存活的后代；前台命令现有的正常释放行为保持不变。
- 通知：独立任务在模型答复及会话空闲时写完成事件；超过 4000 字节的提醒正文保留 artifact 引用。下一次模型请求前查找未通知的完成/失联事件，并以 `background_job_notified` 与 reminder 消息的原子 checkpoint 确认送达。普通用户输入不承担任务通知。事件写失败保留内存结果并在下一轮/关闭时重试，不伪造成功。
- 清理：运行取消、异常/panic、收尾存储错误、空闲 `cancelTurn`、删除会话、Bridge EOF、CLI 退出/切会话、已完成子会话均显式等待清理；最后一个任务表 owner 被释放时也取消进程。Windows 使用现有 Job Object，Unix 使用现有进程组；不能跨 agent 进程重启继续运行，恢复时没有在本进程管理的未完成任务仍按失联报告，已完成提醒和归档可恢复。
- 回归：3 项 Rust 测试覆盖跨回合/空闲完成/提醒只送一次、增量游标、未知任务/越界游标、并发/重复终止、Drop 清理、Unicode 长输出分页、退出码/超时和重启恢复提醒；保留并更新原 R41 panic/存储失败测试。4 项真实 Bridge 测试启动 HTTP dev server，验证下一轮服务仍可访问、新增输出、终止、空闲停止、删除及 EOF 后 PID 消失。
- 兼容性：仅新增工具与事件，不修改已有协议版本、会话 schema 或配置；无新依赖。会话关闭不再保留活进程，恢复不尝试根据旧 PID 操作未知进程，避免 PID 重用风险。两个既有 `.pending` 文件保持原样。
- 完整验证：`output/check-5.ps1 -Item R3 -Desktop` 全部通过：fmt、Clippy `--locked --all-targets -D warnings`、Rust test/build、全协议 209 通过/2 个既有跳过、生成任务/trace 无差异、60 次主任务与 9 个场景符合预期、npm ci/build、真实 Bridge TUI 119 通过，以及 Desktop go test/vet/vendor UI（包内无测试）。日志前缀 `output/5-R3-`。最后补齐 CLI/子会话/外层 panic 清理及提醒 artifact 后，重新执行 fmt、Clippy、全 Rust 178 通过/2 个既有手动基准忽略、build 和 7 项后台/生命周期真实协议回归，日志 `output/5-R3-*-final*`。Linux/macOS 行为本机不可执行，不能据 Windows 结果标记通过。

### R6 按会话复用 MCP、增量重连与独立响应分发

- 核查：原 `connect_mcp` 每次调用 `prepare`，`run` 收尾无条件关闭连接；子代理在工具白名单筛选前已启动所有服务。失败用例确认第二轮计数从 1 重新开始、`.mcp.json` 更新后工具报配置失效，以及只读 explore 启动第二个进程，日志 `output/5A-R6-red.log`。
- 生命周期：正常回合保留注册表；每回合刷新 Catalog 的配置快照，按每个服务的命令/参数/cwd/传输/env/headers/timeout/来源等判断复用。插件还须指纹一致；本地导入文件内其他服务改变时，未变服务仅更新完整性验证快照。撤销信任、移除、变化或断开的连接先关闭，再启动替代服务，防止旧进程占用端口或等待审批期间继续运行；不变服务保留状态。子代理先按工具白名单筛选服务，不包含 MCP 工具的子代理完全跳过连接和信任请求。
- 并发：注册表保存 `Arc<Connector>`，取消覆盖整次调用的 connector 互斥锁；独立 stdio reader 保有完整帧缓冲，并将响应按请求 ID 分发到 oneshot。仅写入完整请求帧时使用异步锁；迟到/无关响应不交给其他请求，服务端 ping/不支持的请求继续回复。旧 SSE 连接同样有独立 reader 与请求表；Streamable HTTP 每次响应流独立，保留会话及协商后的协议头。
- 恢复与关闭：EOF、损坏/超大帧关闭请求表并唤醒等待者，下一模型轮/下一 run 增量重连。HTTP 网络连接失败或 404/410 会使旧会话失效，下一模型轮重新初始化。取消和异常收尾、CLI/Bridge 会话退出、删除、子会话完成均关闭进程树和 reader；Drop 继续由原进程树机制兜底。HTTP 连接释放本地客户端资源，远端会话保留期仍由服务端控制，不额外假定服务支持 DELETE。
- 测试：6 项真实 Bridge 回归覆盖五轮一次启动、两服务只重连配置变化的一方、子代理不连接、外部杀进程后的恢复、已重新锁定/信任的插件指纹变更只重连该插件、HTTP 会话跨回合复用及过期后在同一 run 内恢复。Rust 用实际 Node 服务等待两个并发请求后逆序回复，并夹入无关响应/同 ID 服务端 ping；两调用分别拿到正确结果，关闭连接立即唤醒未完成请求。已运行原 MCP 传输、权限信任、超时与 R41 生命周期回归。
- 后续依赖：本项提供独立 reader 和取消时移除待响应请求的基础；MC4 仍须补专门的超时/半帧/取消测试及必要的写入中断处理；MC5 尚未扩展 ID 兼容表示。父/子连接资源共享在 R35 中继续处理，当前子代理仍只连接自己获准使用的服务。Catalog 和插件验证缓存依次在 EXT1、EXT2 实现。
- 验证：完整检查脚本 `output/check-5.ps1 -Item R6 -Desktop` 全部通过，日志前缀 `output/5-R6-`：fmt、Clippy、Rust 179 通过/2 个既有忽略、build、协议 215 通过/2 个既有条件跳过、生成文件无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过，以及 Desktop go test/vet/vendor UI。首次 Clippy 的 `manual_inspect` 已按建议改为 `inspect_err`，随后完整重跑通过。无新依赖、配置迁移或协议版本变化；Linux/macOS 执行结果不在 Windows 验证范围内。

### EXT1 进程级 Catalog 发现缓存

- 核查：`Catalog::scan` 在每次 Runtime/子代理构造和扩展刷新时完整读取指令、技能头、插件、导入配置及代理定义。先在完整扫描入口加入仅测试可见的计数，连续两个相同构造实际扫描 2 次（预期 1），日志 `output/5A-EXT1-red.log`。
- 实现：`agent/extension_cache.rs` 按规范化工作区、配置路径、锁验证模式和环境指纹缓存最多 32 个发现快照；每个 key 的锁合并并发构造，其他工作区可并行扫描。环境只保留哈希，HOME/导入环境变量改变不会复用旧结果。返回独立 Catalog 副本，技能激活、已加载的子目录指令及 pending_scopes 保持会话私有。
- 失效：发现时记录所有影响结果的文件、目录、尚不存在的候选路径、配置/信任库/锁文件、插件遍历节点和符号链接状态。命中时只检查这些路径的元数据，不重复目录遍历、解析或内容哈希；新增/修改/删除和原子替换使缓存失效。除 mtime/size 外还记录创建时间、类型/属性，Unix 记录设备/inode/ctime；不可读依赖、扫描期间变化和失败结果不缓存。嵌套跟踪与 panic 都恢复线程状态，避免污染后续扫描。
- 兼容与集成：原 `check_lock` 校验、插件指纹、信任流程和 Source 大小限制仍执行，未检查锁的快照不能用于要求检查锁的调用。R6 的刷新与 Runtime::new 自动使用同一缓存；refresh 继续执行原有会话指令和激活技能规则。无新依赖、配置/协议/schema 变化。插件完整性指纹本身尚按原实现计算，EXT2 在相同依赖检测机制上继续扩展。
- 测试：4 项覆盖重复构造仅一次完整扫描、激活/目录状态隔离、8 个并发构造合并、SKILL.md/AGENTS.md 新增修改删除、本地 MCP 配置及启用状态变化、插件新增/代码变化/删除、旧锁拒绝和 check_lock 模式隔离。测试选择明确的项目 MCP 条目，不依赖宿主机导入条目的排序。
- 验证：完整脚本 `output/check-5.ps1 -Item EXT1 -Desktop` 全部通过，日志前缀 `output/5-EXT1-`：fmt、Clippy、Rust 183 通过/2 个既有忽略、build、协议 215 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。Linux/macOS 运行验证仍需对应平台。

### EXT2 缓存插件完整性指纹与导入配置摘要

- 核查：`Server::verify` 在连接和每次调用时重读本地 MCP 来源或重新遍历/哈希插件全部文件；Catalog 发现也重复计算同一内容。计数失败测试确认相同导入文件的两次校验进行了 2 次哈希，日志 `output/5A-EXT2-red.log`。
- 实现：在 EXT1 的同一模块复用元数据签名、线程依赖跟踪、并发合并和失效策略；按规范化路径及文件/目录类型缓存最多 128 个摘要。mtime、size、类型/属性、创建时间（Unix 还包含 inode/ctime）变化后重新计算，目录节点用于发现新增/删除文件。完整性比较仍在每次 `verify` 执行，不缓存“已获信任”布尔值、不修改锁格式或授权规则；错误和扫描期间发生变化的结果不缓存。
- 集成与边界：Catalog 命中指纹缓存时继承其全部文件/目录依赖，避免外层 Catalog 因内层命中而漏掉代码变化。插件根的链接/junction 检查在缓存查找前执行，规范化到相同目录的别名不能借用缓存绕过链接拒绝。已有文件数量、内容大小和插件内部链接检查仍由原哈希路径执行；本地导入配置更新仍会被 `Server::verify` 拒绝直至刷新。
- 测试：4 项覆盖反复实际 Server::verify 只哈希一次、同长度原子替换后旧指纹拒绝、文件新增/删除、导入配置删除、已缓存根目录的链接别名拒绝，以及先预热内层缓存后深层代码修改仍使 Catalog/锁校验失效。Windows junction 场景已实际执行；Unix 对应 symlink 分支需 Linux/macOS 验证。
- 验证：完整脚本 `output/check-5.ps1 -Item EXT2 -Desktop` 全部通过，日志前缀 `output/5-EXT2-`：fmt、Clippy、Rust 187 通过/2 个既有忽略、build、协议 215 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。首次 Clippy 的缓存类型复杂度检查通过类型别名修正，随后完整重跑。无新依赖或持久格式变化。

### MC4 超时、取消与协议同步恢复

- 核查：R6 的独立 reader 已能保留响应半帧，但 `write_all` 在超时/取消时仍可能只写入部分请求而保持连接可复用。真实服务暂停 stdin 后，大请求超时，后续 fast 请求继续超时；响应半帧用例在 R6 基础上已通过。失败日志 `output/5A-MC4-red.log`。
- 修复：请求写入取得 stdin 锁后设置完整帧守卫；写入/flush 失败或 future 被取消时关闭待响应表并标记连接不可复用，R6 的增量恢复在下一模型轮关闭旧进程并重建。尚在等待写锁的请求取消不污染正在写入的请求；完整请求已经发送、只在等待响应时取消，则仅移除该请求注册，独立 reader 继续读完整帧并丢弃迟到结果。
- 回归：3 项真实 Bridge 测试覆盖 1 MiB 部分请求写超时后重连成功、半帧响应超时后同连接正确取得下一结果、损坏 JSON 帧后重连。Rust 直接取消已经收到半帧的 rpc future，验证连接仍可用且下一响应不被旧 ID 替代。所有进程均沿会话清理路径回收，不自动重试状态未知的工具副作用。
- 兼容：不改变 ID 表示或工具协议，保留有界帧大小和取消/超时结果；无法保证输入流同步时重建连接，有状态服务此时会重新初始化。无新依赖。
- 验证：完整脚本 `output/check-5.ps1 -Item MC4 -Desktop` 全部通过，日志前缀 `output/5-MC4-`：fmt、Clippy、Rust 188 通过/2 个既有忽略、build、协议 218 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。仅 Windows 分支在本机执行。

### MC5 兼容响应 ID 并避免数值舍入误匹配

- 核查：stdio/旧 SSE 的响应表只接受 JSON 整数，HTTP 分支严格比较 Value；返回 `"1"` 或 `1.0` 无法完成初始化/工具调用。10 项真实传输测试在原实现上 8 项失败、2 项控制用例通过，日志 `output/5A-MC5-red.log`。
- 修复：所有传输共用 `mcp_dispatch::decode`，从原始 JSON ID 文本归一化非负 u64 整数、整数值小数/指数表示及规范十进制数字字符串。字符串 `"01"`、`"1.0"`、带空白/符号的字符串、非整数、负数、bool/null/复合值及溢出不参与匹配；文本最多 128 字节。服务端请求的 ID 保留原表示以便正确回复，不与客户端请求表混淆。
- 精度与边界：直接按十进制位数和指数检查整数性及 u64 溢出，不先转 f64；`1.0000000000000001`、`0.99999999999999999` 和相邻大整数的小数不能舍入成另一个待响应 ID。新增 serde_json 的 `raw_value` 功能读取原始 ID，无新增包、版本或 lockfile 变化，也不启用影响整个应用数值行为的 arbitrary_precision。重复 ID 字段属于不明确帧，拒绝并沿 MC4 路径关闭连接。
- 回归：2 项 Rust 测试覆盖数字/字符串/浮点/指数、u64 最大值、浮点精度边界、无效值、极端指数、重复字段与服务端请求；10 项真实 Bridge 测试覆盖 stdio 三种 ID、HTTP JSON/Streamable SSE/旧 SSE 的字符串和浮点 ID，以及无效/舍入 ID 不抢占正确响应。MC4 的半帧和并发分发回归继续运行。
- 验证：完整脚本 `output/check-5.ps1 -Item MC5 -Desktop` 全部通过，日志前缀 `output/5-MC5-`：fmt、Clippy、Rust 190 通过/2 个既有忽略、build、协议 228 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。作为阶段 5A 六项主体完成回归，包含前五项全部测试。仅本机 Windows 平台可实际执行。

### R3 补充：无效 continue 保持空闲资源

- 问题：没有暂停任务的 `/continue` 仍进入 run_inner，随后被作为运行异常收尾并关闭空闲 dev server。新增 `r3_invalid_continue_does_not_close_idle_session_resources` 先失败，日志 `output/5A-invalid-continue-red.log`。
- 修复：在开始 run 和取得运行锁之前验证 pending；无效 continue 直接报错，保持后台任务和 MCP 状态。聚焦测试通过，日志 `output/5A-invalid-continue-focused.log`。
- 验证：最终阶段完整检查 `output/5-5A-final-*` 全部通过，包含本回归和后续 R6 资源持有者补充。无协议或存储迁移。

### R3/R6 阶段复核：资源持有者与跨进程会话锁

- 核查与失败证据：Bridge 原有六个空闲 owner 的缓存会回收后台任务表或仍活跃的 MCP 连接；七会话回归表现为原会话 cancelTurn 失去控制权、MCP 计数从 1 重启。另一个 Bridge 进程还可删除正在持有空闲后台/MCP 资源的会话，因为 activation 锁原先随每次 run 释放。日志 `output/5A-owner-retention-red.log`、`output/5A-resource-ownership-red.log`。
- 修复：Runtime 明确报告保留的任务输出、活跃 MCP 和子会话资源；这些 owner 不参与普通空闲缓存淘汰。正常 run 结束后有资源时继续持有既有 SessionLock，同 owner 下次 run 复用锁；无资源、显式关闭或失败清理后释放。清理期间锁保留到最终事件完成，避免另一个进程在 teardown 中途删除数据；替换 Runtime 也先等待资源清理。跨进程接管/删除须等原资源 owner 退出，同 Bridge 的多视图仍共享 owner。
- 验证：新增 4 项真实 Bridge 测试覆盖七个会话后仍能查询/终止原后台服务、MCP 状态不重置、另一进程不能删除两类资源所属会话，以及原 owner 关闭后删除成功、进程全部退出。已运行两个文件共 14 项聚焦回归；首轮纯 owner 缓存修复的全套检查也通过。最终完整脚本 `output/check-5.ps1 -Item resource-ownership -Desktop`、日志 `output/5-resource-ownership-`，完整检查已通过。
- 兼容：复用阶段 0/4 的锁文件与删除保护，不新增存储格式或锁类型。持有活资源的会话数量可能超过普通六 owner 缓存上限，这是会话级资源存活的必要行为；删除/退出仍执行确定的清理。
- 验证记录：首次资源锁回归全测中，未改动的 `concurrent_edits_of_one_file_are_serialized` 出现一次 Windows os error 2。tools/util/editing/tests 的对应逻辑与 MC5 提交一致；保留日志 `output/5-resource-ownership-test-flake.log`，连续 8 次聚焦复跑和随后完整资源锁检查通过，未修改该并发编辑逻辑，也不声称已根治此时序波动。补齐无效 continue 后的最终全阶段检查使用 `output/check-5.ps1 -Item 5A-final -Desktop`、日志 `output/5-5A-final-`，完整检查已通过。

- 最终阶段结果：`5A-final` 的 fmt、Clippy、Rust 191 通过/2 个既有忽略、build、协议 232 通过/2 个既有跳过、生成文件检查、60 次主任务与 9 个场景、npm ci/build、TUI 119 通过，以及 Desktop go test/vet/vendor UI 全部通过。Runtime 替换在更新快照和设置前完成旧资源清理。

## 阶段 5B：子代理可靠性与隔离

### R31 容错结构化结果与 submit_result

- 核查：原实现直接 `serde_json::from_str(final_text)`；围栏、附加说明和 submit_result 回归失败，裸 JSON、非法结构拒绝和父会话不可调用的控制场景通过。日志 `output/5B-R31-red.log`（7 项中 4 项失败）。已有 summary 字符串及三个字符串数组的结构检查继续使用，不放宽元素类型。
- 结果提交：仅子代理获得 submit_result 工具，仍执行 schema、白名单及权限授权。成功提交立即结束子任务，补齐同一响应后续工具的“未执行”回执；不消耗父任务的下一轮模型响应。新工具拒绝未知参数，不能通过额外 path 字段伪造文件接触记录。父会话的工具表不包含 submit_result，旧裸 JSON 路径和文本结果中的已有额外字段保持兼容。
- 恢复：提交事件只保存 call_id，实际结果从既有 tool_call_start 参数取得，避免重复持久化正文。提交标记落库后才返回成功；工具回执写入失败时可恢复并幂等补回回执，无需再次请求子模型。正常结果仍经过原 evidence_refs 验证流程，R31b 继续处理路径及搜索命中的真实性。
- 文本回退：`agent/subagent_result.rs` 支持裸 JSON、Markdown JSON 围栏及说明中的首个合法结果对象；识别到无关的完整对象/数组时跳过整个值，防止从嵌套示例中抽出结果。最多解析 128 个候选，拒绝类型错误或不完整结构。沿用既有提供商/协议体积限制，不给原本合法的大结果新增更低上限。
- 测试：8 项真实 Bridge 回归覆盖三种文本形式、工具提交直接完成、非法类型/未知参数修正、嵌套无关对象拒绝、父会话拒绝、提交后不执行写入；2 项 Rust 回归覆盖解析边界/大结果，以及注入回执写失败后重载并恢复，确认子会话模型请求与提交标记均只有一次。
- 验证：完整脚本 `output/check-5.ps1 -Item R31 -Desktop` 全部通过，日志前缀 `output/5-R31-`：fmt、Clippy、Rust 193 通过/2 个既有忽略、build、协议 240 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。首次 Clippy 的测试迭代器告警改为从末尾查找后重跑通过。无新依赖或 schema 迁移；新增工具/事件兼容旧客户端，子任务进度展示在 R33 完成。

### R31b 真实文件接触记录与证据路径归一化

- 核查：原代码仅比较成功调用的原始 input.path；未指定 path 的 grep、`./README.md` 与 `README.md`、分页 ls 均误报。还发现 skills_list 携带一个未使用的 path 参数便能把未读文件标成已验证。4 项真实失败回归均复现，日志 `output/5B-R31b-red.log`。当前工具表没有独立 glob 工具；glob 是 grep 的筛选参数，本项覆盖其实际命中文件，不新增重复工具。
- 修复：Outcome 增加兼容的可选 observed_paths；原生 read/write/edit 成功后记录规范路径，grep 只记录真正产生命中的文件，ls 记录实际显示的分页条目和所访问目录。元数据由执行器产生并随成功回执落库，验证不解析可伪造的输出文本，也不把纯控制工具的额外 path 参数当作接触证明。失败调用、无命中 grep、未显示的分页条目和工作区外引用保持未验证。
- 路径：执行器和验证器复用工作区 canonical 边界；引用只移除尾部行号/列号/行范围，保留 Windows 盘符，支持相对路径、`./`、Windows 分隔符与实际文件大小写。路径字符串仅在 Windows 转换反斜杠，避免把 Unix 文件名里的字面反斜杠误当目录分隔符；不可无损表示的原生路径不授予引用证明。
- 存储与兼容：R28 的恢复投影补充工具名和 observed_paths，不读取输出正文。旧回执没有元数据时，仅对成功的原生 read/write/edit/ls 参数做保守回退，工具名可从对应调用恢复；旧 grep 没有可靠命中元数据时须重新 grep/read，不从任意输出文本推断授权。旧 Outcome 缺少新字段时仍正常反序列化，无 schema 迁移或新依赖。
- 测试：6 项真实 Bridge 回归覆盖 grep glob/无 path、等价路径/行列号/Windows 大小写、伪造和工作区外文件、ls 分页、纯工具参数伪造、无命中显式 grep，以及移除新字段后重载子会话的旧记录兼容。2 项 Rust 回归覆盖原生接触元数据、无命中/分页/普通读取、旧 Outcome 和盘符/位置后缀解析。
- 验证：完整脚本 `output/check-5.ps1 -Item R31b -Desktop` 全部通过，日志前缀 `output/5-R31b-`：fmt、Clippy、Rust 195 通过/2 个既有忽略、build、协议 246 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 119 通过、Desktop go test/vet/vendor UI。仅 Windows 分支在本机执行。

### R33 子任务实时摘要与可折叠进度

- 核查：delegate 的 sink 只转发审批；两个客户端都按 kind/task 关联子会话，相同任务会串行或并发串错。新增协议、TUI reducer/快捷键及 Desktop 模型/真实界面测试先失败，见 `output/5-R33-red-protocol.log`、`output/5-R33-red-tui.log`、`output/5-R33-red-desktop.log`、`output/5-R33-red-desktop-ui.log`。
- 后端：subagent_start/result 新增可选父 call_id；子 round_start 和工具 start/result 转为父会话持久化 subagent_progress，携带 child_session_id、原子会话事件序号、工具调用 ID、名称和状态。按白名单构造摘要，不复制参数、路径、输出、推理或错误正文。子任务失败也有安全的终止事件，暂停和取消继续沿用既有 exit_reason。
- 展示：TUI 在 delegate 行显示当前活动及工具次数，Ctrl+O 展开/收起调用列表；Desktop 沿用带 Expanded 无障碍语义的原工具披露按钮，折叠时仍显示摘要。两端 live/history 共用各自的归约逻辑，按父调用 ID 精确关联；按子调用 ID 去重、按子事件序号排序且旧 start 不覆盖新 result，终止后不恢复为运行中。旧 subagent_start 保留 kind/task 回退及子会话跳转。
- 测试：3 项真实 Bridge 回归验证父会话收到并持久化顺序正确的进度、工具开始/完成摘要、敏感字段缺席、暂停及异常终止；TUI 验证相同任务绑定、逆序/重复事件、live/history 一致及运行中 Ctrl+O；Desktop 验证同样的模型行为及真实按钮展开/折叠。无新依赖、无数据库迁移；不支持新事件的旧客户端继续忽略它。
- 验证：完整 `output/check-5.ps1 -Item R33 -Desktop` 通过，日志前缀 `output/5-R33-`：fmt、Clippy、Rust 195 通过/2 个既有忽略、build、协议 249 通过/2 个既有跳过、生成文件无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 121 通过、Desktop test/vet/vendor UI。首次全测新增错误用例误把父会话终止方式当作子错误验收条件，按现有 provider_error 语义校正后聚焦及全测通过，保留 `output/5-R33-first-protocol.log`。仅在本机 Windows 执行。

### R35 父级资源借用与真实用户 requirements

- 核查：每个 child 仍创建/关闭独立 MCP 注册表；状态计数回归得到 state:1 而非父级连续 state:2，失败子任务也额外启动第二个服务。requirements 从最后 12 条 role=user 直接拼接，20 条提醒挤掉真实输入，单条 UTF-8 请求超过 40 KB 无截断。失败日志 `output/5-R35-red-protocol.log`、`output/5-R35-red-rust.log`。
- 资源：子 Runtime 构造直接克隆父级已解析 Catalog，保持独立可变状态，后续发现刷新仍复用 EXT1/EXT2 缓存及变更校验。MCP restricted 视图只暴露白名单的精确工具，并记录连接属于父级借用；子任务正常完成、失败、取消、无 MCP 工具、增量重连和移除服务均不得关闭借用连接。只有实际拥有的连接参与资源所有权/会话锁保留。父会话关闭仍统一清理；借用连接断开/变化时，子任务可创建并清理自己的恢复连接，不更改父级所有权。
- 权限：保留子代理原有 allowed/specs/preflight 三层过滤，复用连接不授予同服务其他工具或兄弟服务权限。saveAgent 原先拒绝所有精确 MCP 工具名，只能 inheritTools；现允许显式 mcp__server__tool 名称，拒绝通配符及空段，运行时仍须真实发现和授权。无 MCP 白名单的 child 不建立连接。
- requirements：先筛选真实 user 文本，排除含工具回执的消息、空消息、带 system-reminder 标签的系统通知及归档摘要，再选最近 12 条，以时间顺序交给 child。总计最多 16,384 UTF-8 字节，优先保留较新请求；单条过长保留头部约束和末尾请求，用可见标记说明截断，始终在字符边界截断。并行 branch 也在取最近输入前应用相同筛选。
- 回归：3 项真实 Bridge 测试覆盖精确白名单和继承白名单复用父进程/连续计数、子任务成功/失败后父连接仍可用、兄弟 MCP 调用被拒绝、非法白名单格式；2 项 Rust 测试覆盖提醒/归档/混合工具消息排除、筛选顺序、真实请求顺序及多字节头尾截断。无新依赖或存储迁移。
- 指令兼容复核：旧 audit 用例通过 system prompt 检查 Skill 正文，实际上依赖合成动态提醒被混入 requirements。R35 去除污染后，Skill 仍由子任务的动态指令快照正确继承；用例现同时验证正文存在于 child user 动态快照、且不被复制进 system requirements。聚焦通过，首轮全测日志保留 `output/5-R35-first-protocol.log`。
- 验证：完整 `output/check-5.ps1 -Item R35 -Desktop` 通过，日志前缀 `output/5-R35-`：fmt、Clippy、Rust 197 通过/2 个既有忽略、build、协议 252 通过/2 个既有跳过、生成文件无差异、60 次主任务与 9 个场景保持预期、npm ci/build、TUI 121 通过、Desktop test/vet/vendor UI。仅 Windows 在本机执行。

### R43 子代理 compaction 设置一致继承

- 核查：eval 的 b0 明确设置 Runtime.compaction=false；并行 delegate 分支只把父开关传给中间 Runtime，随后串行/并行共用的真实 child 构造仍使用默认 true。两个新增回归均复现 false→true 的错误，见 `output/5-R43-red.log`。
- 修复：在 delegate 的统一 child 激活路径上设置 child.compaction=self.compaction，既覆盖新创建/恢复的 child，也覆盖并行分支真正创建的 child。Runtime 的普通默认值及评测 b0/b1/b2 定义保持原状，无协议、配置或依赖变化。
- 测试：串行回归覆盖父开关 false/true、新 child 继承，以及同一暂停 child 在父开关切换后继续执行；真实本地 HTTP 提供商回归触发实际双 child 并行分支，在两个开关值下验证返回父会话的两个实际 Runtime。聚焦通过，见 `output/5-R43-focused.log`；不调用真实模型 API。
- 验证：完整 `output/check-5.ps1 -Item R43 -Desktop` 全部通过，日志前缀 `output/5-R43-`：fmt、Clippy、Rust 199 通过/2 个既有忽略、build、协议 252 通过/2 个既有跳过、任务/trace 无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 121 通过、Desktop test/vet/vendor UI。本机仅执行 Windows 平台。

### A4 默认只读、独立快照与父会话审阅合并

- 核查：fixer/ui-designer 的文件工具和 shell 在父工作区直接执行，bypass/accept_edits 会跳过文件审批，默认委派即可改写主目录。新增默认只读、显式可写但未合并的两项协议回归均先失败，见 `output/5-A4-red-protocol.log`。
- 默认边界：delegate 新增可选 writable，默认为 false；子工具白名单再与只读能力相交，不能通过模式、inheritTools、伪造工具调用或直接打开子会话恢复写权限。MCP 仅在原白名单内且服务声明 readOnlyHint=true 时可供只读 child 使用；未声明的工具保守排除。该提示是已信任 MCP 服务的能力声明，不把远端服务当成本机文件系统沙箱。隔离可写 child 不借用在父目录运行的 MCP，防止绕过本地隔离。
- 隔离：显式可写任务在会话存储的 delegates/随机 ID/work 中使用独立文件副本；base 保存创建时的字节/权限，复制当前未提交内容。Git 项目取 tracked 和未忽略的 untracked 文件，未安装 Git或非 Git 目录使用既有 ignore 遍历；不复制 .git、硬链接或目录链接，必要的已固定项目指令额外复制并重新绑定到隔离路径。文件工具继续执行真实 canonical 工作区边界，并禁止写入 .git 或通过路径别名触及 Git 元数据。创建失败清理临时目录，不影响主工作区。
- Shell：本项在运行时 preflight 拒绝隔离 child 的 bash/powershell，不能靠 cwd 或提示词假装限制绝对路径。后续 SEC2 为该入口提供真实 OS 沙箱；在沙箱不可用时继续明确拒绝执行。内置代理描述同步说明需显式请求隔离可写任务，不再默认为可修改主目录。
- 审阅：父会话专有 review_delegate(delegate_call_id, action) 提供 inspect/merge/discard。inspect 输出完整 unified diff，按现有 50 KiB 工具上限归档大 diff，read_artifact 可分页读取完整内容；非 UTF-8 变化明确标注为二进制。以路径、前后内容和权限的摘要绑定审阅版本，merge 只能出现在后续模型响应，不能在同一批次中跳过查看结果；快照改变须重新 inspect。bypass、accept_edits 和旧 alwaysAllow 都不能跳过独立合并审批。拒绝或 discard 删除副本，主目录不变。
- 合并：复用原子文件替换；取得进程间合并锁后，在首次写入前核对所有父文件是否仍等于 base。已有脏内容允许保留并作为基线；后续外部修改、链接/别名、文件类型变化或 diff 摘要变化均拒绝。应用中失败时仅回滚本次写入并移除本次创建的空目录；如果外部进程又改了已写文件，不覆盖它，而是报告需复核的回滚冲突。权限变化随文件保存，不操作主仓库的 index 和提交，保留与本次 diff 无关的未提交内容。
- 生命周期：快照和审阅状态持久化，父进程中断后可恢复审阅或恢复同一个暂停 child；Windows 路径在创建/恢复时统一 canonical，避免长路径表示差异。正常回合保留待审改动和父 SessionLock，取消/关闭/拒绝后清理。清理遇到链接或文件系统错误会报告并继续持有锁，阻止另一进程提前删除会话；修复后可重试。Store.delete 同样在 activation 锁下清理崩溃 owner 留下的快照，再删除会话日志，避免失去目录归属信息。
- 回归：17 项真实 Bridge 场景覆盖默认只读、显式隔离、两种模式下批准/拒绝合并、脏内容和审批期间冲突、跨响应审阅、无沙箱拒绝 shell、取消、直接打开 child、父进程恢复、暂停可写 child 恢复、大 diff 完整归档、创建失败、清理失败后跨进程锁及崩溃后删除清理；5 项 Rust 场景覆盖独立副本/持久审阅摘要、逐文件失败回滚与新目录回滚、创建中途失败、Windows junction/Unix symlink 及清理重定向拒绝、Git dirty/untracked/ignored 数据。新增清理锁和删除回归的失败证据为 `output/5-A4-cleanup-ownership-red.log`、`output/5-A4-delete-red.log`。协议 helper 也修正 signalCode 已终止的进程清理，避免强制退出测试等待已错过的 exit 事件。
- 兼容与限制：无新依赖或数据库表迁移，新工具、delegate 参数、ToolSpec hint、session_start 元数据均可选。旧父会话仍可恢复；新 child 会话持久化白名单/隔离边界。单次快照上限 50,000 文件/512 MiB、单文件 64 MiB，超限明确失败，避免复制构建输出耗尽存储。链接和 submodule 本身不作为可写副本导入，需在实际目标目录单独委派；不把完整副本描述成 OS 进程沙箱。
- 最终验证：完整 `output/check-5.ps1 -Item A4-complete -Desktop` 全部通过，日志前缀 `output/5-A4-complete-`：fmt、Clippy、Rust 204 通过/2 个既有忽略、build、协议 269 通过/2 个既有跳过、生成文件无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 121 通过、Desktop test/vet/vendor UI。首次核心检查 `A4`、路径/大 diff 检查 `A4-final`、清理锁检查 `A4-ownership` 亦通过；最终检查在 Store.delete 清理补齐后重新构建并执行，未复用之前的评测目录。本机执行 Windows；Unix 的 symlink/权限分支尚未在 Linux/macOS 实机运行。

## 阶段 6：权限、凭据与沙箱

### R26 分层权限规则与有范围的审批

- 核查：旧 tools::policy 只按名称/模式决定，Bridge 把记住审批变成 always_allow 整个工具；bypass 放行任意 MCP。新增路径 deny 和复合命令 deny 两项回归先失败，见 `output/6-R26-red-protocol.log`。
- DSL：permissions.allow/ask/deny 支持工具名模式（包括 mcp__server__*）、bash/powershell 命令精确匹配及 token 前缀 `:*`、工作区相对文件 glob。读取用户 settings.json、项目 .minicode/settings.json、项目本地 settings.local.json、CLI --allow/--ask/--deny，按低到高累加规则；新层贡献不删除旧层的显式拒绝。最终全局 deny > ask > allow > 模式默认，重复规则不改变结果；错误类型、未知 effect、无效 glob/相对路径和超限输入明确拒绝，避免错误配置静默放行。每轮刷新规则，子任务沿用父项目规则来源，隔离 child 仍按隔离目录的相对路径匹配。
- 文件：复用 canonical/in_workspace 规范化实际路径，Windows 匹配大小写及分隔符；拒绝配置中的 ../ 和绝对路径。read 的 deny/ask 同时约束 grep/ls 和 edit/write 的读取侧，edit 规则也覆盖 write。grep/ls 对实际文件逐项过滤 deny，链接/越界条目不读取；对可能触及 ask 文件的搜索/列表保守要求整次确认，确认后仍过滤 deny。不能通过 `sub/../.env`、搜索目录或编辑预览读出被拒绝内容。
- Shell：识别引号/转义和 ;、&&、||、管道的命令段，每段独立匹配。前缀按解析后的完整 token 比较，npm test 不匹配 npm testing，带引号的 rm/-rf 仍命中 deny。PowerShell 大小写及 Windows 可执行文件大小写兼容。变量/命令替换、重定向、脚本块等未能可靠解释的结构要求单次审批；存在 shell deny 时无法确认未绕过该 deny 的结构及常见解释器/包装命令直接拒绝。规则是执行授权策略，不声称能证明任意脚本或包管理器内部代码的行为；OS 文件和网络边界在 SEC2 实现。
- 审批：授权请求携带由运行时计算的 grant_rule；本会话只记住具体命令的执行文件/子命令 token 前缀或具体文件路径，不记住整个 bash/powershell。旧整工具 shell grant 忽略；原生非 shell 旧 grant 保持兼容。MCP 默认为 ask（包括 bypass），仅明确匹配 allow 或确认后记住的精确 MCP 工具可放行。workspace_trust、review_delegate 合并以及后续危险操作不产生可记住规则。显式 deny 在 preflight 直接失败，不创建可批准的请求。
- 界面与持久化：TUI、Desktop、原生 CLI 提供仅本次、会话规则、写入项目规则；同时展示具体范围。无可靠范围时只提供单次确认，不提示可永久放行。复用既有 ask/resolveApproval 确认通道和 atomic_write 替换，新项目规则写入 settings.local.json，经 fs2 进程间事务锁合并并去重；保存失败保留待审批状态，不授予未保存的权限。R26 首次提取 settings_file 公用事务入口，C2 后续复用到服务配置。
- 回归：5 项 Bridge 场景覆盖分层 deny、搜索/路径归一化、会话前缀不放行另一命令、复合段 deny、项目范围保存和默认 bypass 的 MCP 单次/会话授权；3 项 Rust 场景覆盖完整 token、所有复合分隔符、引号、保守语法、路径 glob、ask/deny 优先和四层合并。TUI 实际快捷键 P 发送 project 范围，Desktop 真实控件显示单次/具体会话范围/项目选项。旧会话缓存断言改为具体 write(cache.txt)；本地 MCP 传输测试显式授权 mock 服务，不再依赖旧 bypass 漏洞，权限测试不调用 mock 授权 helper。
- 验证过程：初次范围变更的全测发现 MCP 图像测试缺少显式 mock 授权、PowerShell 方法调用测试现在需要确认，已按新语义更新；日志保留 `output/6-R26-second-protocol.log`。首次并行 Desktop 全测还出现一次未修改历史分页用例的时序失败，需在最终完整检查复核，未宣称修复其时序根因。新增中文界面文案已补英语目录。
- 最终复核补充：拒绝 read 同时阻止 edit/write 预览侧读取，带路径的 rm 可执行文件及加引号 token 不绕过相应 deny；不能安全表示成规则的引用参数不生成可记住前缀。PowerShell 的相邻引号、反引号及 stop-parsing 形式采用单次确认。路径越界错误恢复为原工具失败回执，避免 preflight 新校验把原有可恢复工具错误升级成 run 异常。窄屏测试已扩展为四选项并验证环绕选择项目/拒绝；聚焦 TUI 13 项及 Desktop 权限/历史/语言测试通过。
- 已有波动记录：并行全测中原 concurrent_edits_of_one_file_are_serialized 再次出现一次 Windows canonical 临时名称导致读取 ledger 未命中；保留 `output/6-R26-concurrent-edit-flake.log`，聚焦重跑及后续完整 Rust 检查通过。相关编辑、替换及 canonical 实现未在 R26 修改，不声称已修复该既有时序问题。
- 最终验证：`output/check-5.ps1 -Item R26-final-code -Desktop` 全部通过，日志前缀 `output/5-R26-final-code-`：fmt、Clippy、Rust 207 通过/2 个既有忽略、build、协议 274 通过/2 个既有跳过、生成文件无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 122 通过、Desktop test/vet/vendor UI。Desktop Bridge 集成的旧整工具 write 断言同步为 write(verified.txt)，最终全测已覆盖。仅 Windows 实机执行，Linux/macOS 分支需对应平台 CI 验证。

### SEC2 OS Shell 沙箱与不可记住的高危确认

- 核查：原 shell 执行器没有 OS 隔离；启用配置后用 PowerShell 写入工作区外文件的回归先失败，见 `output/6-SEC2-red.log`。R26 已区分 deny/ask，本项在普通授权之后增加高危单次确认，deny 在 preflight 直接拒绝，不进入一般或高危审批。
- 高危确认：识别 rm 的递归+强制组合、Windows 递归强制删除、git push 的 force/force-with-lease/-f、常见下载管道直接执行。命令是否由 bypass、allow 或会话前缀放行都不影响单独确认；dangerous_command 不产生可记住范围，恶意 remember=session/project 也不能保存该确认。静态检测不声称覆盖任意解释器内部所有行为。存在文件 deny 的工作区为保证任意脚本不能绕过，保守拒绝所有 Shell，原生文件工具继续按路径工作；不把文件规则错误地描述成内核过滤器。
- 配置：三层 settings 文件增加 shellSandbox.enabled/network/readOnlyPaths；原生 CLI --sandbox/--no-sandbox 可临时覆盖普通父命令，默认关闭。显式 writable 子任务强制进入沙箱路径，不能用父级关闭选项绕过；无可用后端时只返回失败。内置 test-runner/fixer/ui-designer 增加 powershell 工具，供 Windows 受限进程使用。配置、各平台边界和完整示例见 `docs/permissions.md`，README 与原权限 ADR 指向新行为。
- Windows：使用原生 CreateAppContainerProfile + SECURITY_CAPABILITIES + CreateProcessW，零网络 capability。仅给随机 package SID 授予工作区/私有子临时目录读写权限及明确 readOnlyPaths 的读执行权限；普通用户目录及网络由 AppContainer 访问检查阻止。ACL 修改使用按路径命名 Mutex 的跨进程事务，撤销只移除本次随机 SID，保留原有/其他任务权限。仅支持 network=false，true 明确拒绝；Git Bash/MSYS 实测因全局命名对象被拒绝而不能启动，明确要求改用 powershell，不扩大命名空间权限或退回宿主 Shell。
- 启动与收尾：宿主先把可信 helper 纳入原有 Job Object，再发私有 ready 信号；helper 在接到信号前不创建子 Shell，避免 attach 与后代创建之间的竞态。真正受限进程先以 suspended 创建，在沙箱不可访问的监督目录写成功标记后再恢复线程；只有成功标记才记录 shell_sandbox.enforced=true。进程树、取消、超时、后台任务统一走 R3/R41 清理，沙箱任务结束时回收后代，不释放脱离控制的后台子进程。正常收尾撤销 ACL、删除 profile；撤销或删除失败明确返回错误，Drop 继续尽力清理。全宿主硬杀仍可能留下无活动进程的随机 profile/ACE，需要后续启动恢复清理作为后续维护事项，不能称其为已完成的跨重启 profile 管理。
- Linux/macOS：Linux 由固定系统位置的 bubblewrap 创建新的 mount/pid/network 等 namespace，系统运行库只读，仅工作区和私有临时目录可写；macOS 由固定系统位置的 sandbox-exec 使用 deny-default Seatbelt profile。可配置额外只读工具链/缓存；网络默认关闭。清除任意继承环境，只保留系统/locale/PATH 等运行必要项和私有 HOME/TEMP，避免把 API 凭据带入命令。不得从工作区 PATH 找到一个同名假沙箱程序。缺少程序、namespace/profile 不可用或启动失败都不回退无隔离执行；Unix 启动握手无法验证时报告副作用未知，不宣称命令从未开始。
- 回归：9 项真实 Bridge 场景覆盖 bypass+allow 的独立高危审批、deny 无审批、危险确认不能记住、内写/外写拒绝、外部私有文件读取拒绝、真实 TCP 服务零连接、环境密钥未继承、Windows 不支持网络配置的真实保护状态、隔离 child 自动受限及 idle 后台取消后实际 PID 已退出。3 项 Rust 场景覆盖危险形式、Windows 参数引用、CreateProcess 失败不能产生启动标记。Windows 文件/网络/后台及子代理场景已实际执行，不依赖字符串过滤冒充 OS 边界。
- 最终验证：`output/check-5.ps1 -Item SEC2-complete -Desktop` 全部通过，日志前缀 `output/5-SEC2-complete-`：fmt、Clippy、Rust 210 通过/2 个既有忽略、build、协议 283 通过/2 个既有跳过、生成文件无差异、60 次主任务和 9 个场景保持预期、npm ci/build、TUI 122 通过、Desktop test/vet/vendor UI。Windows AppContainer 启动、访问检查、零 TCP 连接、私有环境和实际进程回收均已在本机执行；Linux/macOS 后端代码尚未在对应系统运行，不标记为已验证。

### C1 API Key 系统安全存储与可重试迁移

- 核查：Configuration.write/save_service 直接序列化 apiKey，read 不迁移。新保存和旧配置两个失败回归见 `output/6-C1-red.log`。
- 实现：新增 credentials 统一入口，Windows 用当前用户 DPAPI 加密独立 blob，保存在配置旁同名 `.credentials` 目录；不使用 machine scope。macOS 原生 Security.framework generic-password Keychain API，Linux 固定 `/usr/bin/secret-tool` 通过 libsecret/Secret Service，密钥仅经 stdin，15 秒超时并回收子进程。没有明文、环境变量或测试专用存储回退，不新增 crate；只开启已有 windows-sys 的 Cryptography feature。Linux CI 配置真实临时 D-Bus/keyring 并先回读探测；其启动脚本尚未在本机执行。
- 配置：仅保存随机不可变 credentialRef；public/getConfiguration 删除引用并保留兼容的空 apiKey/hasApiKey。新建/替换均先存储再回读逐字验证；失败保留旧配置和旧引用。read 对旧配置执行锁内迁移，所有 Key 准备成功才原子替换配置，重试不反复更换已迁移引用；BOM 仍兼容。不可变引用保证保存失败不会覆盖旧 Key，但失败或替换可能遗留安全存储条目，暂不自动回收，避免误删仍被旧配置引用的凭据。
- 边界：普通服务保存和连接探测忽略客户端提供的 credentialRef，只可沿用服务 ID 对应的已有引用；空 Key 编辑保留已有 Key。系统存储不可用、blob 损坏或引用非法明确报错，不静默改用其他凭据。错误只含操作状态，不回显密钥或底层 stderr。第三方自动发现将在下一条 C3 单独移除，未混入本次变更。
- 回归：新配置无明文/引用不出 UI、迁移后可取回原 Key、重复迁移无变化、真实 DPAPI 失败后逐字保留原文件并可重试、进程重启后认证、空编辑保留/换 Key 生效、伪造引用拒绝、损坏 blob 拒绝、事件/错误无测试密钥。聚焦成功见 `output/6-C1-focused.log`；平台实际执行仅 Windows，macOS/Linux 需对应系统验证。Keychain/Secret Service 的解锁和授权交互由系统负责，移动配置文件不保证引用可移植。
- 原生 API 依据：[DPAPI](https://learn.microsoft.com/en-us/windows/win32/seccrypto/example-c-program-using-cryptprotectdata)、[Apple Keychain](https://developer.apple.com/documentation/security/seckeychainfindgenericpassword(_:_:_:_:_:_:_:_:))、[libsecret secret-tool](https://manpages.ubuntu.com/manpages/noble/man1/secret-tool.1.html)。
- 完整验证：`output/check-5.ps1 -Item C1 -Desktop` 全部通过，日志前缀 `output/5-C1-`：fmt、Clippy、Rust 212 通过/2 个既有忽略、build、协议 284 通过/2 个既有跳过、生成文件无差异、60 次任务和 9 个场景保持预期、npm ci/build、TUI 122 通过、Desktop test/vet/vendor UI。随后新增的 C2 回归单独记录在 C2 日志，未计入 C1 通过结果。

### C2 配置事务、版本冲突与原子提交

- 核查：C1 的迁移已有锁，但普通保存仍在锁外读改写，save_service/deleteService 各可写两次。两个真实 Bridge 并发新增触发 Windows replacement error 1176，旧草稿无条件覆盖；另两项 Rust 回归确认 write 不等待锁、过期快照仍成功，失败证据为 `output/6-C2-red.log`、`output/6-C2-rust-red.log`。
- 修复：扩展 R26 settings_file 的跨进程锁，canonical 路径统一锁文件，拒绝链接锁。Configuration.update 将读取、校验、修改、凭据准备、版本递增、序列化和 atomic_write 放在同一排他锁内；服务/代理新增编辑、删除、默认模型、启停和本地资源状态均使用该事务，服务默认模型修复同次提交。权限规则原有 update 与插件信任库使用同一通用锁语义，信任不同工作区不再覆盖整份旧快照。无新增依赖。
- 冲突：配置 version 在一次事务成功后递增；直接 write 是按读到的 version 做 compare-and-swap，过期整份快照明确拒绝。服务和自定义代理使用各自 revision，编辑草稿必须回传；不同服务可独立提交，同一旧草稿、删除后的草稿、编辑期间被禁用的代理都不会覆盖新状态。Desktop 类型和操作保留 revision，冲突保留编辑内容并提示刷新；旧无版本配置视为 0，可正常迁移，旧客户端若丢弃新返回的 revision，后续整项编辑会被明确拒绝，避免盲写。按名称启停/按 ID 删除仍兼容无 revision 的显式动作，升级客户端会回传 revision 检测过期动作。
- 文件语义：继续使用既有同目录临时文件、内容 sync、Windows ReplaceFile/MoveFileEx 和 Unix rename+父目录 sync。错误或 panic 不提交并由 RAII 释放锁；不自动采用遗留临时文件。版本化整份替换允许修复 JSON 语法有效但字段无效的文档；普通字段事务拒绝当前无效配置，避免静默抹除内容。首轮全测发现原 r27 修复配置用例被过严的旧值校验阻止，已保留此恢复路径并聚焦验证成功，记录在 `output/5-C2-test.log`、`output/6-C2-repair-focused.log`。
- 回归：两个进程同时新增 12 服务全部保留且恰好 12 次版本递增；旧服务/代理编辑、并发禁用、删除后重建冲突；直接旧快照 CAS、真实锁等待、超限失败保留原文件、事务拒绝/panic 后字节不变并可继续写入。聚焦通过见 `output/6-C2-focused.log`、`output/6-C2-protocol-focused.log`；Windows 实机执行，Unix 依靠对应平台 CI 验证。
- 完整验证：`output/check-5.ps1 -Item C2-complete -Desktop` 全部通过，日志前缀 `output/5-C2-complete-`：fmt、Clippy、Rust 216 通过/2 个既有忽略（212 单元+4 事务集成）、build、协议 287 通过/2 个既有跳过、生成文件无差异、60 次任务和 9 个场景保持预期、npm ci/build、TUI 122 通过、Desktop test/vet/vendor UI。C3 的独立失败测试随后单独记录，未计入本次通过结果。

### C3 第三方凭据仅在用户显式确认后导入

- 核查：discover_commandcode 除显式环境变量外，会在 provider 初始化时读取 `.zcode/v2/provider_config.json`，并可回退选取其他 OpenAI-compatible 服务。真实 Bridge 回归在放置第三方配置后确认 setTuiProvider 无需授权即可成功；缺失显式导入 API/CLI，Desktop 无导入入口。失败证据为 `output/6-C3-red.log`、`output/6-C3-desktop-red.log`。
- 移除：默认凭据入口改名 commandcode_environment，仅检查用户显式设置的环境变量，不再 stat/open/read 第三方文件。全 agent 搜索第三方路径仅余导入来源构造和 CLI 帮助说明；Catalog/普通配置/provider 选择不走导入方法。环境变量仍只在内存使用，不自动复制到配置。
- 产品流程：Desktop“模型与预算 → 从 Command Code 导入”先调用 getCredentialImportInfo 显示来源、将读取的接口地址/API Key、系统安全存储方式和源文件保持不变的说明；该接口只构造说明，不访问文件。确认按钮才调用 importCommandCode(confirmed=true)，取消不调用；失败保留说明并显示错误，成功提示添加模型 ID，按钮在请求期间禁用。CLI `minicode config import-commandcode` 先输出同样说明，再询问 y/N；EOF/否定取消，`--yes` 表示脚本已明确同意，仍输出说明。TUI 用户可在终端使用该命令，不增加另一套凭据入口。
- 导入边界：确认检查在文件路径和文件操作之前，仅读取固定来源的普通文件并限制 1 MB、兼容 BOM；仅选 providerName=Command Code 且 OpenAI chat 类型，不取其他提供商作回退。复用 C1/C2 save_service 做端点校验、系统存储写入/回读和一次原子事务，返回值仅含空 apiKey/hasApiKey，不暴露引用或密钥。源文件从不修改，导入的模型列表为空，用户之后自行添加模型，避免错误推断第三方模型元数据。
- 回归：普通配置/provider 不使用第三方 Key、显式环境变量保持可用、来源文件不存在时仍能展示说明且拒绝授权不访问/保存、授权导入仅保存安全引用、源文件不变、CLI 拒绝/同意、拒绝无关提供商和畸形敏感 JSON、Desktop 展示说明→取消→再次确认流程。聚焦 Node 5 通过/1 个 Unix-only FIFO 跳过（`output/6-C3-focused.log`）、Desktop 聚焦通过（`output/6-C3-desktop-focused.log`）。Unix FIFO 回归通过阻塞文件检测未授权 open/read，本机 Windows 不支持此用例，留给 Linux CI，不能标为实测通过。Windows 来源路径分隔符的聚焦断言首次失败后已在产品路径构造中统一，最终聚焦通过。
- 最终完整验证：`output/check-5.ps1 -Item C3-final -Desktop` 全部通过，日志前缀 `output/5-C3-final-`：fmt、Clippy、Rust 216 通过/2 个既有忽略（212 单元+4 配置事务集成）、build、协议 292 通过/3 个跳过、生成文件无差异、60 次主任务与 9 个场景保持预期、npm ci/build、TUI 122 通过、Desktop test/vet/vendor UI。三个协议跳过分别为本机不适用的 Unix FIFO/Unix MCP 进程组，以及既有 200 秒长流可选测试；Rust 忽略项仍为两个手动性能基准。导入界面预览 `output/native/credential-import-dark-1x.png` 已实际查看，来源/读取字段/安全存储说明和确认/取消按钮均可见，无裁切。最终检查包含前面 16 项的所有适用回归。

## 阶段 5–6 最终验收报告

基线 `52fcea1`，主体实现提交 `0bca0c7`；完成审计另将 C2 四个 Rust 回归移入配置模块，断言逐字保留。17 项均已修复，无跳过项、部分完成项或阻塞项；跨平台未执行的验证单独列出，不计作通过。共 17 个条目提交，另有 R3/R6 两个生命周期复核提交及 C2 测试布局复核提交；本报告为独立文档提交。未推送远端，原有两个 `.pending` 文件保留。

下表中 Rust 文件相对 `agent/`，协议测试相对 `tests/agent/`；客户端文件使用仓库相对路径。各条目上文包含核查根因、先失败证据、实现、完整检查日志和兼容性记录。

### 5A 状态、实现与提交

| 条目 | 状态 | 提交 | 主要实现与回归证据 |
| --- | --- | --- | --- |
| R3 | 已修复 | `676bdb5`、`8509e25` | `background.rs`、`runtime.rs`、`bridge.rs`、`cli.rs`：会话级任务、增量输出、终止、持久通知与确定清理；`background.test.mjs` 和 Rust 生命周期用例覆盖跨回合 dev server、重复终止、EOF/取消及无效 continue。 |
| R6 | 已修复 | `a83aa27`、`d4393e9` | `mcp.rs`、`mcp_dispatch.rs`、`runtime.rs`、`bridge.rs`：会话复用、增量重连、独立 reader/demux、资源 owner 和 SessionLock；`mcp-lifecycle.test.mjs`、`session-cache.test.mjs` 覆盖五轮复用、并发、七 owner 和跨进程删除保护。 |
| EXT1 | 已修复 | `f5fea26` | `extension_cache.rs`、`extensions.rs`：32 项共享 Catalog 缓存和文件/目录/缺失路径依赖失效；模块 Rust 测试通过计数及新增/修改/删除/链接验证。 |
| EXT2 | 已修复 | `650e1a7` | 同上：128 项元数据依赖的指纹缓存，嵌套变化失效、删除及链接拒绝；模块 Rust 测试验证重复调用仅哈希一次和改变后的拒绝。 |
| MC4 | 已修复 | `44aaf90` | `mcp.rs`、`mcp_dispatch.rs`：响应半帧持续读取，请求写中断使连接失效并重建；`mcp-recovery.test.mjs` 和 Rust 取消用例覆盖超时、迟到帧、损坏帧及后续请求。 |
| MC5 | 已修复 | `786f389` | 同上及 Cargo serde_json feature：按原始十进制文本归一化 ID，避免 f64 舍入；`mcp-ids.test.mjs` 与模块测试覆盖字符串/整数/小数/指数及无效、溢出、重复 ID。 |

5A 完整阶段复核日志为 `output/5-5A-final-*`，其后所有阶段完整检查继续包含上述回归。

### 5B 状态、实现与提交

| 条目 | 状态 | 提交 | 主要实现与回归证据 |
| --- | --- | --- | --- |
| R31 | 已修复 | `a46a37a` | `subagent_result.rs`、`runtime.rs`、工具定义、`store.rs`：child 专用 submit_result、结构校验和裸/围栏/周边文本 JSON 回退；`subagent-results.test.mjs` 覆盖成功、非法和恢复幂等。 |
| R31b | 已修复 | `3feef9c` | `tools.rs`、`runtime.rs`、`models.rs`、`subagent_result.rs`：仅以真实原生文件工具结果记录观察路径；`subagent-evidence.test.mjs` 覆盖 grep 实际命中、相对路径/位置后缀及伪造引用拒绝。 |
| R33 | 已修复 | `3ecb720` | `runtime.rs`、`tui/src/child-progress.ts`/ToolLine、Desktop feed/state/child_progress：父回合转发安全摘要、排序去重及折叠；协议 `subagent-progress.test.mjs`、TUI 和 Desktop 专项测试。 |
| R35 | 已修复 | `1ba52c4` | `runtime.rs`、`mcp.rs`、`configuration.rs`：借用父 Catalog/获准 MCP 连接、独立所有权、真实用户 requirements 上限 16,384 UTF-8 字节；`mcp-lifecycle.test.mjs`、`audit.test.mjs` 和 Rust 测试验证权限不扩大及合成消息排除。 |
| R43 | 已修复 | `edf0d34` | `runtime.rs`：实际新建/恢复 child 统一继承父 compaction；Rust 串行开关/恢复和真实 HTTP 双 child 并行测试覆盖 true/false。 |
| A4 | 已修复 | `b610b72` | `delegate_workspace.rs`、`runtime.rs`、`store.rs`、工具/代理定义：默认只读、隔离副本、持久 diff 审阅、独立批准、冲突预检、失败回滚/取消清理；`subagent-isolation.test.mjs` 和 Rust 文件系统测试。Shell 的实际 OS 隔离由 SEC2 补齐。 |

5B 各条目完整日志前缀为 `output/5-R31-*`、`5-R31b-*`、`5-R33-*`、`5-R35-*`、`5-R43-*`、`5-A4-*`；最终全套继续覆盖。

### 阶段 6 状态、实现与提交

| 条目 | 状态 | 提交 | 主要实现与回归证据 |
| --- | --- | --- | --- |
| R26 | 已修复 | `49ae077` | `permissions.rs`、`settings_file.rs`、runtime/tools/bridge/CLI 与 TUI/Desktop 审批：分层规则、deny 优先、命令/文件/MCP 范围、保守复合命令、单次/会话/项目授权；`permissions.test.mjs`、模块及客户端审批测试。 |
| SEC2 | 已修复 | `21aad6e` | `sandbox.rs`、`sandbox_windows.rs`、runtime/tools/CLI：真实 OS 沙箱、强制隔离可写 child、独立不可记住的高危确认；`sandbox.test.mjs` 的 Windows 实机文件、TCP、环境、后台 PID 和 child 测试全部通过。 |
| C1 | 已修复 | `432a593` | `credentials.rs`、`configuration.rs`、provider/bridge、Linux CI 启动脚本：OS 凭据存储、回读验证后迁移、无明文回退；Rust 迁移/失败保留测试和 `credentials.test.mjs` 的重启认证、换 Key、伪造引用/损坏拒绝。 |
| C2 | 已修复 | `45b2cf5`、`1f7ff32` | `configuration.rs`、`settings_file.rs`、bridge/extensions、Desktop 类型/编辑：完整事务锁、version CAS、条目 revision、一次写入；`configuration.rs::tests` 和 `config-transactions.test.mjs` 覆盖锁等待、失败/panic 恢复和两进程保留全部修改。测试布局复核见下文。 |
| C3 | 已修复 | `0bca0c7` | configuration/provider/bridge/CLI、Desktop settings/fixture/localization：删除自动第三方读取，先披露后确认，统一安全存储；`credential-import.test.mjs`、`desktop/native/credential_import_test.go`，并已查看实际界面预览。 |

### 最终测试与评测证据

完成审计后的最终全套命令为 `output/check-5.ps1 -Item completion-audit -Desktop`，另执行 CI 的 `desktop/native/build.ps1`。对应 `output/5-completion-audit-*.log` 保留完整输出，没有真实模型 API 调用；协议/HTTP/MCP 均使用本地受控 mock。原 `C3-final` 完整结果仍保留，只有 Rust 测试位置发生调整。

| 检查 | 最终结果 |
| --- | --- |
| Rust fmt / Clippy `--locked --all-targets -- -D warnings` / build | 全通过 |
| Rust `cargo test --locked` | 216 单元通过（含移入模块的 4 个配置事务回归）；2 个既有手动基准忽略 |
| Node 全协议 | 292 通过、0 失败、3 跳过：Unix FIFO、Unix MCP 进程组、本就可选的 200 秒长流 |
| 任务生成 / trace / Git 生成文件差异 | 通过，无生成文件差异 |
| b0/b1/b2 主任务和场景 | 60 次主任务、9 次场景符合预期验收结果；`verify-results.mjs` 通过 |
| TUI npm ci / build / test，启用真实 Rust Bridge 测试 | 全通过；122 个测试通过 |
| Desktop go test / vet / vendor UI / Windows executable build | 全通过；包含子任务进度、审批范围、配置编辑和显式导入；已生成 `output/native/MiniCode.exe` |

`counter_retry` 的 b0/b1 按评测定义失败、b2 成功，这是 verifier 接受的预期结果，不能把“预期验收结果保持一致”改述为所有 baseline 场景均成功。首轮 C2 的 r27 修复配置路径回归已修复并通过最终全测。历史 Windows 并发原子编辑波动及一次 Desktop 分页时序波动的失败/复跑记录保留在对应条目，没有将它们声称为本轮已修复的独立问题。

#### 完成审计补充：C2 测试布局

- 重新读取原始任务后发现四个 C2 Rust 回归原先位于 `tests/configuration_transactions.rs`，不符合任务规定的 `agent/tests.rs` 或模块 `#[cfg(test)]` 布局。已将四个测试完整移入 `agent/configuration.rs::tests` 并删除原独立文件；逐字比对确认测试体和断言不变，仅导入改为 `super::Configuration`，运行时实现不变。
- 四项模块聚焦回归通过（`output/6-C2-audit-focused.log`）。随后完整 `completion-audit` 检查通过：Rust 216 单元/2 个既有忽略、协议 292 通过/3 个既有条件跳过、60 次任务和 9 个场景符合预期、TUI 122 通过、Desktop test/vet/vendor UI 通过；额外执行 CI 的 Windows executable build 成功，输出 `output/native/MiniCode.exe`。
- 已核对 17 项的对应源代码、测试断言、独立提交和检查证据；各条目均保留完整检查日志。R26 最终日志采用 `R26-final-code`，避免把中间 `R26-verified` 缺少的 Desktop vet/UI 日志误认为最终证据。最终源代码之后的报告更新仅修改文档，不增加未验证实现。

### 兼容性、安全边界与回滚

- 会话/协议：保留既有 NDJSON 协议版本和旧会话读取，新工具/事件/关联字段均增量加入；老历史缺少 child call_id 时走兼容展示。后台任务只在同一进程的会话间保留，不按旧 PID 跨重启接管进程。
- 子代理：默认只读；显式 writable 才创建隔离快照，保留父工作区原有脏修改。inspect 后再独立批准才能合并，冲突先拒绝，应用失败仅回滚自身写入并报告外部冲突。MCP readOnlyHint 是已信任服务的声明，不代表远程工具受本地内核沙箱控制。
- 权限/沙箱：deny 不能被批准覆盖。高危确认不可记住，静态识别不声称能理解任意解释器；有文件 deny 时保守阻止任意 Shell。普通父 Shell 沙箱默认关闭、可显式启用，可写 child 强制开启；不可用则拒绝，不回退无隔离。Windows 仅 network=false，Git Bash/MSYS 在 AppContainer 不可启动时要求改用 PowerShell；Linux/macOS 行为详见 permissions 文档。
- 配置：旧无 version/revision 文档视为 0；新客户端需保留返回的 revision，过期整项编辑明确冲突。纯字段动作在锁内修改当前值，不覆盖整份旧配置；JSON 有效但字段无效时可用带版本的完整替换修复。原子提交失败保留旧文件，不把残留临时文件当成恢复来源。
- 凭据：明文迁移先写安全存储并回读验证，所有项准备完毕才替换配置；失败保留原字节并明确报错。配置备份只含引用，不能当作可移植 Key 备份。保留旧引用对应的安全条目，避免保存失败时破坏仍有效凭据。第三方导入必须显式确认，原文件不修改，导入不推测模型 ID。

### 剩余风险、保守决策与后续建议

1. 本机实际验证为 Windows。Linux 的 bubblewrap/Secret Service、CI 临时 D-Bus/keyring 启动、Unix FIFO/进程组，以及 macOS Seatbelt/Keychain 未在对应平台执行，仍需平台 CI/实机覆盖；macOS sandbox-exec 已被系统弃用，不将未来可用性视为保证。优先补 macOS 测试和 Linux 沙箱/凭据 CI 实测。
2. Windows 宿主整体硬杀可能留下没有活动进程的随机 AppContainer profile/ACE；正常结束和取消已清理，Job Object 回收活动进程。后续优先做启动恢复时按所有权清理残留 profile/ACL，而非扩大沙箱权限。
3. 安全凭据条目在失败保存/替换后可能成为孤立条目；本轮选择保留，避免误删旧配置仍引用的凭据。后续实现引用感知、可审阅的凭据清理和受控备份/迁移，不提供明文回退。
4. 继续跟踪既有 Windows 并发文件替换和 Desktop 分页负载时序波动，建立确定性压力回归；无需修改本轮已经通过的权限/凭据协议来掩盖它们。持有活资源的 owner 可超过普通六 owner 缓存上限，后续可增加显式资源预算和状态展示。

上述保守决策均已按任务的安全默认值实施，不需要额外阻塞当前交付；本轮没有部署、远端推送或真实第三方凭据导入。

### TU3 — use monotonic streaming IDs

问题/根因：feed.ts::upsertAssistant 以数组长度构造 key，清屏/截断后复用。修复：进程内单调计数，delta 保留同一条目 ID。回归：stage7-feed.test.ts 截断、多轮及追加；先失败后通过。状态：已修复。


### TU4 — omit empty narrow table headings

问题/根因：markdown.tsx::table 无条件输出冒号。修复：缺失/空表头只显示内容。回归：markdown.test.tsx 用超出表头数量的单元格触发窄布局，先失败后通过。状态：已修复。


### T-INK — clear with controlled terminal output

核查：terminal.ts::clearTranscript 动态导入 Ink instances.js 并修改五个私有状态。修复：受控 ANSI 清屏序列，无私有导入/状态变更。当前视口无 Static 缓存，后续滚屏实现须保持此约束。回归：terminal.test.ts 无 Ink renderer 的流先失败后通过。状态：已修复。


### T-UI9 — stream sealed transcript to native scrollback

核查：App/TranscriptViewport 全量 Yoga 树与备用屏、鼠标捕获导致 O(N) 重绘。修复：默认启动器用 TranscriptLog，公开 renderToString/useStdout.write 一次输出完成条目，无 Static 的私有缓存重放；实时视口限制未完成条目数，嵌入 App 保留原视口模式。终端默认仅 bracketed paste，不捕获鼠标、不进备用屏。Desktop 已有 AppendFeed 与 ui.List，重建仅结构变化，未重复重写。回归：1200 条目一次渲染，10 次更新计数不变；terminal.test.ts 输入模式。基准：transcript-bench.tsx Windows/Node22，1200条/20次，102.05ms→0.20ms 活动树；只衡量 Ink 树渲染，不包含网络和首次回放。状态：已修复（原生终端人工选择/复制仍需人工验收）。


### T-UI12 — display collapsible reasoning and streamed usage

核查：feed.ts 忽略 reasoning_delta；Provider 只在结束统计 usage。修复：独立 reasoning 状态，Ctrl+R 折叠/展开末尾8行，保持 transcript 身份；Provider 在可用时发可选 usage_delta，Spinner 使用实际输出 tokens，缺失时回退。回归：stage7-feed.test.ts 仅 reasoning、多 delta、新 run 重置，先失败后通过。状态：部分完成（尚需高频批处理及真实 SSE/可折叠 UI 集成回归）。


### TU5 — read complete tool artifacts by page

核查：ToolLine 展开仅80行，artifactId 未使用。修复：Ctrl+F 完整输出查看器，调用现有 readArtifact；每次替换一页，按 Unicode 字符计算 offset，n/p 翻页、上下滚动、Esc关闭、r重试，失败保留当前页。回归：artifact-pages.test.ts Unicode分页/末页/后退/不存在错误，先失败后通过。状态：部分完成（超长单行的水平查看和真实桥接 UI 集成尚需补齐）。


### T-UI6 — add history file completion and folded paste

核查：无历史/文件候选/附件导入 UI，paste 直接进草稿。修复：首末行上下历史/草稿恢复，100ms防抖共享 listFiles，Tab 带引号路径；/attach 路径调用 importAttachments；长文本折叠 [paste:N]，/paste N 预览，/paste N cancel 取消，发送展开。回归：input-history.test.ts 历史/Unicode/10000行无损与取消；file-completion.test.mjs 共享接口、空格Unicode与ignore，均先失败。状态：部分完成（历史持久化、图片剪贴板自动提取、候选上下选择和附件队列隔离待验收）。


### TU7 — prioritize escape cancellation and message editing

核查：Esc 仅审批/菜单，无运行取消和编辑。修复：审批/菜单优先，运行Esc只取消一次，空闲400ms双Esc复制上一用户消息到草稿，重发追加历史；取消中第二次Ctrl+C退出。回归：stage7-keys.test.tsx Esc两次仅一次取消、完成后双Esc不自动重发。状态：已修复。App.tsx 同时含 T-UI6 输入接线与 TU7，提交中明确此直接依赖。


### TU8 — configure status fields with safe fallbacks

核查：StatusLine 固定模型/effort/context。修复：MINICODE_STATUS_FIELDS 逗号配置 model,effort,context,turns,budget,cost,branch,elapsed，默认保留既有三字段，缺失费用/分支明确不可用，窄屏按显示宽度截断。统计复用AgentState，耗时复用run开始时间。回归：status-fields.test.ts 缺失、未知字段与自定义组合，先失败后通过。状态：已修复（当前服务端无定价则费用不可用，不推算虚构价格）。


### B6 — persist steering and follow-up queues

核查：sendPrompt忙时拒绝，无服务端队列。修复：可选messageQueue能力；enqueuePrompt/listQueue/deleteQueuedPrompt/resumeQueue，最多100条；接受/消费/删除持久化事件。steer在工具结果后原子提交用户消息+queue_consumed；正常结束自动链式followUp，无工具边界的steer转下一run。每run独立ID；取消/失败暂停保留队列。旧sendPrompt拒绝忙时行为保留。回归：queue.test.mjs 真实Bridge审批边界、消费顺序/一次执行、followUp自动启动、取消保留与删除，先失败后通过。状态：部分完成（跨进程恢复与删除/消费并发测试需补齐）。

