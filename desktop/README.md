# MiniCode Native Desktop 1.2

MyGo 原生三栏 Coding Agent 工作台。窗口和控件由 Go 构建，Windows 使用 Direct3D 11 绘制；Python Agent 通过 NDJSON v2 继续提供模型、工具、权限、Skills、MCP 和持久会话。生产桌面端不启动 Electron、Chromium 或 WebView2。

Windows 安装包包含 Python、Git，以及 CLI / Ink TUI 所用的 Node.js。安装、构建和校验见 [发行说明](../release/README.md)，实测数据见 [迁移验收与性能报告](../docs/native-desktop-report.md)。

## 源码运行

需要 Windows、PowerShell 7、Go 1.27.1，以及安装了项目依赖的 Python。原生桌面端开发不需要 Node.js 或 Bun；开发终端 TUI 时仍需 Node.js。

在仓库根目录执行：

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -e ".[dev]"
pwsh -NoProfile -File desktop/native/dev.ps1
```

编译原生可执行文件：

```powershell
pwsh -NoProfile -File desktop/native/build.ps1
# output/native/MiniCode.exe
```

也可以沿用 `npm run dev --prefix desktop` 和 `npm run build --prefix desktop`，这两个命令现在调用原生 PowerShell 脚本。Go 依赖及必要的工具包扩展已 vendored；源码编译不需要先下载 MyGo CLI。

## 对话与工作台

- 侧栏按置顶和工作空间分组，支持搜索、重命名、置顶、删除确认和后台任务标记。任务可并行运行，事件按 clientKey / sessionId 路由。
- 回复流式显示 Markdown、GFM 表格、任务列表、链接、图片和高亮代码块。连续工具调用组成可展开的操作分组，编辑显示 Diff，命令输出和归档输出可继续展开。
- 输入支持中文组合输入、原生选择与撤销、`@` 文件补全、`/` 命令、文件拖入、历史输入和运行中排队。停止或暂停后保留队列，成功结束后按顺序执行。
- 审批展示命令、编辑 Diff 或写入内容。`Y` 批准，`A` 本会话始终允许，`N` 拒绝；自动批准范围可在权限设置中撤销。
- 工作台提供 Git 改动和暂存、文件树与高亮预览、终端记录、任务进度，以及 token、缓存与 TPS 统计。所有文件操作都校验工作区路径边界。
- 面板可拖拽调宽、双击恢复默认、键盘调节和收拢。浅色、深色及跟随系统模式沿用原版颜色；动效遵循减少动态效果偏好。弹层管理焦点、Escape 和键盘循环。

设置包含通用、模型与预算、权限、技能、MCP、插件、子助手、快捷键、运行记录和关于。AI 服务支持 OpenAI Chat Completions 与 Anthropic Messages；可管理同 ID 的不同服务模型、思考强度、默认模型、预算和验收命令。网络请求在后台执行，取消与审批不会等待其他请求完成。

模型和思考强度在下一次请求生效；权限和技能用于后续操作；预算、插件与 MCP 在下一回合生效；验收配置用于新会话。TPS 为输出 token / 完整请求耗时，包含首字等待，缺少数据时显示“未报告”。

## 数据与升级

Python 配置和会话继续位于 `%USERPROFILE%\.minicode`，服务密钥仅由 Python 持久保存。旧版 Electron 的 workspace.json 和 Chromium Local Storage 偏好会从临时 LevelDB 副本导入，不改写旧文件。主题、缩放、面板状态、发送方式和输入历史随之迁移。

原生偏好保存在 MyGo 用户数据目录中的 `native-preferences.json`，窗口位置、尺寸和最大化状态由 MyGo 保存。写入合并后落盘。安装包不携带个人配置或会话。

安装版使用 MyGo 官方 `updater/native`。“设置 → 通用 → 应用更新”默认启用自动检查：启动约 10 秒后检查，随后每 24 小时检查一次。可选“自动下载并安装”默认关闭；开启后更新在下次启动生效，当前任务继续运行。通用页和关于页均有“检查更新”按钮，更新提示、进度和重新启动使用原生窗口。

更新来自 `oysterhyd/minicode` 的 GitHub Releases，SDK 使用嵌入的 Ed25519 公钥验证更新归档。开发构建或不可写的安装目录会禁用更新并显示原因。首次从 Electron 1.1.0 迁移需手动安装原生版，以后的签名原生版本可以应用内升级。构建签名及升级验收见 [发行说明](../release/README.md#自动更新与签名)。

插件沿用精确宿主版本契约。升级至 1.2.0 后，现有插件 manifest 的 `minicode_version` 也需要更新为 `1.2.0` 并重新锁定；仓库示例已同步。

## 快捷键

| 快捷键 | 操作 |
| --- | --- |
| Ctrl K / Ctrl N / Ctrl , | 快捷操作 / 新任务 / 设置 |
| Ctrl B / Ctrl Shift B | 收拢左右面板 |
| Ctrl 1–4 | 切换工作台标签 |
| Ctrl Shift F | 搜索任务 |
| Ctrl . | 停止任务 |
| Ctrl Shift [ / ] | 切换任务 |
| Ctrl Shift L / Ctrl L | 切换深浅主题 / 清空视图 |
| Ctrl / | 快捷键列表 |

## 测试与证据

```powershell
cd desktop/native
go test -count=1 ./...
go vet ./...
```

测试覆盖原生组件输入、组合状态、补全与撤销、排队移除、取消、弹层、菜单、会话路由、流式更新、Markdown、图片边界、Git 服务、更新设置和真实 Python 桥接。快照测试生成 18 张浅色/深色截图到 `output/native/`；最小窗口和 150% 缩放另有布局检查。

实际窗口的隔离 fixture 和性能入口：

```powershell
output/native/MiniCode.exe --fixture --fixture-page conversation --theme dark --capture output/native/window.png
output/native/MiniCode.exe --benchmark output/native/ui-latency.json
```

`--fixture` 使用独立临时偏好和测试数据，不请求真实模型。`--benchmark` 测量状态/事件进入到原生视图构建的耗时；不代表模型响应或屏幕最终显示时间。最终包内 Python、CLI、Git、TUI 和窗口烟测使用 `release/smoke-native.ps1`。

## 实现目录

```text
native/main.go              原生窗口、单实例、生命周期
native/state.go             多会话控制器、事件批处理与任务队列
native/shell.go              标题栏、侧栏、欢迎页与快捷键
native/feed.go               消息、工具、审批与归档输出
native/composer.go           输入、补全、模型/权限/上下文弹层
native/markdown.go           Markdown、代码高亮与渲染缓存
native/work.go               Git、文件树、预览与用量统计
native/settings.go           服务、模型、权限与扩展设置
native/updates.go            官方原生更新窗口和更新偏好
native/internal/bridge       Go → Python NDJSON 客户端与进程生命周期
native/internal/workspace    Git 与文件服务
native/internal/content      Myers Diff 与统一差异解析
native/toolkit               可重放的 MyGo 编辑、动效与帧调度扩展
bridge.py                    共享 Python AgentRuntime 桥接
```

升级 MyGo 时运行 `native/toolkit/refresh-vendor.ps1`，审查扩展插入点并重新测试。该脚本恢复 caret 补全、按压缩放、列表横向滚动及 Windows 响应帧；动画按显示器刷新率调度，DWM 完成窗口合成，UI 线程不等待垂直空白。

旧 `electron/`、`src/` 和 Vite 依赖保留用于迁移对照，`dev:legacy`、`build:legacy`、`test:legacy` 显式运行旧实现。它们不进入原生安装包。
