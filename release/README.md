# MiniCode 1.2.0 Windows 发行版

Windows 10/11 x64 原生 Desktop + CLI + Ink TUI。Desktop 使用 MyGo 0.2.15 / Go，安装包内含 Python 3.12.12、Git 2.56.0、Node.js 22.22.0 和锁定运行依赖。无需安装 Electron、WebView2、Python、Git 或 Node.js。项目编译器和语言 SDK 仍由项目自行提供。

安装包为 `dist/MiniCode-Setup-1.2.0-win-x64.exe`，SHA256 在 `dist/SHA256SUMS.txt`。发布地址为 [v1.2.0](https://github.com/oysterhyd/minicode/releases/tag/v1.2.0)。安装包未作 Windows 代码签名；自动更新归档使用 Ed25519 签名。

## 安装和使用

运行安装向导后打开 MiniCode，在“设置 → 模型与预算”添加自己的 OpenAI 兼容或 Anthropic 服务，填写地址、密钥和模型 ID，测试连接并选择默认模型。打开本地项目后即可开始任务。发行包不提供模型账号或服务密钥。

安装目录包含 `minicode.cmd`：

```powershell
& "$env:LOCALAPPDATA\Programs\MiniCode\minicode.cmd" --help
& "$env:LOCALAPPDATA\Programs\MiniCode\minicode.cmd" run "修复失败的测试" --workspace D:\my-project
& "$env:LOCALAPPDATA\Programs\MiniCode\minicode.cmd" tui --workspace D:\my-project
```

使用自定义安装目录时替换路径。CLI 不自动添加到系统 PATH。配置和会话继续保存在 `%USERPROFILE%\.minicode`；旧 Electron 界面偏好会导入原生用户数据目录。卸载保留个人配置和会话。

插件仍使用精确宿主版本约束。现有插件需将 `minicode_version` 更新到 `1.2.0` 后重新生成锁文件；仓库 MCP 示例已更新。

## 从源码打包

需要 Windows x64、PowerShell 7、Go 1.27.1、uv、Git 和 Node.js 22+。Node.js 在构建时负责 Ink TUI。

```powershell
go install github.com/egoist/mygo/cmd/mygo@v0.2.15
pwsh -NoProfile -File release/build.ps1
```

构建脚本创建干净 staging，安装带 SHA256 的 Python 锁定依赖和当前源码 wheel，构建 TUI，只携带其生产依赖。MinGit 和 Node.js 下载均核对 SHA256。最后调用 MyGo 创建 Windows 安装包和签名更新归档，运行包内运行环境烟测、产物审计及真实 SDK 升级验收。

默认检查 Go 原生 UI / 桥接 / 文件服务、完整 Python 测试及 TUI 单元与真实桥接测试。包内烟测使用空白用户目录；PATH 只包含包内工具和 Windows 系统目录，检查 CLI、Git、B0/B2 离线分页评测、TUI → Python NDJSON，以及原生窗口截图和协议。Windows 自带 PowerShell 用于命令工具。

```powershell
pwsh -NoProfile -File release/smoke-native.ps1
```

`-SkipTests` 仅用于已经单独完成测试后的重新打包；运行环境烟测和产物审计仍执行。完整构建不依赖 Electron 或 electron-builder。源码、个人目录、虚拟环境和用户数据不会被复制进安装包。

构建还执行 `release/test-installer.ps1`：将实际 NSIS 安装包静默安装到中文和空格路径，核对可执行文件、开始菜单及卸载登记，运行包内烟测，然后卸载。测试前备份已有同名快捷方式和登记，结束后恢复。

## 产物目录

```text
dist/MiniCode-Setup-1.2.0-win-x64.exe
dist/SHA256SUMS.txt
dist/update-windows-amd64.json
dist/minicode-1.2.0-windows-amd64.tar.gz
dist/native/windows-amd64/
  MiniCode.exe
  minicode.cmd
  LICENSE, THIRD-PARTY-NOTICES.md, licenses/
  runtime/python/, git/, node/, tui/, bridge.py
```

`dist/native/windows-amd64` 是打包布局目录；其中也保存 MyGo 生成的安装器。测试截图、临时用户目录和离线评测保存在 `.cache/`，不进入 Git。许可见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)，核验结果见 [VERIFICATION.md](VERIFICATION.md)，体积及性能对比见 [迁移报告](../docs/native-desktop-report.md)。

发布前按项目已有凭据审计流程检查 Git 历史、源码快照及最终产物。API 密钥配置逻辑和测试假密钥可以进入源码，个人配置与真实凭据不能进入发行包。自动发布 workflow 已改用固定版本 MyGo CLI。

## 自动更新与签名

Desktop 使用 MyGo 官方 `updater/native`，默认启动后约 10 秒检查一次，随后每 24 小时检查 GitHub Releases。“设置 → 通用”提供自动检查、自动下载安装和手动检查；“关于”也提供手动检查。自动下载安装默认关闭，开启后下次启动使用新版本。更新不主动中断任务。

MyGo 在可写的正式安装目录启用更新。开发构建或只读目录会显示禁用原因。1.1.0 的 Electron 版需要手动安装一次 1.2.0，此后的原生版本可应用内更新。Python 配置、会话及原生偏好均在安装目录外保留。

`desktop/native/mygo.json` 只包含公开验证密钥。签名私钥由 `MYGO_UPDATER_PRIVATE_KEY` 环境变量提供，本机构建也可从 `%APPDATA%\mygo\update-keys\mygo-update.key` 读取；私钥不进入仓库或发行包。GitHub Actions 使用同名 repository secret。保留并备份当前密钥，后续版本继续用同一把密钥签名；重新生成密钥会导致现有客户端拒绝更新。

每次发布必须同时上传安装包、`SHA256SUMS.txt`、`update-windows-amd64.json` 和清单引用的 `.tar.gz`（有差分包时也上传）。SDK 先验证归档摘要的 Ed25519 签名，再替换程序和完整运行环境。Windows 代码签名与更新归档签名分别处理。

```powershell
pwsh -NoProfile -File release/test-updater.ps1
```

此验收在 `.cache/` 的中文和空格路径里调用真实 MyGo SDK，确认篡改签名被拒绝且原程序未变、正确签名能替换完整运行环境、相同版本不重复更新，再对更新后的实际 Desktop / CLI / TUI 做烟测。正常构建自动执行该验收。
