# MiniCode 3.0.1 Windows 发行版

Windows 10/11 x64：Go / MyGo Desktop、Rust Agent / CLI、TypeScript + Ink TUI。内置 Git 2.56.0、Node.js 22.22.0；后端及内置评测不依赖 Python。项目自己的 SDK 和测试依赖由项目提供。

下载 [v3.0.1](https://github.com/oysterhyd/minicode/releases/tag/v3.0.1) 的 MiniCode-Setup-3.0.1-win-x64.exe；摘要见 SHA256SUMS.txt。首次启动在“设置 → 模型与预算”添加自己的服务。升级保留配置、schema 7 会话、附件和记忆。

## 从源码打包

需要 Windows x64、PowerShell 7、Rust stable（MSVC 目标与 C++ Build Tools）、Go 1.27.1、Git、Node.js 22+。

```powershell
go install github.com/egoist/mygo/cmd/mygo@v0.5.5
pwsh -NoProfile -File release/build.ps1
```

提供既有 MYGO_UPDATER_PRIVATE_KEY；本地也可读取 `%APPDATA%\mygo\update-keys\mygo-update.key`。GitHub Actions 使用同名 secret。公钥不变，私钥不进入仓库或产物。

脚本从干净 staging 构建锁定 Cargo 依赖，完整 LTO、静态链接 MSVC CRT；构建 Ink，仅安装生产依赖，裁剪声明、source map 和 Flow 类型元数据，并压缩 JavaScript，保留模块路径、函数／类名称、环境分支、运行资源与许可。压缩后直接运行包内 TUI 的完整回归。Git / Node 下载核对 SHA256；Cargo metadata 收集 Rust 依赖许可。

默认运行 Rust 单元与 Clippy、协议 / HTTP / MCP / 恢复测试、TUI 与 Go 完整测试、20 题及 3 场景评测。包内烟测使用空白用户目录和仅含包内工具及系统目录的 PATH，检查 CLI、评测、Git、TUI、真实窗口与更新能力。

test-installer.ps1 将实际 NSIS 包安装到中文和空格路径，验证 Unicode 快捷方式、登记、运行与卸载。test-updater.ps1 调用真实 MyGo SDK，验证签名、篡改拒绝、同版本忽略及完整 runtime 替换；旧 Rust 进程继续响应，更新后再做烟测。-SkipTests 仅用于已经完整检查后的重新打包，安装 / 更新验收仍运行。

## 产物

```text
dist/MiniCode-Setup-3.0.1-win-x64.exe
dist/SHA256SUMS.txt
dist/update-windows-amd64.json
dist/minicode-3.0.1-windows-amd64.tar.gz
dist/native/windows-amd64/
  MiniCode.exe, minicode.cmd, LICENSE, THIRD-PARTY-NOTICES.md, licenses/
  runtime/minicode.exe, git/, node/, tui/, evals/, licenses/
```

发布需上传安装器、摘要、更新清单及其引用的归档和差分包。MyGo 官方 updater/native 在下次启动应用更新，自动下载默认关闭。Windows 代码签名与更新归档签名分别处理；本发行包沿用 Ed25519 更新签名。

源码、个人目录、会话数据库和旧实现不进入安装包。3.0.0 界面与兼容说明见 [界面重设计](../docs/ui-redesign-3.0.md)，验收见 [核验记录](VERIFICATION.md)。历史修复和性能数据见 [旧版修复说明](../docs/fixes-2.0.3.md) 和 [性能报告](../docs/performance-2.0.2.md)。
