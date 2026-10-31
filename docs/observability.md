# 结构化日志与离线回放

3.0.3 的 G5 提供默认关闭的本地 JSONL 日志和 provider 语义录制。环境变量由 CLI、Bridge、TUI 和 Desktop 启动的 agent 继承；不改变协议版本、旧配置或会话 schema。

```powershell
$env:MINICODE_LOG_FILE = "$PWD/output/events.jsonl"
$env:MINICODE_PROVIDER_RECORD = "$PWD/output/provider.jsonl"
./target/debug/minicode.exe run "检查并修复问题" --workspace ./repo --model service::model
```

日志包含 schema_version、timestamp、level、kind、session_id、run_id、call_id 和 data。provider_start/provider_end/provider_error 关联同一个请求，run_start/run_end 关联一次执行；Runtime 事件保留工具 call_id。文件追加使用进程间锁；provider 开始、完成、工具结果与 run 边界同步落盘。

评测可将每次运行的录制文件与报告放在一起：

```powershell
./target/debug/minicode.exe eval --model service::model-a,service::model-b --baselines b1 --ablations full,no-compression,no-cache,no-subagents,no-prompt --repeats 3 --temperature 0 --record --output output/eval-recorded
```

每个独立 run 目录包含 provider.jsonl、events.jsonl 和 record.json；record.json 的 provider_recording 指向对应文件。录制开关进入评测配置身份，不能将未录制的完成记录误用为已录制结果。

复制录制文件即可独立重现 Runtime 的 provider、工具结果与验收路径：

```powershell
./target/debug/minicode.exe replay ./output/provider.jsonl --output ./output/replay-case
# 一个文件中有多次 run 时，默认选择最后一个根 run；也可指定日志中的 run_id。
./target/debug/minicode.exe replay ./output/provider.jsonl --run-id <run_id> --output ./output/replay-earlier
```

replay 在新目录创建独立 SQLite 会话、空工作区、events.jsonl 和 result.json。它从录制文件恢复起始上下文、模型元数据、预算、机制、已用调用 ID、验收与累计用量，不需要原模型服务或 API key。普通回放结果写入 result.json；重现 provider panic 时沿用原有非零错误退出，证据保留在日志和 SQLite 中。工具、Shell、子代理、验收命令和 MCP 不执行，返回录制结果；共享用量包含子代理消耗。请求指纹不匹配则明确失败。已有输出目录不覆盖。

也可将 `MINICODE_PROVIDER_REPLAY` 指向录制文件，然后用相同输入启动 CLI/Bridge；它与 MINICODE_PROVIDER_RECORD 互斥。请求匹配会归一化工作区绝对路径与环境日期，其余输入变化仍须匹配；CLI replay 已使用录制的起始消息和系统提示词。HTTP 状态、Retry-After、上下文溢出、流错误、超时和取消保留原有重试分类；取消/截止时间导致 future 被丢弃时也会写结束记录。缺少结束记录的 durable start 按 Interrupted 处理；只有最后一个未完成 JSON 行可以忽略，已提交行损坏或未知 schema 会拒绝。

录制内容是归一化请求、解析后响应、合并后的流输出、工具结果与用量，不是原始 HTTP 字节抓包。它验证执行语义，不能复现 TCP 分片或解析器内部缺陷，也不重建原工作区文件副作用。回放时长不用于性能对比；模型/Runtime 行为改变导致指纹或分支不匹配时应保留失败证据。启动早于首个 provider 请求的故障、磁盘写入失败，以及录制文件不完整的机器崩溃不保证能重跑完整 Runtime。

不保存鉴权头、API 地址或密钥配置。脱敏覆盖当前 provider key、名称含 API_KEY/TOKEN/PASSWORD/SECRET/CREDENTIAL 的环境变量值、嵌套凭据字段、Bearer 和文本赋值；流分片先合并再脱敏，避免完整密钥跨片泄露。提示词、源码和工具输出仍可能含无法自动识别的私人内容，因此录制明确由用户开启，外发前需检查。文件上限 128 MiB，每种部分流输出上限 16 MiB；Unix 新文件为 0600，Windows 继承目标目录 ACL。默认不产生录制或日志文件。
