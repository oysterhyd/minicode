# Harness 核心重构审计（2026-09-30）

这次重构针对 CLI、TUI 和 desktop 共用的执行核心。验收重点是可恢复的执行语义、明确的权限与副作用边界、稳定的扩展生命周期以及可并发控制的宿主接口。原有工作区路径门、验收器、任务图、压缩与归档机制继续使用。

## 审查发现与修复

| 旧实现的问题 | 当前行为 | 实现位置 |
| --- | --- | --- |
| CLI chat 每轮 `asyncio.run`，回合之间后台 watcher 被取消 | 一个 REPL 使用一个事件循环；异步终端输入期间后台命令继续工作；Ctrl+C 取消当前激活 | `runtime/host.py`、`cli.py` |
| 单会话缺少明确的运行所有权 | runtime 拒绝重入，SQLite 会话使用 OS 文件锁；不同 CLI/desktop 进程不能同时激活同一会话；进程退出后锁由 OS 释放 | `storage/ownership.py`、`loop.run_turn/resume` |
| 助手消息、事件、用量分开提交，崩溃可能只留下其中一部分 | 消息、用量与 ASSISTANT_MESSAGE 同一个 WAL/FULL 事务提交；终态与 SESSION_END 同样原子提交 | `SqliteStore.checkpoint`、`EventRecorder.commit` |
| 工具执行结果已记录，但轮次消息还没交付时，恢复可能误报副作用未知 | 工具结果先进入持久 outbox；恢复先回填已知结果，保留原始 call id；只在没有已知结果时执行重放判定 | `tool_results`、`loop._settle_recovery` |
| 已有最终答案但终态未落库，续跑会再付费请求一次模型 | 恢复时先执行原验收门；无验收器或验收通过时直接完成 | `loop._has_unfinalized_answer` |
| 凭工具名称决定并发、重试和安全重放 | 本地工具必须显式声明 `ToolExecution`；未知工具默认串行、单次执行、禁止重放；MCP 的远程注释不能授予这些资格 | `tools/base.py`、`runtime/execution.py`、`scheduling.py` |
| 写入型自定义子助手沿用只读代理的并发和恢复规则 | 父会话内的写入型代理是排序屏障，不能自动重放；只读代理可最多两个并发 | `ToolRegistry.execution_for`、`runtime/delegation.py` |
| 多次续跑的子助手累计用量被重复加入父用量 | 恢复时每个 child session 只计一次；新 SUBAGENT_RESULT 记录本次激活的增量用量 | `runtime/recovery.py`、`delegation.py` |
| SDK 上下文在一个任务打开、另一个任务关闭 | 每个 MCP 连接由专门任务创建、使用和关闭；取消外部调用会终止该连接，并等待资源回收 | `tools/mcp.py` |
| 每轮重新发现 MCP schema | registry 的准备幂等；连接跨激活保留，在重载、驱逐或宿主退出时关闭 | `tools/registry.py` |
| 缺少流式终止标记也可报告完成，重复工具 ID 可能执行两次 | 拒绝无终止信号的 SSE、空助手回复、缺失/重复/历史复用的调用 ID；失败不授权工具执行 | `runtime/streaming.py`、`providers/commandcode.py` |
| UI、provider 或审批回调拿到运行状态的可变引用 | 请求消息、schema、审批参数和事件观察副本彼此隔离；公开 snapshot/usage 不暴露可变内部状态 | `streaming.py`、`execution.py`、`events.py`、`state.py` |
| provider 与 SDK 双层自动重试，重试不可见 | Anthropic SDK 重试关闭；runtime 单层重试并记录 PROVIDER_RETRY；已输出文本后禁止重试；不可修复的 4xx 不重试 | `providers/errors.py`、`streaming.py` |
| 自定义工具可无限等待 | 普通工具默认 300 秒执行超时；Shell 使用自己的命令超时与进程树清理；超时失败明确说明副作用可能不完整 | `ToolExecution`、`runtime/execution.py` |
| 无 ripgrep 时，一次灾难性正则回溯可阻塞搜索线程 | Python 回退使用有单次匹配 timeout 的 regex；超时返回可解释的工具失败 | `tools/search.py` |
| 插件配置写入期间可能出现半份 JSON，清单字段拼错被忽略 | 清单与 lock 原子替换并 fsync；未知字段、链接/junction、超大内容与文件数量被拒绝；调用前继续检查内容指纹 | `plugins.py` |
| 归档原文写入失败可能留下临时或无清单文件 | 归档内容写入后 fsync，再发布清单；失败清理本次临时文件与未发布文件 | `storage/artifacts.py` |
| desktop 顺序读取并等待每条请求，慢网络操作阻塞取消/审批 | v2 NDJSON 并发分派；普通在途请求上限 64，取消/审批保留 8 个控制槽；会话变更串行，慢服务探测独立 | `runtime/protocol.py`、`desktop/bridge.py` |
| desktop 私自修改 runtime 的 registry、预算与消息 | 使用公开快照、模型切换、预算设置、扩展重载与压缩接口；共享 workspace 装配和模型路由 | `runtime/services.py`、`state.py`、`providers/factory.py` |
| CLI 与 desktop 服务/模型/子助手配置分离，auto 无凭据时默默运行演示 | 共用 HarnessConfiguration；配置模型使用服务别名与原始 model ID；无可用服务时失败；显式 fake/script 仍供测试回放 | `configuration.py`、`providers/factory.py`、`cli.py` |

