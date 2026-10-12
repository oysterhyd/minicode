# 补充场景

| 场景 | 验证点 | B0 / B1 / B2 |
| --- | --- | --- |
| invoice_discount | 两文件修复、折扣先扣后计税、测试保护 | 通过 / 通过 / 通过 |
| log_context | 188815 字节日志、有界检索 / 分页、最后状态 | 通过 / 通过 / 通过 |
| counter_retry | 错误首修、独立验收反馈与续跑 | 失败 / 失败 / 通过 |

运行 `minicode eval --tasks-dir evals/scenarios --baselines b0,b1,b2`。
--model <模型 ID> --repeats 3 启用真实模型。夹具用已有 Node.js，隐藏断言位于模型工作区外。日志可由 `node evals/scenarios/generate-trace.mjs` 再生。
