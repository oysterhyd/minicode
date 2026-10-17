# v3.0.1 工作台按钮布局修复与发行核验

2026-10-17，Windows x64 / PowerShell 7，MyGo 0.5.5。收拢按钮移至工作台工具栏左端，收拢／刷新按钮固定宽度，标签在窄侧栏内横向滚动；移除首页和对话区重复的收拢按钮，保留右栏收起后的展开入口。280／400／680 宽度、1.4 倍字体、首页／对话两类状态的六个边界及点击回归用例通过。

本地 `pwsh -NoProfile -File release/build.ps1` 未使用 SkipTests，Rust 46、后端集成 66、TUI 源码 97 和压缩运行时 97 项全过且无跳过；Go 全量含测量项通过，vendor UI 包无测试文件。fmt / Clippy、go vet / mygo vet 与原生 Windows 编译通过。60 次任务及 9 次场景符合预设结果，源码／历史／产物审计、包内烟测、签名更新、篡改拒绝、同版本忽略、更新后运行、中文／空格路径安装、快捷方式和卸载均通过。

实际 3.0.0 → 3.0.1 覆盖升级通过，升级前已正常关闭 MiniCode 并在仓库外备份数据；36 个原有会话、数据库各表、配置和偏好哈希不变。另以旧版写入非空记忆和会话，新版读取后内容一致。已安装包的首页／对话／关于页截图已捕获。

本地安装器 58,278,385 B，SHA256：`01f267e9ca152a82f737bfc2f8da2ad6e7ac5d24f35979761cd7f994a3b882d5`。公开资产和 CI 核验待发布后补记。

# v3.0.0 界面重设计与发行核验

2026-10-17，Windows x64 / PowerShell 7，Rust stable MSVC、Go 1.27.1、Node.js 22.22.0、MyGo 0.5.5、NSIS 3.13。PR #1 已 Squash 合并，提交 `f615f3ef8247012ee1bba1d0d8906f205bd485ab`；后续修复并行 fork 持有会话锁时的释放问题，[main CI](https://github.com/oysterhyd/minicode/actions/runs/39022699116) 五项全部通过（Windows Rust 46 项、Linux Rust 47 项）。

审查修复组合键误触发永久审批、减少动态效果仍播放加载动画、MyGo 0.5 安装器文件名变化、Windows 测试收尾瞬时目录占用，以及升级主版本后插件测试样本写死旧兼容版本的问题。插件主／次版本兼容校验保持原有行为；若插件显式声明 2.0.x，需审查其兼容性后更新声明和锁。MyGo vendor 484 个文件与上游 v0.5.5 字节一致，go mod vendor 未产生差异；依赖、打包工具、第三方声明与应用各入口版本均已统一。

本地 `pwsh -NoProfile -File release/build.ps1` 完整通过，未使用 SkipTests。Rust 46、后端集成 66、TUI 源码 97 和压缩发行运行时同一套 97 项全部通过且无跳过。fmt / Clippy、go vet / mygo vet、Go 全量（含测量项；专项 JSON 记录含子测试 103 项）通过；vendor UI 命令成功但上游包无测试文件。20 题三基线共 60 次通过；场景 9 次符合预设结果（7 通过、2 个预设首轮失败）。源码、Git 历史与产物审计通过。

包内原生窗口、CLI / Git / TUI 烟测，原公钥签名更新、篡改拒绝、同版本忽略与更新后运行，以及中文／空格路径 NSIS 安装、快捷方式和卸载全部通过。用户已人工确认 Desktop 首页／对话／审批／设置／主题／Mica／拖动缩放，以及 Windows Terminal 40／120 列、字符、Diff、审批键、Shift+Tab 和 ? 面板无明显问题。

在实际用户安装目录先安装 2.0.3，再以 3.0.0 覆盖升级；原有 33 个会话、数据库各表、配置、指令及界面偏好内容哈希不变。原库记忆为空，另用 2.0.3 写入非空记忆和会话，3.0.0 打开后记忆及消息／事件内容完全一致。已备份原用户数据；安装后的运行版本及关于页 3.0.0 / MyGo 0.5.5 已核对。

