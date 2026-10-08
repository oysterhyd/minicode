# Desktop 1.2.0 原生迁移与验收

2026-10-08，Windows x64，PowerShell 7。Desktop 从 Electron / React 改为 MyGo 0.2.15 / Go 1.27.1 原生 UI，Windows 使用 Direct3D 11；Python Agent、CLI 和 Ink TUI 继续复用。发行包内置 Python 3.12.12、Git 2.56.0 和 Node.js 22.22.0。

## 迁移结果

保留三栏工作台、深浅主题与布局偏好、任务管理、Markdown 和代码 Diff、审批、模型与权限弹层、Git 暂存、文件预览、任务/终端/用量页以及服务、模型、Skills、MCP、插件和子助手设置。旧 Electron workspace.json 和 Chromium Local Storage 从临时副本导入；Python 密钥及 SQLite 会话继续使用原位置。

原生控件支持补全、撤销、组合输入状态、文本选择、键盘循环、上下文菜单和排队任务。修复代码块横向排列、工具分组、缓存统计、任务进度和上下文展示；图片在后台加载，并限制尺寸、响应大小及工作区边界。窗口、弹层和按钮沿用原版颜色、位置、圆角、模糊和按压反馈。

MyGo 官方 `updater/native` 提供原生更新窗口。“通用”页默认自动检查更新，自动下载安装可选；“关于”页可手动检查。签名、升级与发布规则见 [发行说明](../release/README.md#自动更新与签名)。首次从 Electron 1.1.0 迁移需下载一次原生安装包，后续可应用内升级。

[v1.2.0 已发布](https://github.com/oysterhyd/minicode/releases/tag/v1.2.0)。发布后使用实际 MyGo SDK 请求公开 `releases/latest/download/update-windows-amd64.json`，确认新版本可发现、相同版本被忽略；从公开地址下载完整更新归档后验证签名，并核对发布安装器的校验值。

## 安装体积

| 产物 | Electron 1.1.0 | MyGo 1.2.0 | 减少 |
| --- | ---: | ---: | ---: |
| Windows 安装包 | 191.3 MiB | 79.1 MiB | 58.7% |
| 干净安装内容 | 678.6 MiB | 316.8 MiB | 53.3% |

原生可执行文件 18.1 MiB，签名更新归档 119.3 MiB。本地安装包 SHA256：`5d70863a69aeacd9c6cbfe0147d4c7969ed293c53793909c9c228b0154457d90`。GitHub Actions 重新构建的文件摘要以 Release 的 `SHA256SUMS.txt` 为准。


旧版来自原有 1.1.0 发行产物，基线 commit 为 `b5468576d54d603a353608160fe3f31b3dc5ef3c`。新旧都包含 Python / Git / Node / TUI。解包体积为文件逻辑大小之和，不含安装器、更新归档及清单；不表示磁盘分配大小。更新 `.tar.gz` 使用与 NSIS 不同的压缩格式。

## 性能实测

同机固定输入，2 次预热、7 次计时，表中为中位数。原版算法从保留的 React / TypeScript 实现测量，原生算法调用实际 Go 实现，GC 在计时外执行。重新测量了未修改的旧版算法，初始基线一并保留在 [原始证据](assets/native-performance.json)。

| 算法 | 原版 ms | 原生 ms | 耗时减少 |
| --- | ---: | ---: | ---: |
| 30,000 条 Feed 分组及位置索引 | 0.7122 | 0.2371 | 66.7% |
| 400 行 Diff / 2 处修改 | 0.1245 | 0.0123 | 90.1% |
| 10,000 行 Diff / 3 处修改 | 1.7308 | 0.2577 | 85.1% |


一万行 Diff 在两端均产生 10,003 行、3 行新增和 3 行删除。原生使用有界 Myers Diff，长列表按可见范围构建，Markdown 及语法高亮按内容缓存；避免对每个流式片段重复解析整个历史。

| UI 操作 | Chromium 提交后 rAF，ms | 原生 view 构建，ms |
| --- | ---: | ---: |
| 切换工作台标签 | 7.50 | 4.61 |
| 打开设置 | 22.20 | 6.85 |
| 打开上下文弹层 | 9.50 | 2.29 |
| 流式片段更新 | 58.90 | 10.68 |


界面数字用于检查各自实现的响应路径，**计时终点不同，不能直接当作屏幕显示的提速百分比**。原版在真实 Chromium 的隔离 fixture 中测量事件派发至内容提交后的首个 requestAnimationFrame；原生在实际 Windows 窗口中测量控制器事件派发至 view 构建完成，未包含布局和绘制。均不包含完整动画、显示器呈现、模型网络延迟或首 token 等待。原生流式事件合并窗口为 8 ms，原版为 40 ms。

Windows 状态和输入变化主动请求响应帧，D3D 呈现不阻塞 UI 线程等待垂直空白；动画帧按显示器刷新率调度并由 DWM 合成。`MYGO_FRAME_STATS=all` 实际窗口日志确认 GPU 绘制。MyGo `CapturePage` 则是最后一帧场景的 CPU 截图，不能用截图单独证明 GPU 性能。本次未测输入到光子、真实模型响应、启动或内存的前后变化。

## 验收与范围

- 完整打包 Python 测试：560 通过、3 跳过；Ink TUI：79 通过，包含真实 Python 桥接、审批、取消和历史恢复。
- 原生 `go test -count=1 ./...` 与 `go vet ./...` 通过；`CGO_ENABLED=0` 的 Windows 和 Linux 编译通过。GUI 发行及运行验收只针对 Windows。
- 欢迎、对话、审批、文件、Diff、通用设置、模型设置、子助手设置和关于共 9 种状态，各生成浅色和深色截图，共 18 张；另检查 1030 × 680 最小窗口及 150% 缩放。命令面板、上下文及菜单有独立交互回归。
- 实际原生窗口启动包内 Python 并完成 NDJSON v2 初始化。空白用户目录和仅含包内工具及 Windows 系统目录的 PATH 下，CLI、Git、TUI 桥接和 B0/B2 离线评测 2/2 通过。
- 实际 NSIS 安装到中文和空格路径，核对开始菜单、卸载登记和程序摘要；安装后运行烟测及卸载通过。测试恢复此前的用户快捷方式。
- 真实 MyGo SDK 在中文和空格安装路径中拒绝篡改签名且原程序摘要保持不变；有效签名替换完整运行环境，程序摘要与发行包一致；同版本不重复更新。更新后再跑 Desktop / CLI / TUI 烟测。
- 升级期间保留加载了 SSL / SQLite 的旧 Python 进程，更新后继续执行 SQLite 查询并返回 42，进程未中断。
- 源码、Git 历史和完整运行目录通过凭据及个人文件审计。签名私钥保存在仓库外，并配置为 Actions Secret；只发布公钥、签名和更新归档。
- README 介绍图以实际原生截图为参考重新生图。图中文字和会话来自 fixture，不作为真实模型效果或测试通过的证据。

自动化检查了 UI 状态、布局边界、事件与组合输入状态；物理中文输入法候选窗、所有通知点击、完整动效的逐像素一致性仍未完成手动验收。旧实现的队列移除自动化检查曾失败；原生队列移除独立回归通过。没有把上述未验收项目描述为已验证，也未调用新的付费模型。

## 复现

算法：`node --expose-gc desktop/tests/benchmark-audit.cjs . output/baseline/algorithms.json`；原生在 `desktop/native` 设置 `MINICODE_BENCH_OUTPUT` 后运行 `go test -count=1 -run TestComparableAlgorithmMeasurements`。

界面：旧页面加载 `desktop/tests/ui-fixture.cjs`，通过 Playwright CLI 执行 `desktop/tests/benchmark-ui.cjs`；实际原生运行 `MiniCode.exe --benchmark <绝对输出路径>`。原始数值、工具版本、体积和本地产物摘要保存在 [native-performance.json](assets/native-performance.json)。测试命令与打包流程见 [Desktop README](../desktop/README.md) 和 [发行核验](../release/VERIFICATION.md)。
