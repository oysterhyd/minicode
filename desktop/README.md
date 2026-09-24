# MiniCode Desktop

本地三栏 Coding Agent 工作台。Electron 主进程负责窗口、工作区文件与 Git；React 渲染进程通过受限 preload IPC 调用主进程；Python NDJSON 桥接进程直接复用现有 `AgentRuntime`、SQLite 会话、工具和审批策略。

## 运行

在仓库根目录先安装 Python 底层依赖：

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -e .
```

然后启动桌面端：

```powershell
cd desktop
npm install
npm run dev
```

`npm run build` 构建渲染进程，`npm start` 构建后启动 Electron。应用优先使用仓库的 `.venv`，否则使用系统 Python。会话直接保存在底层现有的 `~/.minicode/sessions.db`；最近选择的工作区保存在 Electron 用户数据目录。

真实模型沿用底层已有的 `COMMANDCODE_API_KEY` / 本机 ZCode 配置或 `ANTHROPIC_API_KEY`。未配置时可选择 `fake` 查看离线交互，但它不执行真实编码任务。`read` 等只读工具直接执行，`edit`、`write`、`bash` 在工作台中逐项请求批准。

输入框使用 `@` 从工作区文件列表补全路径；运行期间发送的提示词会按顺序排队。左右栏可收拢且分别滚动，中间输入框固定在底部。右栏提供 Git 改动 Diff、文件预览、Agent 终端输出、当前会话的 TODO 和真实用量摘要。Diff 的“暂存并确认”会对所选文件执行 `git add`。

桌面端读取与 TUI 共用的 `/` 命令目录，支持 `/help`、`/model`、`/effort`、`/permissions`、`/clear`、`/new`、`/compact`、`/skill`、`/sessions`、`/resume`、`/continue` 和 `/exit`。输入 `/` 可用方向键、Tab 和 Enter 选择命令。输入框下方的模型与权限按钮会直接更新当前 Agent 运行状态。

设置提供模型、推理预算、权限模式、回合/Token/时长预算、YAML 验收文件、Skills 激活/停用、MCP 发现、项目插件启用/停用及锁定、只读子助手目录和运行记录。验收文件需在新会话开始前设置。Agent 注册了与 CLI/TUI 相同的 Skills、MCP、任务、记忆、背景命令及 `delegate` 工具。启用或停用插件会更新工作区 `.minicode/plugins` 中对应的 `plugin.json` 和 `.minicode/plugins.lock.json`，下一回合重载工具。

## 界面与交互

三栏界面统一使用暖白/石墨灰主题与陶土橙强调色。使用本机 Segoe UI / 微软雅黑及 Cascadia Code 字体，无远程字体请求。首次跟随系统主题，手动主题与侧栏收拢状态保存在本机。首页输入区紧邻任务入口，任务开始后移至底部；回复支持 Markdown、代码块与复制，流式输出在向上阅读时停止自动滚动。

- `Ctrl K`：搜索操作与最近任务。
- `Ctrl N`：新建任务；`Ctrl ,` / `Ctrl I`：设置。
- `Ctrl B` / `Ctrl Shift B`：收拢左右面板。
- `Ctrl .`：停止当前任务，`Ctrl C` 保留复制行为。
- 输入 `/` 或 `@` 后使用方向键、Tab、Enter 选择；Esc 关闭建议并保留草稿。
- 运行中可追加排队消息，也可逐项取消。权限审批、文件读取和暂存提供状态反馈。
- 每个会话都有自己独立的 Agent 运行实例，任务运行期间可以自由切换会话；后台会话的进度、审批和结果保持在原会话中，侧栏用图标标出运行中与等待审批的会话。

动效覆盖选中项、工作台标签、弹窗进入/退出、任务输入区位置变化和运行状态；遵循系统“减少动态效果”偏好。弹窗管理焦点与键盘循环，工作台标签支持左右方向键。

## UI 回归检查

在 `desktop` 目录运行 `npm test` 可检查主进程关闭时序，包括窗口销毁后的迟到消息、桥接退出、待处理 IPC 请求清理和重复退出。

先启动 `npm run dev`，然后在仓库根目录执行以下命令。检查通过测试专用 IPC 数据运行，不调用真实模型或修改项目文件。

```powershell
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui open about:blank
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui run-code --filename desktop/tests/ui-fixture.cjs
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui run-code --filename desktop/tests/ui-checks.cjs
```

检查覆盖深浅主题、1030×680 / 1280×800 / 1920×1080 窗口、命令面板、键盘补全、焦点管理、文件搜索与预览、队列、审批和流式滚动。截图保存在 `output/playwright/`，该目录不提交到仓库。

## 目录

```text
desktop/
  electron/main.cjs       Electron 窗口、IPC、文件与 Git 操作
  electron/preload.cjs    受限渲染进程 API
  bridge.py               Python Agent 双向 NDJSON 事件桥
  src/                    React 三栏组件与样式
  package.json            开发、构建、启动脚本
```
