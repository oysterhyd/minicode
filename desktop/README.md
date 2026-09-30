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

真实模型可在设置中添加 AI 服务，支持 OpenAI Chat Completions 与 Anthropic Messages 接口。原有 `COMMANDCODE_API_KEY` / 本机 ZCode 配置或 `ANTHROPIC_API_KEY` 仍可使用。桌面端不提供离线演示模型；未配置服务时先在设置中添加。`read` 等只读工具直接执行，`edit`、`write`、`bash` 沿用当前权限模式与审批流程。

## 对话与审批

- 回复按 Markdown 流式渲染，代码块带语法高亮、语言标签与复制按钮，支持 GFM 表格和任务列表。
- 连续工具调用折叠为“已执行 N 个操作”分组；每一步显示状态、目标和耗时。`edit` 显示内联 Diff，`write` 显示高亮预览，`bash` 以终端样式显示命令与输出。
- 审批卡片展示命令、Diff 或写入内容，可“批准执行”(`Y`)、“本会话始终允许”(`A`) 或“拒绝”(`N`)。始终允许的工具只在当前会话内自动批准，可在设置 › 权限中撤销。
- 每回合结束显示用时和操作数；最后一条消息可复制、编辑重发或重新生成。
- 输入框：`/` 命令、`@` 文件模糊补全、拖拽文件引用、空输入框中 ↑/↓ 浏览历史；运行中发送的内容自动排队。模型、思考强度、权限模式和上下文用量环都在输入框工具栏上。

## 任务与工作台

- 侧栏按置顶和工作空间分组，支持搜索、重命名（双击或右键）、置顶和删除（需确认；删除对话记录、事件与归档输出，不影响工作区文件）。运行中、等待审批、有新结果的任务各有标记。
- 工作台包含改动（行数统计、全部暂存、单文件暂存/取消暂存）、文件（文件树、高亮预览、在文件夹中显示、`@` 引用）、终端记录和任务进度与会话用量。
- 左右面板可拖拽调整宽度（双击恢复默认，也可用方向键），宽度与收拢状态保存在本机。
- 窗口不在前台时，任务完成或需要审批会发出系统通知；点击通知跳回对应任务。后台任务完成时显示可跳转的提示。

## 设置

通用（主题：跟随系统/浅色/深色、界面缩放、发送方式、默认展开工具详情、系统通知）、模型与预算、权限、技能、MCP 服务、插件、子助手、快捷键、运行记录和关于。

- 模型按 AI 服务管理，可随时新增、编辑、启停或删除服务；测试连接并获取模型列表，也可手动添加模型 ID。每个模型可配置显示名称、上下文窗口、输出上限与 OpenAI 接口的思考强度支持。不同服务可使用同一个模型 ID。可分别选择当前会话模型和新会话默认模型。
- 子助手提供探索者、代码审查员、测试执行者、修复者与 UI 设计师模板；可复制模板或空白创建，编辑名称、委派条件、指令和工具，支持启停与删除。自定义子助手适用于所有工作区，委派时继承父会话权限、审批处理与共享预算。
- 服务与子助手配置保存在 `~/.minicode/desktop-config.json`。API 密钥仅由桥接进程持有，已有密钥不会返回渲染进程，编辑时留空保留。
- 运行中仍可修改设置。模型与思考强度在下一次模型请求生效，权限与技能用于后续工具调用/请求；预算、插件与 MCP 更新在下一回合生效，验收配置用于新会话。插件更新工具时保留当前会话和后台任务。
- 上下文用量环展示缓存命中率与最近请求 TPS，展开可查看缓存读取/写入与累计用量。任务栏下方包含 token 分布、缓存比例、每轮柱状图、TPS 趋势与请求明细。TPS 按输出 token / 完整请求耗时计算，包含首字等待；缺少用量或历史耗时的指标显示为未报告。

桌面端读取与 TUI 共用的 `/` 命令目录：`/help`、`/model`、`/effort`、`/permissions`、`/clear`、`/new`、`/compact`、`/skill`、`/sessions`、`/resume`、`/continue`、`/exit`。

## 快捷键

- `Ctrl K` 搜索操作、任务与文件；`Ctrl N` 新建任务；`Ctrl ,` 设置；`Ctrl /` 快捷键列表。
- `Ctrl B` / `Ctrl Shift B` 收拢左右面板；`Ctrl 1-4` 切换工作台标签；`Ctrl Shift F` 搜索任务。
- `Ctrl .` 停止任务；`Ctrl Shift [` / `]` 切换上/下一个任务；`Ctrl Shift L` 切换深浅主题；`Ctrl L` 清空当前视图。

动效遵循系统“减少动态效果”偏好。弹窗管理焦点与键盘循环；所有图标按钮都有无障碍标签和悬停提示。

## 安全与发布

- 渲染进程开启 `sandbox`、`contextIsolation`，关闭 `nodeIntegration`；只通过 preload 暴露受限 API。
- 生产构建注入 CSP（仅允许本地脚本）；外部链接仅允许 http/https/mailto 并交给系统浏览器；阻止离开应用页面的导航。
- 文件预览、暂存、打开等操作都校验路径在工作区内。
- 单实例运行；窗口位置、尺寸与最大化状态会被记住。

## 测试

在 `desktop` 目录运行 `npm test`，检查主进程关闭时序、Git 行数统计解析、最近工作区与外部链接过滤。Python 桥接的会话管理与始终允许逻辑见仓库根目录 `tests/test_desktop_bridge_sessions.py`；配置持久化、密钥保留与脱敏、运行中切换模型、插件重载、自定义子助手与统计恢复见 `tests/test_desktop_configuration.py`。

UI 回归检查：先启动 `npm run dev`（或 `npx vite --port <端口>`），然后执行以下命令。检查使用测试专用 IPC 数据，不调用真实模型，也不修改项目文件。fixture 会沿用已打开页面的端口。

```powershell
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui open http://127.0.0.1:5173
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui run-code --filename desktop/tests/ui-fixture.cjs
npx --yes --package @playwright/cli playwright-cli -s=minicode-ui run-code --filename desktop/tests/ui-checks.cjs
```

请使用无头浏览器运行：有界面的 Chrome 会占用 `Ctrl Shift B`（书签栏），Electron 中没有这个问题。截图保存在 `output/playwright/`，该目录不提交。

## 目录

```text
desktop/
  electron/main.cjs        Electron 窗口、IPC、文件、Git 与系统集成
  electron/git-utils.cjs   可单测的 Git 解析与链接过滤
  electron/preload.cjs     受限渲染进程 API
  bridge.py                Python Agent 双向 NDJSON 事件桥
  configuration.py         持久化 AI 服务、模型与全局子助手配置
  src/components/          标题栏、侧栏、对话流、输入框、工作台、设置与通用 UI
  src/hooks, src/lib       会话状态、偏好、格式化、Diff 与语法高亮
  src/styles/              设计令牌与分区样式
```
