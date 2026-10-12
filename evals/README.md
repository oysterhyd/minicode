# 原生评测

Rust 宿主 minicode eval 使用 20 个 Node.js 微型缺陷仓库及 3 个补充场景。Node.js 同时服务现有 Ink TUI，发行包没有额外解释器或 Python 评测依赖。

```powershell
cargo build --release --locked
.\target\release\minicode.exe eval --baselines b0,b1,b2 --concurrency 3 --output output/eval
.\target\release\minicode.exe eval --tasks-dir evals/scenarios --baselines b0,b1,b2 --output output/scenarios
node evals/verify-results.mjs output/eval output/scenarios
```

B0 运行基本循环，B1 增加压缩，B2 收到独立验收失败反馈后继续修复。20 题各基线均应通过；counter_retry 在 B0/B1 失败、B2 通过，其余场景通过。

--model <配置模型 ID> 启用真实模型；--repeats 3 --concurrency 1 --schedule alternating 控制重复与调度。真实调用使用用户配置的服务。每次尝试复制干净夹具，宿主独立检查测试、受保护文件和允许修改路径，最后执行工作区外隐藏断言。runs/.../record.json 保存完成记录；同配置重跑复用记录，配置或夹具变化拒绝混用。

输出 config.json、results.json、summary.md、每次尝试的工作区及 SQLite 日志。`node evals/generate-tasks.mjs --verify` 验证 20 题红→绿，generate-hidden.mjs 再生隐藏断言，scenarios/generate-trace.mjs 再生长日志。

Node 夹具保留原缺陷类别，旧 Python 真实模型分数不能当成新版分数。历史报告仅描述当时版本；本次证据见 [Rust 迁移报告](../docs/rust-agent-2.0.0.md)。
