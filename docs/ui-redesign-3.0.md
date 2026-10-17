# 界面重设计：MyGo 0.5.5、Desktop 与 TUI

本次只改界面层与其依赖，Rust Agent、NDJSON v2 协议、会话存储和发行脚本保持不变。

## MyGo 0.5.5

- `desktop/native/go.mod` 升级到 `github.com/egoist/mygo v0.5.5`，`vendor/` 使用上游原样代码；
  0.5 已内置的能力不再需要 `toolkit/*.go.txt` 补丁，`refresh-vendor.ps1` 只做 `go mod vendor`。
- 按 `mygo migrate-ui` 迁移 UI 代码：输入框与命令面板改用 `OnTextKey`，补全保留独立的撤销步骤；
  光标、脉冲、火花和加载动画改为只重绘（paint-only）的 `Draw` 帧，按压反馈使用 `AnimatePose`。

## Desktop

- 配色与字体：纸色 / 墨色两套表面，陶土色只用于发送按钮、Logo 与火花；回复与标题使用衬线体，
  对话字体可在“设置 → 通用”选择衬线、无衬线或系统字体。
- macOS 风格：输入框、弹出菜单、提示、命令面板与“回到最新”胶囊使用液态玻璃材质和大圆角；
  侧栏与标题栏背后使用 Windows 11 Mica（“设置 → 通用 → 窗口 → 半透明侧栏”可关闭，Windows 10 不支持 Mica，显示实色背景）。
- 外壳：标题栏侧栏开关、滑动选中胶囊、按项目分组、首页问候与建议、悬浮工具条。
- 对话流：带步骤时间线的活动卡片、思考中火花与流光、带快捷键提示的审批卡片、重做的 Diff。
- 设置：衬线标题侧栏（支持搜索）、Apple 分组列表、外观与字体卡片、弹簧分段控件、iOS 开关、减少动态效果。
- 工作面板：带计数的分段标签和分组任务列表。所有新文案都有英文翻译。

## TUI

布局与交互参照 Claude Code：

- 欢迎面板：圆角边框标题 `MiniCode vX`，左侧问候、像素吉祥物、模型与工作区，右侧“快速上手”和“最近活动”；窄终端改为单栏。
- 对话流：`> ` 用户消息灰底条；`●` 助手回复；`● Bash(…)` 工具行（运行中圆点闪烁，成功绿、失败红），
  结果以 `⎿` 缩进，默认 3 行，`… +N 行 (ctrl+o 展开)`；连续读取 / 搜索合并为一行摘要。
- Diff：Edit / Write 解析核心返回的统一 Diff，显示“已更新 path，新增 N 行，删除 M 行”、行号、红绿整行色带和词级高亮。
- 审批：底部输入区换成审批面板（标题、命令或 Diff 预览、“是否继续？”三个选项），`↑` `↓` `Enter` 选择，
  `1` `2` `3` / `Y` `A` `N` 直接回答，`Esc` 拒绝。
- 运行中：`✻ 推敲中… (12s · ↓ 1.2k tokens · ctrl+c 中断)` 带流光动画，下面一行提示；排队消息以 `> …（已排队 N 条）` 显示。
- 输入区：上下分隔线、`> ` 提示符；`/` 命令菜单显示在输入框下方；空输入按 `?` 显示快捷键面板。
- 底栏：左侧 `? 查看快捷键`，或 `⏵⏵ 自动接受编辑 (shift+tab 切换)`；右侧模型、推理预算和上下文占用。`Shift+Tab` 在默认与自动接受编辑之间切换。
- 配色使用 Claude Code 的终端色板，浅色终端按 `COLORFGBG` 自动切换，也可设 `MINICODE_THEME=light|dark`。

## 验证

```bash
cd desktop/native && go vet ./... && go test -count=1 ./... && GOOS=windows go build -o minicode-desktop.exe .
npm ci --prefix tui && npm run build --prefix tui && npm test --prefix tui
cargo build --release && (cd tui && MINICODE_TUI_BRIDGE_TEST=1 npx tsx --test tests/audit-bridge.integration.test.tsx)
```

Windows 安装包仍由 `desktop/native/build.ps1` 与 `release/` 在本地生成，本次不包含任何构建产物。
