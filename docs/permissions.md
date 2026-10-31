# 权限与 Shell 沙箱

规则来源依次为 `~/.minicode/settings.json`、项目 `.minicode/settings.json`、项目 `.minicode/settings.local.json` 和原生 CLI 的 `--allow` / `--ask` / `--deny`。各层累加，所有显式 deny 优先，其次 ask、allow，最后才是所选权限模式。高层 allow 不删除低层 deny。

```json
{
  "permissions": {
    "allow": ["bash(cargo test:*)", "bash(git status)", "edit(src/**)"],
    "ask": ["bash(git push:*)", "mcp__browser__*"],
    "deny": ["read(.env)", "read(**/*.pem)", "bash(rm -rf:*)"]
  }
}
```

命令前缀按完整 token 匹配；复合命令逐段判断，`npm test` 的授权不包括下一段未知命令。无法可靠解析的结构要求单次确认；存在 shell deny 时，对无法证明未绕过拒绝的结构直接拒绝。文件规则以工作区相对路径匹配实际 canonical 路径，搜索和列表同样过滤被拒绝的文件。为保证文件 deny 不被任意脚本绕过，工作区存在文件 deny 时，Shell 保守拒绝执行，仍可使用受规则限制的原生文件工具。

审批可选择仅本次、记住本会话的具体范围、写入项目本地规则。界面展示保存的规则；Shell 不会因一次审批而整工具放行。MCP 在 bypass 下仍需明确规则或确认。workspace trust、子任务合并和高危命令不产生可记住的授权。

`rm -rf`、强制推送及常见下载后直接执行的形式需要单独的高危确认。已有 deny 直接拒绝，不能在高危确认中批准。静态识别无法证明任意脚本的所有行为；运行陌生代码时应启用 OS 沙箱。

## 启用沙箱

在以上任一设置文件增加：

```json
{
  "shellSandbox": {
    "enabled": true,
    "network": false,
    "readOnlyPaths": []
  }
}
```

原生 CLI 可用 `minicode run "任务" --sandbox` 或 `minicode chat --sandbox` 临时启用，`--no-sandbox` 关闭普通父会话命令的沙箱。默认关闭。`readOnlyPaths` 用于显式授权工作区外的工具链或缓存目录，只增加读取/执行权限。

- Windows 使用 AppContainer。工作区和临时目录可写，另行授权的目录只读，未授权用户目录和网络访问由操作系统限制。当前仅支持 `network=false`；设置 true 时拒绝执行。Git Bash/MSYS 依赖沙箱不允许的全局命名对象，因此拒绝该路径；请使用 `powershell` 工具。内置可写助手已包含 PowerShell。
- Linux 使用 bubblewrap 和独立文件系统/网络命名空间。需安装 `bwrap` 并允许无特权用户命名空间；系统工具目录只读，工作区及私有临时目录可写。显式 `network=true` 才共享网络。
- macOS 使用 `sandbox-exec`/Seatbelt，按 profile 限制文件读取、写入与网络。命令不存在或 profile 不受支持时拒绝执行。

沙箱启动失败时命令不会以普通宿主进程重试。`shell_sandbox` 事件只有在受限进程成功创建后才记录 enforced=true；准备失败时记录 enforced=false 和 command_executed=false，工具返回失败。Windows 使用受保护的启动握手；Unix 启动状态无法验证时会明确报告“可能已开始”，不把未知状态说成未执行。沙箱进程使用环境变量白名单，不继承 API Key、代理凭据等任意环境变量。后台任务、超时与取消沿用进程树清理，Windows 临时 AppContainer 授权和 profile 在正常收尾时撤销。

Windows 的文件、网络和取消行为在本机测试；Linux/macOS 实现需要相应平台执行验证。

## 可写子任务

子助手默认只读。父会话须显式使用 `delegate(..., writable=true)` 才会创建独立副本。子 Shell 强制使用 OS 沙箱，普通父会话的 `--no-sandbox` 不能取消该边界；沙箱不可用时子命令失败。

完成后，父会话先调用 `review_delegate` 的 inspect 操作查看 diff，在后续模型响应中选择 merge 或 discard。合并在所有权限模式下都需要用户独立确认；冲突会阻止写入，拒绝合并保持主工作区内容不变。独立副本不会修改主仓库 index 或创建 Git 提交。

实现依据：[Windows AppContainer](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)、[bubblewrap](https://github.com/containers/bubblewrap)。
