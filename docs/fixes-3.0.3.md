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
