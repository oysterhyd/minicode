# 补充场景（独立于 20 题微型集）

这三题补足微型单文件集没有覆盖的行为：

| 场景 | 验证点 | 离线 B0/B1/B2 |
| --- | --- | --- |
| `invoice_discount` | 两个实现文件都需修改；测试与隐藏断言保护折扣和税额语义 | 通过/通过/通过 |
| `log_context` | 约 190 KB 日志；需有界搜索或分页读取，返回同一请求的最后状态 | 通过/通过/通过 |
| `counter_retry` | 脚本首次给出错误修复；宿主验收失败后 B2 续跑并纠正 | 失败/失败/通过 |

运行离线场景：

```bash
python -m evals.run_eval --tasks-dir evals/scenarios --baselines b0,b1,b2 --output reports/eval-scenarios
```

运行真实模型重复场景：

```bash
python -m evals.run_real_eval --tasks-dir evals/scenarios --baselines b0,b2 --repeats 3 --concurrency 1 --schedule alternating --output reports/eval-real-scenarios-20260924
```

真实评测会调用 CommandCode API。每次运行从干净副本开始，隐藏测试只在模型停止后由宿主执行。`source-snapshot.zip` 和配置中的 SHA-256 固定了该批运行读取的 harness 与任务文件；重新运行时不得悄悄换代码。

`log_context/repo/trace.log` 可用 `python evals/scenarios/generate_trace.py` 再生。它不要求模型必须读完整日志；有界检索并定位相关记录同样是正确行为。三个场景是补充，不把它们冒充原计划的 20 题真实评测结果。
