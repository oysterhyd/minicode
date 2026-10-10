# v1.2.2 核验

2026-10-10，开始页仅保留简短提示、工作区选择与任务输入，移除大图标、建议卡片和最近任务区。

- 原生测试、Go 静态检查与 Windows 可执行文件编译通过，浅色 / 深色截图及小窗口布局已检查。
- [完整发行构建](https://github.com/oysterhyd/minicode/actions/runs/38034726932) 通过：Python 563 通过、2 跳过，TUI 79 通过；包内空白配置、原生窗口、桥接、CLI / Git / TUI 烟测通过。
- 中文路径安装、Unicode 快捷方式、卸载、签名更新、篡改拒绝、旧 Python 进程继续运行与更新后烟测通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38034727099) 七个任务全部通过。
- GitHub 上的安装包摘要与校验文件一致；完整更新归档及 1.2.0 / 1.2.1 差分包的摘要与 Ed25519 签名核对通过。
- [v1.2.2 已发布](https://github.com/oysterhyd/minicode/releases/tag/v1.2.2)。安装包 SHA256：`4fe8ff137e26eb0e5350a7ebbf41b38d282e0d6171a35a2a018bec753ac881f1`。

## v1.2.1 核验

2026-10-10，Windows x64。

- 完整 Python 测试：562 通过、3 跳过；Ink TUI：79 通过，包含真实 Python 桥接。
- 原生 `go test -count=1 ./...`、`go vet ./...` 与 Windows 可执行文件编译通过。
- 生成 28 张浅色 / 深色状态截图，检查权限设置、主页权限菜单、模型菜单、空白模型配置和收起侧栏。
- 新增滑条拖动、释放后提交、方向键、点击刻度、禁用状态及模型切换检查；最小窗口、150% 缩放和较大文字布局检查通过。
- 首次启动不预置服务与模型；从零保存服务、已有配置保留及删除最后服务后的空模型状态通过验证。
- 本机源码 1528 个文件与 Git 历史 2054 个 blob 的个人文件 / 已知凭据审计通过。

发行构建使用仓库 GitHub Actions 中已有的更新签名密钥。[Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38025249894) 完整执行测试并通过：干净发行 Python 环境 563 通过、2 跳过，TUI 79 通过；安装包与签名归档已生成。

- 包内空白服务 / 模型配置、真实原生窗口、Python 桥接、CLI / Git / TUI 运行烟测通过。
- 中文及空格路径中的 NSIS 安装、Unicode 开始菜单快捷方式与卸载检查通过。
- Ed25519 签名验证、拒绝篡改更新、真实 SDK 安装更新、相同版本不重复更新及旧 Python 进程继续运行检查通过。
- 最终运行环境 15342 个文件的凭据审计通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38025242821) 七个任务全部通过。

下载发行产物后再次核对安装包 SHA256、完整更新归档和 1.2.0 → 1.2.1 差分包的 Ed25519 签名，均通过。安装包摘要为 `2ae09c96655e7fa5999f862530821878fbf43680847a633b96a9a949dc5eee15`。

## v1.2.0 本地发行核验

2026-10-08，Windows x64，PowerShell 7，MyGo 0.2.15，Go 1.27.1，
CPython 3.12.12，Node.js 22.22.0。Desktop 使用原生 UI / D3D11。

- 干净发行 Python 完整测试：560 通过、3 跳过；Ink TUI：79 通过，包含真实 Python bridge、审批、取消及恢复。
- 原生 `go test -count=1 ./...`、`go vet ./...` 通过，无 CGO 的 Windows / Linux 编译通过。真实 GUI 只验收 Windows。
- 浅色 / 深色 18 张状态截图、最小窗口及 150% 缩放、快捷键、补全、撤销、排队、菜单、Markdown、图片边界、更新偏好和文件/Git 服务检查通过。
- 包内 Python、Git、CLI、TUI 及实际原生窗口在空白用户目录下通过；B0/B2 离线分页评测 2/2；NDJSON v2 与原生更新可用性通过。
- 实际 NSIS 静默安装至中文和空格路径，程序摘要、开始菜单、卸载登记及安装后运行烟测通过；卸载清除测试目录和登记，原有用户快捷方式恢复。
- MyGo 更新清单和归档的 Ed25519 签名验证通过。真实 SDK 在中文和空格路径中拒绝篡改签名、保留原程序；正确签名安装完整运行环境，摘要一致；相同版本不重复安装；更新后的实际运行环境烟测通过。
- 源码、历史和最终运行环境凭据审计通过，个人目录及签名私钥未进入包。GitHub Actions 使用 repository secret，公钥编入正式应用。
- 独立算法和原生/Chromium fixture 响应测量见 [迁移报告](../docs/native-desktop-report.md)，其中说明计时边界，未声称输入到屏幕、模型、内存或启动性能改善。

