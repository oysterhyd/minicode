# 阶段 8A：工程化与评测

版本 3.0.3，接续 40c81b6。四项均核查后实施，没有按过时描述跳过现有缺口。未修改 Desktop、未引入依赖，保留原有两份 .pending 文件；未推送或调用付费模型。

| ID | 状态 | 结果与证据 |
| --- | --- | --- |
| E1 | 已修复 | [Provider::for_settings 与脚本恢复](../agent/provider.rs) 集中构造、同身份游标继承和事件恢复；Bridge/CLI/Runtime/评测统一使用。 |
| E2 | 已修复 | 历史设计标注、Rust 路径映射、TaskStore 与既有真实评测状态已更正；[路径检查器](../scripts/check-doc-paths.mjs) 加入 CI。 |
| E3 | 工程能力已实现 | 多模型、五机制组、至少三次重复、均值/样本方差、缓存和费用、版本趋势。已有单模型 120 次历史实测未被误判为不存在；此次以本地 HTTP 模拟验证能力，未追加付费质量评测。 |
| G5 | 语义录制与回放已实现 | [结构化日志/录制模块](../agent/observability.rs)、独立 replay 命令、逐 run 录制；恢复工具/验收结果与子代理用量，回放禁止网络和工具副作用。 |

提交按条目拆分：dad5c8a（E1）、78af9f5（E2）、b548ce2（E3）；G5 使用独立 feat(observability) 提交，具体对象见 git log。

E1 修改 Provider、Runtime、Bridge、CLI、评测与 Rust 回归。E2 修改 plan.md、两份历史设计、CI、scripts/check-doc-paths.mjs 和 doc-paths.test.mjs。E3 修改 eval/eval_stats、请求和机制开关、系统模板选择、evals/README.md、compare-results.mjs 与 eval-matrix.test.mjs。G5 修改 observability、Provider/Runtime/CLI/Bridge 接线、Response 序列化、评测录制和对应回归；详细根因与修复过程见 [fixes-3.0.3](fixes-3.0.3.md)。

先失败后通过的回归包括 E1 缺统一入口、E2 失效路径、E3 缺矩阵及直接调用绕过委派禁用、G5 缺录制、含空格密钥片段泄漏、委派用量丢失、panic 类型丢失、非默认 effort 和新任务验收尝试重置。最终回归还覆盖配置恢复不重复请求、未知价格/用量、失败分母、旧离线命令、跨分片脱敏、文件变化后回放原工具结果、验收不重执行、截止/取消、并发追加、截断/损坏记录、选择历史 run、录制写入失败。

与 CI 相同的整套检查在每项提交前执行，日志分别为 output/stage8a-e1-retry/、output/stage8a-e2-final/、output/stage8a-e3/、output/stage8a-g5-complete/，包含 fmt、Clippy、Rust 测试/构建、Node 协议、夹具验证和生成差异、两套离线评测、结果校验、TUI npm ci/build/真实 Bridge 测试。Desktop 未改动，因此未追加 Go 检查。

最终构建：Rust 234 通过/2 忽略，协议 317 通过/3 跳过，TUI 142 通过；离线任务 60/60，场景 7/9 符合预期。fmt、Clippy、构建、夹具生成差异、结果校验及 TUI 安装/构建均通过。G5 保留同一最终二进制已通过的 Rust 检查，首次协议轮的迁移竞争见 protocol-initial.log，成功重跑见 protocol-retry-1.log；成功日志归档于 output/stage8a-g5-complete/。二进制 SHA256：F66811CE711EB41DF04A63C6E74BD840194FAC20A749C281FAB5828ECE1348D1。

早期整轮失败均保留：E1 首轮受宿主 FORCE_COLOR 干扰并有进程退出超时；清理该环境变量后通过。E2 与 G5 首轮出现既有首次建库竞争（EPIPE/database locked/duplicate column），本批未修改该批次外实现。另有既有插件指纹缓存和并发编辑测试的偶发失败，未修改这些实现。成功整轮与失败日志分开，不能把重跑通过表述为该竞争已修复。

待确认决策与限制：

- 真实模型评测默认每组至少三次；价格必须显式提供，未知费用为 null。采样参数可选，三次重复不代表统计显著性。
- OpenAI 服务端自动缓存不可由 no-cache 关闭，报告注明能力，不能据此声称完成缓存因果消融。微型任务未触发压缩时也不能推断其收益。
- 录制默认关闭，脱敏覆盖已知凭据模式但不等于匿名化。语义回放不恢复原文件副作用或原始网络分片；首请求前故障、记录 I/O 失败、记录不完整的机器崩溃不保证完整重现。见 [操作与边界](observability.md)。

下一批建议先复用录制 schema 扩展 E4 故障素材和跨文件/长上下文场景，再在固定版本、模型能力与价格配置下运行真实多模型对照；将首次建库竞争单列到后续存储修复，避免混入阶段 8 能力条目。
