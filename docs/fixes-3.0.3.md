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
