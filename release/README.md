# MiniCode 1.0.0 — Desktop + CLI

Windows 10/11 x64 安装版，包含 Electron、Python 3.12.12、全部 Python 依赖和
Git 2.56.0。无需安装 Python、Node.js 或 Git。其他平台暂未提供安装包。

## 安装和使用

从 GitHub Releases 下载 `MiniCode-Setup-1.0.0-win-x64.exe`，运行安装向导，
选择安装位置。可用 `Get-FileHash <安装包> -Algorithm SHA256` 对照同页的
`SHA256SUMS.txt`。桌面/开始菜单的 MiniCode 快捷方式启动桌面端。

首次启动在设置 → 模型中添加自己的 OpenAI 兼容或 Anthropic 服务，填写
服务地址、API key 和模型，测试连接并设置默认模型，然后选择工作区开始任务。
发行包不包含作者的服务配置或 API key，不提供免费模型账号。

开始菜单的 **MiniCode CLI** 打开命令窗口。也可以在 PowerShell 中直接调用：

```powershell
& "$env:LOCALAPPDATA\Programs\MiniCode\minicode.cmd" --help
& "$env:LOCALAPPDATA\Programs\MiniCode\minicode.cmd" run "修复测试失败" --workspace D:\my-project
```

自定义安装位置时替换路径。CLI 不自动写入系统 PATH；可以将安装目录手动加入
用户 PATH。CLI 与 Desktop 共用 `%USERPROFILE%\.minicode` 下的配置和会话。
项目所需的其他语言 SDK/编译器仍由项目自身决定。

安装包未作商业代码签名，Windows 可能显示未知发布者。卸载保留用户目录中的
配置与会话；要彻底清除个人数据，退出应用后手动删除 `.minicode`。

## 构建和验证

要求 Windows x64、PowerShell 7、Node.js 22+、uv、Git。仓库根目录执行：

```powershell
npm ci --prefix release
pwsh -NoProfile -File release/build.ps1
```

`release/` 是集成项目：复用 `src/minicode` 与 `desktop/`，不复制维护第二套源码。
构建脚本从下载的干净独立 Python 创建 staging，只安装带哈希的锁定依赖和
当前源码生成的 wheel，再打包 Desktop；MinGit 下载也校验 SHA256。
不会复制仓库虚拟环境、用户目录、`.minicode` 或个人配置。

默认运行 Python 和 Electron 单元测试、源码/产物本机凭据审计、实际安装布局的
CLI launcher 检查、无密钥分页修复 B0/B2 评测，以及真实 Electron → preload →
Python IPC 启动检查。烟测使用空白用户目录和仅包含内置 Python/Git 的工具 PATH。
构建产物和验证截图位于被忽略的 `release/dist/` 和 `release/.cache/`。

发布前还须用 Gitleaks `git --redact` 审查全部 Git 历史，并在临时目录中扫描
`git archive HEAD` 导出的源码。API key 的配置逻辑、空值和测试假密钥可以发布；
个人配置文件和真实凭据不可以。仓库忽略规则与打包白名单同时防止误带入。
`python release/audit.py --source --history .` 还会对全部历史 Git blob 匹配本机
已有凭据，审计只报告文件位置，不打印凭据内容。

更新依赖锁：

```powershell
uv pip compile pyproject.toml release/runtime.in --extra real --extra eval --python-version 3.12 --generate-hashes --output-file release/requirements.lock
```

安装包及 `SHA256SUMS.txt` 上传 GitHub Release，构建产物不进入 Git 源码仓库。
第三方软件许可见 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。
v1.0.0 的本地验证结果见 [VERIFICATION.md](VERIFICATION.md)。