物理输入法候选窗、全部通知点击与完整动效逐像素一致性尚未完成手动验收。截图来自 MyGo CPU 场景渲染，GPU 运行另由实际窗口 FrameStats 验证。测试使用离线 Provider，没有进行新的付费模型评测。安装器未作 Windows 代码签名；更新归档签名独立于代码签名。远端 CI 结果在 GitHub Actions 记录，本页数量来自本机完整测试。

首次远端 Windows 验收发现 TUI 测试使用的 Node JS `realpathSync` 保留 8.3 别名，Python 则返回长路径。测试改用 `realpathSync.native` 比较实际目录；此修正仅影响测试，不改变发布程序。

修正后的 [跨平台 CI 七个任务](https://github.com/oysterhyd/minicode/actions/runs/37736334659) 全部通过。发行安装的快捷方式验收使用 Unicode `IShellLinkW` 和原生路径解析，以识别中文与长短路径；WScript 在英语系统上把中文目标读成问号，已替换该验收 API。额外验收已加载 SSL / SQLite 的 Python 进程在签名升级过程中继续运行并返回查询结果。

发布后的公开 `latest` 清单已通过真实 SDK 检查，新版本可发现、同版本被忽略；下载完整公开归档后签名验证通过，安装器校验值与发布资产 digest 一致。

最终 [Windows 发行 CI](https://github.com/oysterhyd/minicode/actions/runs/37738782495) 通过：实际中文路径安装、Unicode 快捷方式、卸载、签名升级、旧 Python 进程继续运行和更新后完整烟测。该次手动构建复用此前已通过的全量测试结果，使用 `skip_tests=true`；所有安装与更新验收仍完整执行。已发布的安装包保持原校验值。

## v1.1.0 本地发行核验

Windows x64，PowerShell 7，CPython 3.12.12，Electron 44.2.0，Node.js 22.22.0。

- 干净发行环境全量 Python 测试：560 通过、3 跳过；Electron 测试：43 通过；
  Ink 测试：79 通过，包含真实 Python bridge、审批、暂停/继续及取消。
- Desktop 和 TUI 生产构建通过，安装包内置 Node.js 与 TUI 生产依赖。
- wheel 与打包 Python 均不含旧 `minicode.ui` 或 Textual 依赖。
- 空白用户目录、无系统 Python/Git/Node 的 PATH 下，包内 CLI、启动器、Ink 模块、
  FakeProvider 执行与 bridge 正常退出通过；仓库外 B0/B2 分页评测为 2/2。
- Windows ConPTY 中实际启动包内 CLI/TUI，中文输入、审批写入、回复和正常退出通过。
- 真实 Electron 页面与 Python IPC 初始化、Git 统计及暂存通过。
- Gitleaks 历史和源码快照无发现；源码及最终打包目录 13827 个文件通过本机凭据审计。
- GitHub Windows runner 的临时目录同时存在长路径和 8.3 短路径，桥接集成测试改为
  比较实际路径，避免把同一目录误判为不同工作区。该修改只影响测试。
- 安装包 SHA256 随 Release 提供，未做商业代码签名。远端 CI 结果由 GitHub Actions
  单独记录；本页列出的测试数量来自本机完整发行构建。

## v1.0.0 本地发行核验（2026-09-30）

发行目标：Windows 10/11 x64。实际验证环境为 Windows x64、PowerShell 7、
CPython 3.12.12、Electron 44.2.0。

- TypeScript/Vite 生产构建成功。
- 使用安装包内置 Python 和锁定依赖执行全量测试：565 通过、3 跳过。
- Electron 主进程及打包路径测试：28 通过。
- 从最终安装包静默安装到包含中文与空格的路径，安装成功；开始菜单的
  Desktop/CLI 快捷方式以及卸载程序已生成。
- 在空白 USERPROFILE/APPDATA 中运行真实 Electron。应用进程的 PATH 仅保留
  Windows 系统目录，Python 桥接初始化、无密钥配置读取、工作区文件的 Git
  行数统计与暂存成功；真实渲染页面截图已检查。
- 安装后的 CLI launcher、内置 Git、Python 模块入口正常；仓库外的分页修复
  B0/B2 无密钥评测为 2/2 通过。
- Gitleaks 8.30.1 扫描全部历史与待发布源码快照，未发现凭据。
- 精确匹配本机配置和环境中的已有凭据：历史 804 个 blob、源码 366 个文件、
  最终打包目录 8156 个文件均通过。个人配置、私钥和会话数据检查通过；
  公开 CA 证书正常保留。
- 安装包 SHA256 单独随 Release 提供。安装包未做代码签名。

真实模型调用必须由下载者在设置中添加自己的服务凭据。本次安装验证使用
离线模型和已有确定性 Provider 测试，不将其描述为新的付费 API 效果评测。
GitHub Actions 的结果由远端运行另行记录，本页只记录本机验证。

安装产物只从干净 Python、受校验的 MinGit、锁定依赖和源码白名单生成；
本机虚拟环境、缓存、个人 API key 和服务配置均不随源码或安装包发布。
