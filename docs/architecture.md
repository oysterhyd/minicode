# MiniCode 2.0 架构

Rust Agent 是 Desktop、CLI 与 Ink TUI 共用的运行核心。Desktop 仍采用 Go / MyGo / Direct3D 11；TUI 仍采用 TypeScript / React / Ink。前端经原有 NDJSON v2 请求和事件协议连接 `minicode bridge`，界面组件与主题未改写。

## 运行链路

`Desktop / Ink TUI → NDJSON v2 → Rust Runtime → Provider / Tools / SQLite`。CLI 直接调用同一个 Runtime。Tokio 使用四个异步工作线程。Bridge 的会话请求与 run 在阻塞任务中驱动，SQLite、仓库扫描和附件 IO 不占用异步 worker；取消与审批绕过会话锁，stdout 由独立 writer 排队写入。

| 模块 | 职责 |
| --- | --- |
| agent/bridge.rs | clientKey / sessionId 路由、多会话、取消、审批、设置、统计缓存 |
| agent/runtime.rs | 轮次、工具顺序、预算、恢复、后台命令、子助手与验收 |
| agent/provider.rs | OpenAI、Anthropic、SSE、重试、用量、离线脚本 |
| agent/tools.rs | 文件、命令、归档、技能、任务与记忆工具 |
| agent/command_process.rs | Windows Job Object / Unix 进程组回收 |
| agent/store.rs | SQLite schema 7、事务检查点、会话锁、任务 DAG、记忆 |
| agent/paging.rs / context.rs | UTF-8 分页、稀疏游标、token 估计、配对、归档与压缩 |
| agent/extensions.rs / mcp.rs | 指令、技能、插件指纹、本地资源及三种 MCP 传输 |
| agent/attachments.rs / goals.rs | 内容寻址媒体、能力校验、受保护文件和验收证据 |
| agent/cli.rs / eval.rs / report.rs | 命令、持续会话、原生评测、HTML / 文本报告 |

## 执行与恢复

工具保持「记录调用 → schema 校验 → 工作区及指令预检 → 权限审批 → 执行 → 原子记录结果」。无效参数不触发审批。连续内置只读工具最多并发 4 个；连续只读子助手最多并发 2 个；写入与未知工具保留顺序屏障。

模型响应及用量先落库，再检查累计 token 预算并执行工具。轮数与墙钟预算用于每次可恢复切片；取消回收命令与 MCP 进程并记录状态。`/continue` 不添加重复用户输入。后台结果在轮次边界交付，执行结束时取消剩余任务并标记失联。

恢复先读取工具结果日志并补齐送达。缺失日志的只读调用可重试；编辑、命令及其他副作用标记状态未知，先核实再继续。子助手用量按 child session 去重，离线脚本计数包含子助手响应。

数据库保持 schema 7 和原字段格式；schema 3–6 补齐迁移，未来 schema 拒绝打开。配置、会话、附件与记忆仍位于用户 `.minicode` 目录，Windows 身份摘要继续使用 DOS / UNC 路径。OS 文件锁限制一个会话的单写者，崩溃后自动释放。

## 扩展与上下文

项目 AGENTS.md 正文与摘要在会话中固定；全局指令在后续模型轮次刷新。进入新子目录加载指令后，当前响应中的同范围工具暂停，下一轮按新指令重试。技能发现只读 YAML 头部，正文按需加载并验证摘要。

插件位于 `.minicode/plugins/<name>/plugin.json`，声明 minicode_version、技能、子助手及 MCP。`minicode plugins lock` 锁定指纹。示例见 examples/mcp_docs。本地导入资源默认关闭，不改写原应用配置；外部 UI / JS 钩子保持不支持的标记。

从 2.0.1 起，minicode_version 表示同一主／次版本系列中所需的最低补丁版本；例如 2.0.0 插件可用于 2.0.1，要求未来补丁或不同主／次版本的插件仍拒绝加载。

MCP 支持 stdio、Streamable HTTP、旧 SSE；初始化、分页、schema、审批和连接生命周期由 Rust 管理。输入有大小限制，UTF-8 按字节边界组装，公共配置不返回密钥。

上下文按完整交互单元归档，工具调用与结果成对保留；旧结果留下预览与 artifact 引用，最近约 20k token 和最近两个结果保留。长命令输出写磁盘，UI 只保留预览。归档缓存最多 16 文件 × 64 个稀疏游标。

自动压缩在上下文接近窗口上限时触发，预留量为窗口的 10%，最多 16,384 token。大块工具结果只有在正常压缩后仍超出硬上限时才归档；文件结果超过 16KB 不会单独触发压缩。MCP 启动探测状态持久保存并可通过连接检测查看，对话只显示实际调用失败。

## 评测与发行

`minicode eval` 支持 B0（基本循环）、B1（压缩）、B2（压缩与验收失败续跑）。默认离线脚本，--model 选择真实服务；--repeats、--concurrency、--schedule 控制调度。每次尝试从干净工作区开始，宿主独立检查测试、允许修改路径及受保护文件，再运行工作区外隐藏测试。完成记录不会隐式重跑。

Windows 发行包包含一个 Rust Agent、原生 Desktop、Git、Node.js、Ink 生产依赖和评测夹具。构建不需要 Python。MyGo 沿用原 Ed25519 公钥，更新替换整个 runtime，个人数据留在安装目录外。
