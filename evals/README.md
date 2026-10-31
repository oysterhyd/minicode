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

多模型与逐机制消融可用一条命令生成 `summary.md`、`statistics.json` 和逐次 `results.json`：

```powershell
./target/debug/minicode.exe eval --model service::model-a,service::model-b --baselines b1 --ablations full,no-compression,no-cache,no-subagents,no-prompt --repeats 3 --temperature 0 --concurrency 1 --schedule alternating --prices prices.json --output output/eval-v3.0.3
```

模型 ID 可重复传 `--model` 或用逗号分隔。显式模型/机制评测默认至少三次；旧离线命令默认仍为一次。`default` 保留 B0/B1/B2 语义；`full` 开启全部机制，其余变体每次仅关闭一项，所以推荐以同一 B1 比较。no-prompt 仅替换内置系统模板，保留环境、项目指令、Skills 与权限；no-subagents 同时隐藏工具和阻止直接调用。无触发的机制不能据分数推断收益，微型任务尤其不会触发压缩。

`--temperature` 和 `--seed` 可选，原样写入请求并记录在配置；未设置时为 null，不能声称已固定随机性。Anthropic 不支持 seed，会明确报错；模型也可能拒绝 temperature。no-cache 关闭 Anthropic 显式缓存标记；OpenAI 服务端自动缓存不可从这里关闭，记录中的 cache_control 会标注该限制，不应将它解释成有效的缓存因果消融。

`prices.json` 由调用者提供并随结果保存，格式为 `{"service::model-a":{"input":1,"output":2,"cache_read":0.1,"cache_write":1.25}}`，单位 USD / 百万 token；四项必须齐全且非负。不推断网关折扣。缺价格、不完整 usage、失败请求或离线合成 usage 的费用为 null。缓存命中率为已测缓存读取 token / 已测总输入 token。统计按模型、机制、基线分组，并提供每任务分组；均值附样本数，方差采用 n-1，少于两次时为 null，失败保留在成功率分母。

输出记录版本、二进制 SHA256、夹具 SHA256、采样参数、机制、定价和调度，拒绝向不同配置的目录追加。跨版本趋势可用 `node evals/compare-results.mjs output/eval-v3.0.2 output/eval-v3.0.3 > output/trend.json`；每行保留版本/二进制身份，夹具、采样、价格或调度变化时 comparable=false。保留历次输出，不把不同任务集或历史 Python 分数合并。

增加 `--record` 为每次评测保存脱敏的 provider.jsonl 与结构化 events.jsonl；`record.json.provider_recording` 可直接传给 `minicode replay <文件> --output <新目录>`，在无 API/工具副作用的条件下重现 Runtime 路径。录制范围、限制和环境变量见 [可观测与回放](../docs/observability.md)。

输出 config.json、results.json、summary.md、每次尝试的工作区及 SQLite 日志。`node evals/generate-tasks.mjs --verify` 验证 20 题红→绿，generate-hidden.mjs 再生隐藏断言，scenarios/generate-trace.mjs 再生长日志。

Node 夹具保留原缺陷类别，旧 Python 真实模型分数不能当成新版分数。历史报告仅描述当时版本；本次证据见 [Rust 迁移报告](../docs/rust-agent-2.0.0.md)。