## 当前模块边界

| 模块 | 职责 |
| --- | --- |
| `runtime/loop.py` | 激活、轮次、预算、终止、验收、上下文与后台结果投递 |
| `runtime/streaming.py` | 单次模型请求、协议检查、重试、输出观察 |
| `runtime/execution.py` | 参数校验、权限判定、审批、工具限时执行 |
| `runtime/scheduling.py` | 有界并发、排序屏障、取消后收集已完成结果 |
| `runtime/recovery.py` | 会话恢复、指令/技能版本、父子用量与回放进度恢复 |
| `runtime/delegation.py` | 单层代理、共享预算、父权限、证据引用、child session 续跑 |
| `runtime/services.py` | 跨宿主 workspace、扩展与压缩装配 |
| `runtime/state.py` | 公开、可复制的生命周期快照 |
| `runtime/protocol.py` | 有界并发请求分派与关闭 |
| `tools/mcp.py` | 官方 SDK 的连接所有权与外部调用生命周期 |

## 状态与宿主接口

`snapshot()` 包含 session_id、run_id、phase、running、task_pending、模型、用量、预算、上下文与激活技能。phase 为 idle、preparing、model、tools、approval、finishing、paused。

公开操作为 `run_turn`、`continue_turn`、`set_model`、`set_budget`、`refresh_tools`、`configure_extensions`、`compact_context`、`replace_context`、`aclose`。会覆盖在途请求或消息的变更被拒绝；desktop 把模型变更排到请求边界，把预算和扩展变更排到下一次激活。权限仍对后续工具调用立即生效。

NDJSON 的响应继续用 `{id, result}` / `{id, error}`，事件继续用 `{event, clientKey, sessionId, ...}`。`getState` 增加 `protocolVersion: 2`、`runId`、`phase`、`running`；渲染端类型和重试提示已同步更新。请求 ID 必须是字符串或整数，同一在途 ID 不能重复使用。服务探测、取消和审批不占用会话变更的锁。

## 升级与恢复

- SQLite 自动迁移到 schema v5，新增 `tool_results`，保留历史会话、事件、工件、任务与用量。旧版本读新库会明确拒绝。
- 已有配置仍使用 `~/.minicode/desktop-config.json`，无需搬文件。源码配置实现移动到 `src/minicode/configuration.py`，CLI 和 desktop 共用它。
- builtin catalog ID 继续可用；自定义模型使用 `<服务ID>::<模型ID>` 作为会话身份，向 provider 发送原始模型 ID。切换、恢复和上下文容量使用同一份配置。
- OS 会话锁文件存放在数据库旁的 `.<数据库文件名>.locks`。锁文件保留以避免 inode 替换竞态；文件存在不代表会话仍在运行。进程死亡会释放持有的锁。
- 重试与中断的未知用量仍记为未知，不用零代替。没有 outbox 证据的写入、Shell、外部 MCP 和写入型代理仍不自动重放。
- 安装更新后的 Python 依赖，再重启 desktop。新增直接依赖为 prompt-toolkit、jsonschema、regex。

## 验证

`tests/test_harness_contracts.py` 覆盖事务回滚、助手与工具结果提交后崩溃、跨进程所有权、重入、重复调用 ID、部分流断线、SSE EOF、可变引用隔离、工具限时、灾难性正则、跨激活后台任务、共享模型配置、子助手累计用量，以及慢设置请求期间的取消。MCP 集成测试使用真实本地 stdio server，覆盖不同宿主任务准备/调用/关闭、在途调用取消和重新连接。

完整 Python 回归包含原有预算、压缩、验收、进程树、工作区边界、插件、CLI、TUI 和 desktop 测试；Electron 单测与 TypeScript/Vite 构建也需要通过。结果以本次测试输出为准；本地确定性测试不作为真实模型效果提升的证据。

本机 Windows / Python 3.12 验证结果：完整回归 **562 passed / 3 skipped**；MCP 并发关闭收尾后再次运行相关契约与集成测试 **33 passed**。Electron **27 passed**，TypeScript/Vite 构建通过，真实桥接进程的初始化、配置脱敏、错误隔离及 v2 状态通过。最终 wheel 已在独立虚拟环境安装，从仓库外执行内置 pagination_bounds 的 B0/B2 两次评测，均通过。`git diff --check` 按仓库 CRLF 规则通过。

## 对标与能力边界

参考 [Codex app-server 的请求/通知和线程接口](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)、[Pi 的 agent loop](https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts)、[Claude Code 的子助手与权限隔离](https://code.claude.com/docs/en/sub-agents)。这些资料用于确定核心边界，没有把三个产品的功能集合当作本项目已经具备的能力。

现有 Shell 在本机可信开发环境执行。路径门和审批不提供 OS 沙箱；容器/原生隔离、独立 worktree 写入代理、远程 MCP、外部 hook 脚本、完整 provider 多模态/reasoning 内容，以及真实模型长任务压力评测未在这次实现或验证。同一父会话的写入代理按序执行；不同会话共用工作区时仍需协调文件修改，其权限与副作用由各父会话的规则约束。
