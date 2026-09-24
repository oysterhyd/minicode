# 复盘：被上级忽略规则遮蔽的评测工作区

日期：2026-09-23。性质：本地确定性测试失败。

## 现象

全量测试中 `test_ripgrep_search_finds_results` 返回 `(no matches)`，长输出归档测试也未产生 artifact。测试工作区在仓库的 `.pytest_cache/` 下；ripgrep 从该目录运行时继承了上级忽略规则，跳过实际存在的文件。

## 原因与修复

工具把工作区当作用户指定的搜索根，却让宿主仓库的 ignore 配置决定哪些文件可见。现在调用 ripgrep 时使用 `--no-ignore --hidden`，并显式排除产品定义的缓存、版本控制和依赖目录。Python 回退策略使用同一 `SKIP_DIRS`。

## 验证与残余风险

两个原失败测试在 `--basetemp .pytest_cache/focused1` 下通过。搜索结果仍受每页结果数、行长和总输出预算约束；用户可缩小路径继续检索。