本地安装器 58,278,621 B，SHA256：`6661b69485c609124bf431c9330f9498925b5f9eadb2f46509ddc3c3e823edd8`。公开 [v3.0.0](https://github.com/oysterhyd/minicode/releases/tag/v3.0.0) 的七个资产已匿名实际下载，逐项大小和 SHA256 与 GitHub 摘要相符，SHA256SUMS 匹配；完整归档及三个差分包通过原 Ed25519 公钥验签，匿名 latest 清单字节一致并指向 3.0.0。[发布 main CI](https://github.com/oysterhyd/minicode/actions/runs/39024031525)、[标签 CI](https://github.com/oysterhyd/minicode/actions/runs/39024031497) 各五项及[完整 Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/39024031484)全部通过。公开安装器再次实际安装，运行、会话／记忆保留及关于页通过；安装器 58,277,114 B，SHA256：`22c6689a893ee8ab020c74cc58c8ebce05531022d4cf9e7c535efb74b4a6c49e`。

# v2.0.3 代码审查核验

2026-10-13，本地 Rust 46 项、后端集成 66 项、Ink TUI 88 项全部通过；TUI 包含真实 Rust 桥接，无跳过。Rust fmt / Clippy、Go 全量 / vet / vendor UI 专项和原生 Windows 编译通过。实际原生审批窗口已隔离捕获并检查。

20 题 B0 / B1 / B2 共 60 次通过；3 个场景共 9 次符合既定结果（7 次通过，counter_retry 的 B0 / B1 为预设首轮失败）。原始运行结果位于本地 output/audit-2.0.3；逐项触发条件和修复行为见 [2.0.3 修复说明](../docs/fixes-2.0.3.md)。源文件与 Git 历史凭据审计通过，并新增签名私钥泄漏拒绝复现。发行构建沿用原 Ed25519 公钥和仓库中的签名 Secret。

运行源码 `c00deeb66bc01c4c08cc00d79c0535b3ed7dbc92`。[main CI](https://github.com/oysterhyd/minicode/actions/runs/38354297404) 和 [标签 CI](https://github.com/oysterhyd/minicode/actions/runs/38354302665)各五项全部通过。[完整 Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38354302660)未跳过全套验证，Rust 46 项、后端 66 项、TUI 88 项及生产压缩 JS 的同一套 88 项均通过。Go 全量 / vendor UI、离线任务和场景符合预期。

包内实际原生窗口、CLI / Git / TUI、空白模型配置，中文路径 NSIS 安装、Unicode 快捷方式、卸载，以及真实 SDK 签名更新、篡改拒绝、同版本忽略和更新后运行均通过。发行过程中新增了真实 Rust 桥接的受控延迟复现，修复后台状态读操作恢复已清除会话的竞态。

公开 [v2.0.3](https://github.com/oysterhyd/minicode/releases/tag/v2.0.3) 七个资产已实际下载，大小和 SHA256 与 GitHub 资产摘要一致；完整归档和三个差分包的原 Ed25519 公钥签名通过，匿名 latest 清单一致。下载归档内 Agent 实际运行确认为 2.0.3、协议 2、全新模型配置为空。安装器 57,759,517 B（55.08 MiB），归档内安装内容 217,869,328 B；安装器 SHA256：`168bd1852ab61a064282cec749bd02addf62e88f0e1595ca70acb8e868255ec6`。完整资产、签名、运行与 CI 记录见 [原始核对记录](../docs/assets/fixes-2.0.3.json)。

# v2.0.2 性能与发行核验

运行源码 `d7cc31e0aae20896ac652c9bc4f270e75bbd4c96`。[main CI](https://github.com/oysterhyd/minicode/actions/runs/38273721751) 与 [标签 CI](https://github.com/oysterhyd/minicode/actions/runs/38273725828)各五项通过。[完整 Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38273725954)通过 Rust 32 项、后端 47 项、Ink 84 项；压缩发行 JS 的同一套 84 项也全部通过。Go 全量、vendor UI 专项、静态检查、60 次任务与 9 次场景评测符合预期。

包内窗口、CLI / Git / TUI、空白模型配置，Unicode NSIS 安装／快捷方式／卸载，以及真实 SDK 签名更新、篡改拒绝、同版本忽略和更新后运行均通过。40 张浅／深色软件快照中，38 张逐像素一致；两张关于页差异只在版本字符区域。

公开七个资产已下载核对大小和 SHA256；完整归档与三个差分包通过原公钥签名，匿名 latest 清单一致。安装器 57,711,411 B（55.04 MiB），安装内容 217,748,604 B（207.66 MiB）。安装器 SHA256：`e37c29603924851b94338e4732cc97a60624a83fdc9a7f58363f51e44685b01d`。[完整审查、测量口径与结果](../docs/performance-2.0.2.md)，[原始记录](../docs/assets/performance-2.0.2.json)。

# v2.0.1 回归修复核验

Rust 28 项、后端集成 46 项、Ink 83 项、Go 全量与 go vet、文件预览浅／深色布局回归通过。[main CI](https://github.com/oysterhyd/minicode/actions/runs/38261358664) 与 [标签 CI](https://github.com/oysterhyd/minicode/actions/runs/38261365208)各五个任务通过。

[完整发行构建](https://github.com/oysterhyd/minicode/actions/runs/38261364660)通过全套回归及离线评测、中文路径安装／快捷方式／卸载、包内实际窗口／CLI／TUI／评测运行，以及真实 SDK 签名更新、篡改拒绝与同版本忽略。

公开七个资产已实际下载，大小／SHA256 与 GitHub 摘要一致；完整归档及三个差分包的原公钥签名通过，匿名 latest 更新清单为 2.0.1。安装器 57,944,024 B，SHA256：`444030e66d575464683b9d3ea974a8a6ecefbd95d75eb55d3375f38430d9da00`。修复原因、范围及资产记录见 [2.0.1 说明](../docs/fixes-2.0.1.md)。

# v2.0.0 Rust 迁移核验

Rust 单元 25 项、真实后端集成 43 项、Ink TUI 82 项均通过，Clippy 无警告。Go 全量、vendor UI 专项及 go vet 通过。[main CI](https://github.com/oysterhyd/minicode/actions/runs/38253516602) 和 [v2.0.0 标签 CI](https://github.com/oysterhyd/minicode/actions/runs/38253521389)各五个任务全部通过。20 题 B0/B1/B2 共 60 次通过，3 个补充场景结果符合预期（7 通过、2 个预设首修失败）。

20 个状态、浅／深色共 40 张原生快照中，38 张逐像素一致；两张关于页仅版本字符不同，差异框为 [1155,277]–[1180,285]，各 161 像素。HTML 报告与旧版固定样本一致。源码及 Git 历史凭据审计通过。

[完整 Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38253521384)通过全套测试、包内实际窗口 / CLI / Git / TUI / 评测烟测、中文路径 NSIS 安装 / Unicode 快捷方式 / 卸载，以及真实 SDK 签名更新、篡改拒绝、同版本忽略。完整归档和三个差分包的原 Ed25519 公钥签名均已核验。实际 v1.3.2 Python 运行环境到 2.0.0 的更新对照通过；旧 bridge 继续响应，新版启动后清除旧运行目录。

安装器 57,924,627 B（55.2412 MiB），较 v1.3.2 减少 32.86%；安装内容 221,227,365 B（210.9789 MiB），减少 35.82%。Agent 空闲工作集减少 90.33%，25 轮离线执行耗时减少 50.31%；全部条件、指标和原始样本见 [迁移报告](../docs/rust-agent-2.0.0.md)。

公开 [v2.0.0](https://github.com/oysterhyd/minicode/releases/tag/v2.0.0) 七个资产已下载核对，GitHub 资产大小与 SHA256、完整／差分签名及匿名 latest 清单一致。公开 Agent SHA256 为 `0b61cbcda6ae6505368ba692da300850893c025e85a5929f41b143573d5830e9`，已直接重测全部七次样本。安装器 SHA256 为 `d827c81afedfbc9b980e8139c51a999fb82e5586a1998a734dca87e7d7c1b168`。

以下保留旧发行历史核验。

# v1.3.2 核验

2026-10-12，发行代码 `7802b9d5d5acf131b0705eeb3286695d974a6ca2`。逐功能审查、代码简化与前后指标见 [1.3.2 性能报告](../docs/performance-1.3.2.md)。

- 本地 Python 617 通过、3 跳过；TUI 82 通过、0 跳过，含实际 Python 桥接；TypeScript 构建通过。
- Go 全量测试、`go vet ./...`、vendor UI 专项与 Windows 编译通过。20 个主要状态、浅／深色共 40 张原生软件快照中，38 张完全一致；两张关于页差异仅位于版本字符区域。
- [主分支 CI](https://github.com/oysterhyd/minicode/actions/runs/38209612252)与[发行标签 CI](https://github.com/oysterhyd/minicode/actions/runs/38209612121)各七个任务全部通过。
- [Windows 完整发行构建](https://github.com/oysterhyd/minicode/actions/runs/38209612131)通过：干净发行 Python 618 通过、2 跳过；TUI 82 通过；Go 全量与 vendor 专项通过，未跳过完整测试。
- 生产 TUI 删除 2,556 个类型声明／source-map 文件，4,728,511 B。包内真实 TUI 主入口、Python 桥接、CLI / Git、B0/B2 离线评测、空白配置及原生窗口烟测通过。
- 中文路径 NSIS 安装、Unicode 开始菜单、实际安装后运行与卸载通过。SDK 签名更新、篡改拒绝、相同版本忽略、更新后烟测及保留已加载 SSL / SQLite 的旧 Python 进程通过。
- 发行源码及历史审计通过，完整运行目录 12,855 文件的个人文件／已知凭据审计通过。
- 七个公开资产实际下载并核对大小和 SHA256；完整归档及 1.2.7 / 1.3.0 / 1.3.1 三个差分包均通过原公钥 Ed25519 核验。
- 包内 bridge、版本、Python 输出裁剪／搜索／SQLite／压缩模块及 TUI 编译模块与发行源码逐文件一致（规范化 CRLF / LF）；TUI 开发元数据确已移除。
- 公开 latest 更新清单与已验证清单摘要相同，版本为 1.3.2。[v1.3.2 已公开发布](https://github.com/oysterhyd/minicode/releases/tag/v1.3.2)。
- 安装包 86,275,104 B（82.2784 MiB），较 1.3.1 下降 0.3835%；安装内容 344,713,612 B（328.7445 MiB），下降 1.3508%。安装包 SHA256：`f4ad03da60ef502adcf06cc8e27252c26640a469eaed704eaf94d13a404a673c`。

## v1.3.1 核验

2026-10-12，代码简化与性能优化；完整测量见 [性能报告](../docs/performance-1.3.1.md)。

- 本机 Python 完整测试 605 通过、3 跳过；TUI 81 通过，TypeScript 构建通过。
- 原生 Go 完整测试、静态检查、Windows 编译及 vendor 动效专项测试通过。该专项进入跨平台 CI 和发行构建。
- 开始页、对话、模型设置、权限设置、模型菜单的浅色／深色共 10 组前后窗口截图像素一致。
- 真实原生窗口三个动画场景各测量三次；运行指示场景 3 秒内重建次数中位数 170 → 3，长流式场景绘制间隔 p95 20.58 → 17.56 ms。不是显示器 FPS 测量。
- [Windows 完整发行构建](https://github.com/oysterhyd/minicode/actions/runs/38196634183)通过：干净发行环境 Python 606 通过、2 跳过，TUI 81 通过；原生完整测试与 vendor 动效专项通过。
- 包内空白配置、真实原生窗口、Python 桥接、CLI / Git / TUI 运行，中文路径 NSIS 安装、Unicode 快捷方式与卸载通过；签名更新、拒绝篡改、相同版本忽略及更新后运行检查通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38196634202)七个任务全部通过；同一发行标签的 [CI](https://github.com/oysterhyd/minicode/actions/runs/38196634211)也全部通过。
- 本机源码 1558 文件及 Git 历史已审计；发行环境 15411 文件的个人文件／已知凭据审计通过。包内 bridge、统计、SQLite、附件、任务图、技能元数据与版本源码和发行标签逐文件核对一致。
- 安装包、完整更新归档及 1.2.3 / 1.2.7 / 1.3.0 → 1.3.1 三个差分包已实际下载，大小与 SHA256 均和 GitHub 资产摘要一致；完整／差分包的 Ed25519 签名均通过原有公钥核验。
- [v1.3.1 已公开发布](https://github.com/oysterhyd/minicode/releases/tag/v1.3.1)并设为最新版本；公开 latest 更新清单与已核验的清单逐字节摘要一致。安装包 SHA256：`b2ed0b11f1b2ad67a46c8102da27a247a980c7ee85b746f16c662efab86bb23c`。

## v1.3.0 核验

2026-10-11，中英文界面、本地扩展启停、全局 AGENTS.md 和工具图片结果。

- 原生完整测试、Go 静态检查与 Windows 编译通过；新增中英文切换、偏好保存、指令编辑、资源搜索／筛选／启停和图片显示的原生交互验证。英文浅色／深色设置截图与图片缩略图检查通过。
- 只读参考本机 Pi Desktop 的共享资源扫描及插件 manifest 格式；扫描不启动命令或连接网络，导入开关仅保存 MiniCode 配置。真实 stdio、HTTP、SSE MCP 连接与调用通过；检查来源文件不变及鉴权信息不进入错误输出。
- 验证已加载技能停用后仍能恢复会话；稳定技能名称、重新启用后的内容指纹检查、无效 MCP 配置隔离通过。
- 全局指令验证 UTF-8、64 KB 上限、原子保存、请求之间重载和恢复会话；项目指令保持范围与指纹检查。未选择工作区时也可编辑指令、启用全局资源及测试 MCP。
- 实际适配器配合 HTTP 模拟验证 read 图片进入模型请求、MCP 图片结果保留、薄引用持久化及源文件删除后恢复；不支持图片的模型和失败读取不会显示图片。未调用付费在线模型。
- [Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38138647742) 通过：Python 603 通过、2 跳过，TUI 79 通过；包内空白配置、原生窗口、Python 桥接和 CLI / Git / TUI 烟测通过。
- 中文路径安装、Unicode 快捷方式、卸载、签名更新、篡改拒绝和更新后运行检查通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38138647731) 七个任务全部通过。
- 实际下载的安装包 SHA256 与 GitHub 资产摘要及 SHA256SUMS 一致；完整归档和 1.2.2 / 1.2.3 / 1.2.7 差分包的资产摘要与 Ed25519 签名核对通过。
- [v1.3.0 已发布](https://github.com/oysterhyd/minicode/releases/tag/v1.3.0)，公开最新更新清单与已验证清单一致。安装包 SHA256：`b74c24d28c0aa1fa580864dc30dcd6bccc7ad775654166e734c71536c58e3280`。

## v1.2.7 核验

2026-10-11，四类附件输入、侧栏新建对话与流式思考内容显示。

- 原生完整测试、Go 静态检查和 Windows 编译通过；浅色 / 深色附件界面检查通过，修复添加附件图标。
- 实际适配器配合 HTTP 模拟验证图片 / PDF / 音频 / 视频的请求内容、能力拦截、文件完整性、源文件删除后的缓存发送和恢复会话后的再次发送；未调用付费在线模型。数量与大小限制逐条消息校验，历史附件不错误累积到单条消息上限。
- 原生交互验证附件导入、移除、排队发送、切换能力后的草稿保留与历史关联。思考内容验证流式显示、取消、跨会话路由和持久化恢复。
- [Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38109612697) 通过：Python 578 通过、2 跳过，TUI 79 通过；包内空白配置、真实原生窗口、Python 桥接、CLI / Git / TUI 烟测通过。
- 中文路径安装、Unicode 快捷方式、卸载、签名更新、篡改拒绝与更新后运行检查通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38109612598) 七个任务全部通过。
- GitHub 安装包资产摘要与 SHA256SUMS 一致；完整更新归档及 1.2.1 / 1.2.2 / 1.2.3 差分包的实际资产摘要与 Ed25519 签名核对通过。
- [v1.2.7 已发布](https://github.com/oysterhyd/minicode/releases/tag/v1.2.7)，公开最新更新清单与已验证清单一致。安装包 SHA256：`deb1d36c2cacd4bc0122e5e3d859b862e9ee0d57d800a8d465754122d79eb793`。

## v1.2.3 核验

2026-10-10，修复计时与执行栏布局、生成速度统计及模型配置 / 选择菜单。

- 原生测试、Go 静态检查与 Windows 可执行文件编译通过；最小窗口、大字号、长模型列表、完整大数值输入和固定保存操作通过交互检查。
- 定时 SSE 验证文本、推理与工具参数：首字等待 5 秒、生成 2 秒、输出 600 token 时生成 TPS 为 300；整包缓冲不推算生成速度。历史记录保留请求整体速度，混合样本按生成时长加权，恢复会话后统计保持一致。
- [Windows 发行构建](https://github.com/oysterhyd/minicode/actions/runs/38036968805) 通过：Python 569 通过、2 跳过，TUI 79 通过；包内空白配置、真实原生窗口、桥接、CLI / Git / TUI 烟测通过。
- 中文路径安装、Unicode 快捷方式、卸载、签名更新、篡改拒绝和更新后运行检查通过。
- [跨平台 CI](https://github.com/oysterhyd/minicode/actions/runs/38036968348) 七个任务全部通过。
- GitHub 安装包摘要与校验文件一致；完整更新归档及 1.2.0 / 1.2.1 / 1.2.2 差分包的实际资产摘要与 Ed25519 签名核对通过。
- [v1.2.3 已发布](https://github.com/oysterhyd/minicode/releases/tag/v1.2.3)，公开最新更新清单与已验证清单一致。安装包 SHA256：`d60f6f3ba4e22e0e03c2f3592dfcd55e0a2078973a466b4515a6ca7271bbeab8`。

## v1.2.2 核验

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
