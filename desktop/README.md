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

Settings 提供模型、推理预算、权限模式、回合/Token/时长预算、YAML 验收文件、Skills 激活/停用、MCP 发现、项目插件启用/停用及锁定、只读 Subagents 目录和会话 Inspector。验收文件需在新会话开始前设置。Agent 注册了与 CLI/TUI 相同的 Skills、MCP、任务、记忆、背景命令及 `delegate` 工具。启用或停用插件会更新工作区 `.minicode/plugins` 中对应的 `plugin.json` 和 `.minicode/plugins.lock.json`，下一回合重载工具。界面参考 MiniCode 标志与工作台布局，采用黑白灰配色、本机 Anthropic Serif Text 字体和兼容减少动态效果偏好的过渡动画。

## 目录

```text
desktop/
  electron/main.cjs       Electron 窗口、IPC、文件与 Git 操作
  electron/preload.cjs    受限渲染进程 API
  bridge.py               Python Agent 双向 NDJSON 事件桥
  src/                    React 三栏组件与样式
  package.json            开发、构建、启动脚本
```
