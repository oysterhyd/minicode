# 分页演示

paginate.mjs 故意保留 `const end=start+pageSize+1` 的边界错误，修复后测试通过。

```powershell
minicode run "修复分页 bug 并验证" --workspace examples/pagination --provider fake --script examples/pagination/scripts/fix_pagination.json --yes
```

离线脚本按序读取、编辑、运行 node --test test_paginate.test.mjs 并总结，无需模型密钥。发行包包含 Node.js。去掉 fake / script 并选择配置模型即可执行真实任务。--acceptance examples/pagination/acceptance.yaml 启用验收门，minicode report <session-id> 查看记录。

重复演示前将修复后的 `const end=start+pageSize` 改回带 +1 的版本。这些测试故意失败，不属于主项目测试集。
