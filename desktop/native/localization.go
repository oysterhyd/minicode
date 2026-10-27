package main

import (
	"fmt"
	"regexp"
	"strings"
	"sync/atomic"
)

// UI language is independent of prompts, model output and resource identifiers.
// The native host has one desktop; atomics also make native dialogs safe to read.
var uiEnglish atomic.Bool

func normalizeLanguage(value string) string {
	if value == "en" {
		return "en"
	}
	return "zh"
}

func setUILanguage(value string) { uiEnglish.Store(normalizeLanguage(value) == "en") }

func tr(text string) string {
	if uiEnglish.Load() {
		if translated, ok := englishUI[text]; ok {
			return translated
		}
	}
	return text
}

func trf(format string, args ...any) string { return fmt.Sprintf(tr(format), args...) }

func localizeLabels(labels []string) []string {
	result := make([]string, len(labels))
	for i, label := range labels {
		result[i] = tr(label)
	}
	return result
}

var retryNotice = regexp.MustCompile(`^模型请求暂时失败，([0-9.eE+-]+) 秒后重试（第 ([0-9.eE+-]+) 次）。$`)
var connectedNotice = regexp.MustCompile(`^已连接，发现 ([0-9]+) 个模型$`)
var connectionError = regexp.MustCompile(`^连接失败（HTTP ([0-9]+)）$`)

// Feed notices are recorded in their original form. Translate the application's
// known status templates when rendering, so existing conversations also switch.
// Variable portions (job IDs, tool names and interruption reasons) stay intact.
func localizeNotice(text string) string {
	if !uiEnglish.Load() {
		return text
	}
	if translated := tr(text); translated != text {
		return translated
	}
	if parts := retryNotice.FindStringSubmatch(text); len(parts) == 3 {
		return fmt.Sprintf("Model request failed temporarily. Retrying in %s seconds (attempt %s).", parts[1], parts[2])
	}
	if parts := connectedNotice.FindStringSubmatch(text); len(parts) == 2 {
		return fmt.Sprintf("Connected. Found %s models", parts[1])
	}
	if parts := connectionError.FindStringSubmatch(text); len(parts) == 2 {
		return fmt.Sprintf("Connection failed (HTTP %s)", parts[1])
	}
	for _, pair := range [][2]string{{"后台命令 ", "：已完成"}, {"后台命令 ", "：结果丢失"}, {"任务已暂停：", "。可输入 /continue 继续"}} {
		if strings.HasPrefix(text, pair[0]) && strings.HasSuffix(text, pair[1]) {
			return tr(pair[0]) + strings.TrimSuffix(strings.TrimPrefix(text, pair[0]), pair[1]) + tr(pair[1])
		}
	}
	if rest, ok := strings.CutPrefix(text, "工具执行结果需复核："); ok {
		return tr("工具执行结果需复核：") + rest
	}
	return text
}

// Keep source keys readable beside the native components. Unknown text is left
// intact, including user content, paths, model identifiers and provider errors.
var englishUI = func() map[string]string {
	result := make(map[string]string)
	for _, line := range strings.Split(englishUIText, "\n") {
		if key, value, ok := strings.Cut(line, "|"); ok {
			result[key] = strings.ReplaceAll(value, `\x20`, " ")
		}
	}
	return result
}()

const englishUIText = `排队中|Queued
回复 MiniCode…|Reply to MiniCode…
查看改动|Review changes
%d 处改动|%d changes
模型|Model
上下文 %d%%|Context %d%%
思考了 %s|Thought for %s
思考已中断|Thinking interrupted
正在思考…|Thinking…
%d 行|%d lines
%d 处匹配|%d matches
读取 %d|Read %d
搜索 %d|Searched %d
编辑 %d|Edited %d
写入 %d|Wrote %d
命令 %d|Ran %d
委派 %d|Delegated %d
%.1f 秒|%.1fs
 于 |\x20in\x20
允许 MiniCode 编辑这个文件吗？|Allow MiniCode to edit this file?
允许 MiniCode 运行这条命令吗？|Allow MiniCode to run this command?
允许 MiniCode 写入这个文件吗？|Allow MiniCode to write this file?
允许 MiniCode 使用 %s 吗？|Allow MiniCode to use %s?
信任此工作区的插件并启动其 MCP 服务吗？|Trust this workspace's plugins and start their MCP servers?
信任并启动|Trust and start
协议版本不匹配|Protocol version mismatch
Agent 协议版本 %d 低于桌面端需要的 %d，请升级 MiniCode agent。|Agent protocol %d is older than the %d this desktop needs; please upgrade the MiniCode agent.
Agent 需要协议版本 %d 以上的客户端，请升级桌面端。|The agent needs a client with protocol %d or later; please upgrade the desktop app.
Agent 协议版本 %d 比桌面端支持的 %d 新，部分新功能不可用；请升级桌面端。|Agent protocol %d is newer than the %d this desktop supports; some new features are unavailable. Please upgrade the desktop app.
选择|Select
打开|Open
搜索设置|Search settings
没有匹配的设置|No matching settings
对话字体|Conversation font
MiniCode 回复所用的字体|The typeface of MiniCode's replies
默认衬线|Serif (default)
无衬线|Sans serif
系统|System
窗口|Window
半透明侧栏|Translucent sidebar
让侧栏和标题栏透出桌面的 Mica 材质（Windows 11）|Let the desktop's Mica material show through the sidebar and title bar (Windows 11)
减少动态效果|Reduce motion
关闭弹簧、淡入与闪烁等动画|Turn off springs, fades and shimmering animations
语言与输入|Language & input
等待你的确认|Waiting for you
本地 · %d 个 AI 服务|Local · %d AI services
早上好|Good morning
中午好|Good afternoon
下午好|Good afternoon
晚上好|Good evening
，|,\x20
审查改动|Review changes
审查当前工作区未提交的改动，指出问题与风险。|Review the uncommitted changes in this workspace and point out problems and risks.
定位问题|Find a bug
帮我定位并修复这个问题：|Help me find and fix this problem:\x20
补充测试|Add tests
为最近改动的代码补充测试，并运行它们。|Add tests for the recently changed code and run them.
了解项目|Explore the project
梳理这个项目的结构、入口和关键模块。|Outline this project's structure, entry points and key modules.
实现功能|Build a feature
实现以下功能：|Implement the following feature:\x20
最近任务|Recent tasks
打开任务 |Open task\x20
工作区|Workspace
通用|General
运行结束但没有产生结果|The run finished without a result
当前任务仍在运行，不能替换会话或工作区|The current task is running. The conversation or workspace cannot be replaced.
当前回合结束后才能执行命令|Wait for the current turn to finish before running a command
已清屏，会话上下文保留。|View cleared. Conversation context is preserved.
已开启新会话。|New conversation started.
暂无会话。|No conversations yet.
会话 ID 不唯一或不存在|The conversation ID is ambiguous or does not exist
当前模型不支持 reasoning effort|The current model does not support adjustable reasoning effort
可选模式：default, accept_edits, bypass|Available modes: default, accept_edits, bypass
当前没有暂停的任务|No paused task to resume
当前任务仍在运行|The current task is still running
上下文已压缩。|Context compacted.
上下文未超过阈值，无需压缩。|Context is below the threshold. No compaction needed.
请先配置并启用该模型的服务|Configure and enable this model's service first
请填写 API 密钥|Enter an API key
接口地址必须是 HTTP(S) URL|The endpoint must be an HTTP(S) URL
连接失败，请检查地址、网络和接口格式|Connection failed. Check the endpoint, network and API format.
找不到子助手|Subagent not found
找不到会话|Conversation not found
归档内容不可用|Archived content is unavailable
不能替换正在运行的会话|Cannot replace a running conversation
无效 reasoning effort|Invalid reasoning effort
无效权限模式|Invalid permission mode
请选择 1 至 10 个附件|Choose 1 to 10 attachments
附件总大小不能超过 24 MB|Attachments cannot exceed 24 MB in total
附件列表无效或超过 10 个|Invalid attachment list or more than 10 attachments
请输入任务内容|Enter a task
工作区不存在|Workspace does not exist
会话不属于当前工作区|This conversation belongs to a different workspace
请在设置中配置、启用并选择一个可用的 AI 模型|Configure, enable and select an available AI model in Settings
任务运行中，无法删除会话|Cannot delete a conversation while its task is running
显示本帮助|Show command help
查看或切换活跃模型（z.ai/glm-5.3-flash 等）|View or switch the active model
调整推理预算（reasoning effort，六档）|Adjust reasoning effort (six levels)
查看或切换权限模式|View or switch the permission mode
清空屏幕显示（保留会话上下文）|Clear the view and preserve conversation context
彻底重置上下文，开启全新会话|Reset context and start a new conversation
手动压缩当前会话上下文|Compact conversation context
列出、激活或停用本地技能|List, activate or deactivate local skills
列出历史会话|List previous conversations
恢复一个历史会话|Resume a previous conversation
从上次暂停处继续未完成任务|Continue the task from its last pause
退出（或 Ctrl+Q / Ctrl+C 两次）|Quit (or press Ctrl+Q / Ctrl+C twice)
全局 AGENTS.md|Global AGENTS.md
这些指令适用于所有工作区。项目 AGENTS.md 在对应目录内优先；保存后从下一次模型请求生效。|These instructions apply to every workspace. Project AGENTS.md takes precedence within its directory. Saved changes apply to the next model request.
正在读取指令…|Loading instructions…
指令内容|Instructions
全局指令内容|Global instructions
%d / 64000 字节|%d / 64000 bytes
未保存|Unsaved
保存指令|Save instructions
正在保存…|Saving…
已保存，下次模型请求生效。|Saved. Applies to the next model request.
还原已保存内容|Restore saved content
重新读取文件|Reload file
留空并保存即可停用全局指令。|Save an empty file to disable global instructions.
本地资源|Local resources
扫描本机已有配置，按需启用。开关仅保存在 MiniCode，下一回合生效；原应用配置不受影响。|Scan existing local configurations and enable what you need. Selections are saved in MiniCode and apply to the next turn. Source app configurations stay unchanged.
资源范围|Resource scope
全部|All
全局|Global
项目|Project
搜索本地资源|Search local resources
搜索名称或来源|Search names or sources
扫描本地配置|Scan local configurations
正在扫描本地配置…|Scanning local configurations…
%d 个资源|%d resources
未发现匹配的本地资源。|No matching local resources found.
暂不支持|Unsupported
unsupported|Unsupported
available|Available
启用本地资源 %s|Enable local resource %s
当前会话已加载|Loaded in this conversation
启用的技能按需读取正文；这里列出已加入当前会话的技能。|Enabled skills are read on demand. These skills have been loaded into the current conversation.
卸载技能|Unload skill
当前会话还没有加载技能正文。|No skill instructions loaded in this conversation yet.
连接状态|Connection status
启用的 MCP 服务在运行回合中连接，也可以手动测试连接。|Enabled MCP servers connect during a run. You can also test their connections manually.
最近日志|Recent logs
日志文件|Log file
测试已启用服务|Test enabled servers
MiniCode 项目插件|MiniCode project plugins
项目插件沿用现有清单与内容锁定。|Project plugins use their existing manifests and content lockfile.
启用 %s|Enable %s
、|,\x20
模型与预算|Models & budget
权限|Permissions
技能|Skills
MCP 服务|MCP servers
插件|Plugins
子助手|Subagents
快捷键|Shortcuts
运行记录|Run history
关于|About
设置|Settings
关闭设置|Close settings
语言|Language
界面语言|Interface language
仅切换界面语言，不影响模型回复或自定义指令。|Changes the interface language. Model responses and custom instructions keep their original language.
外观|Appearance
主题|Theme
跟随系统时会随 Windows 深浅色切换|Follows the Windows light or dark theme
跟随系统|System
浅色|Light
深色|Dark
界面缩放|Interface scale
调整对话与面板的文字大小|Adjust text size in conversations and panels
紧凑|Compact
标准|Default
宽松|Comfortable
输入与对话|Input & conversations
发送方式|Send shortcut
Enter 发送，Shift + Enter 换行|Enter to send, Shift + Enter for a new line
默认展开工具详情|Expand tool details by default
在对话中直接显示每一步的参数与输出|Show each step's arguments and output in the conversation
通知|Notifications
系统通知|Desktop notifications
窗口不在前台时，任务完成或需要审批会发出桌面通知|Notify when a task finishes or needs approval while the window is in the background
模型与思考强度在下一次请求生效，预算在下一回合生效。|Model and reasoning changes apply to the next request; budget changes apply to the next turn.
保存服务|Save service
取消|Cancel
控制模型用于分析复杂问题的推理预算。|Control the reasoning budget for complex tasks.
当前模型不支持调整思考强度。|This model does not support adjustable reasoning effort.
思考强度|Reasoning effort
运行预算|Run budget
限制单次任务的资源上限。0 表示不限制，下一回合生效。|Set resource limits for a task. Zero means unlimited. Changes apply to the next turn.
轮次上限|Maximum rounds
Token 上限|Maximum tokens
时长（秒）|Time limit (seconds)
应用预算|Apply budget
预算必须是非负数。|Budget values must be nonnegative.
还原|Reset
验收标准|Acceptance checks
选择 YAML 验收配置，任务完成时自动检查结果。保存后用于下一个新会话。|Choose a YAML acceptance configuration to check results when a task finishes. Applies to new conversations.
验收文件路径|Acceptance file path
选择文件|Choose file
应用|Apply
清除|Clear
工具权限|Tool permissions
选择文件编辑与终端命令的批准方式。只读工具始终直接执行。|Choose how to approve file edits and terminal commands. Read-only tools run automatically.
本会话始终允许|Always allow in this conversation
在审批卡片中选择“本会话始终允许”的工具，会在当前会话内自动批准。|Tools marked "Always allow in this conversation" on an approval card are approved automatically for this conversation.
当前会话没有自动批准的工具。|No tools are automatically approved for this conversation.
全部撤销|Revoke all
来自用户、项目和已启用插件。激活后会加入当前 Agent 的指令上下文。|Available from your profile, project and enabled plugins. Active skills are included in the agent's instructions.
当前工作区没有发现 Skills。|No skills found in this workspace.
启用 |Enable\x20
MCP 由已启用的本地插件提供，工具会在 Agent 回合中自动发现。|MCP servers are provided by enabled local plugins. Their tools are discovered when the agent runs.
重新发现|Discover again
当前工作区没有已启用的 MCP 服务。|No enabled MCP servers in this workspace.
插件：|Plugin:\x20
已配置|Configured
已连接|Connected
本地插件|Local plugins
插件清单位于工作区 .minicode/plugins，切换后下一回合重载工具。|Plugin manifests are in .minicode/plugins. Tools reload on the next turn after a change.
更新锁定|Update lockfile
当前工作区没有本地插件。|No local plugins in this workspace.
会话运行记录|Conversation run history
会话 %s · %d 轮 · 上下文 %d tokens|Conversation %s · %d rounds · %d context tokens
筛选事件|Filter events
开始任务后，这里会记录每一个运行事件。|Run events will appear here after you start a task.
本地优先的 Coding Agent 工作台。会话保存在 ~/.minicode/sessions.db。|A local-first coding agent workspace. Conversations are stored in ~/.minicode/sessions.db.
版本|Version
渲染器|Renderer
原生 Direct3D 11|Native Direct3D 11
平台|Platform
当前模型|Current model
切换模型在下一次请求生效；默认模型用于新会话。|Model changes apply to the next request. The default model is used for new conversations.
请选择模型|Choose a model
当前会话|Current conversation
当前会话模型|Current conversation model
自动选择可用模型|Choose an available model automatically
默认模型|Default model
AI 服务 · %d|AI services · %d
配置接口与模型，新服务从空白开始。|Configure endpoints and models. New services start empty.
尚未添加 AI 服务。填写接口地址、密钥和模型 ID，从零配置你的模型。|No AI services yet. Add an endpoint, API key and model ID to configure your model.
添加服务|Add service
 · %d 个模型| · %d models
编辑 |Edit\x20
删除 |Delete\x20
删除此服务配置？|Delete this service configuration?
删除|Delete
添加 AI 服务|Add AI service
编辑 AI 服务|Edit AI service
保存后可随时继续添加或编辑模型。已存密钥留空即可保留。|You can add or edit models after saving. Leave the API key empty to keep the saved key.
服务名称|Service name
接口格式|API format
接口地址|Endpoint URL
API 密钥|API key
已保存密钥，留空保留|Key saved; leave empty to keep it
测试连接 / 获取列表|Test connection / fetch models
测试连接|Test connection
获取模型列表|Fetch models
连接成功|Connected
仅在模型支持思考时启用。|Enable only for models that support thinking.
添加模型|Add model
获取服务列表或输入模型 ID。|Fetch the model list or enter a model ID.
自定义模型 ID|Custom model ID
输入模型 ID|Enter a model ID
添加|Add
搜索服务模型|Search service models
已配置模型 · %d|Configured models · %d
展开模型，编辑名称、上下文窗口与输出上限。|Expand a model to edit its name, context window and output limit.
尚未添加模型。|No models added yet.
编辑模型 |Edit model\x20
模型 ID|Model ID
显示名称|Display name
上下文窗口|Context window
输出上限|Output limit
支持思考强度|Adjustable reasoning effort
仅适用于支持 reasoning_effort 的 OpenAI 兼容模型。|Available for OpenAI-compatible models that support reasoning_effort.
输入能力|Input capabilities
图片|Images
音频|Audio
视频|Video
按服务和模型的实际能力勾选；视频需要接口支持视频输入。Anthropic Messages 支持图片与 PDF。|Select the inputs supported by your service and model. Video requires a compatible endpoint. Anthropic Messages supports images and PDF.
移除模型|Remove model
请输入 %d 至 2147483647 的整数|Enter an integer from %d to 2147483647
创建子助手|Create subagent
编辑子助手|Edit subagent
保存子助手|Save subagent
名称|Name
指令|Instructions
委派条件|Delegation criteria
子助手指令|Subagent instructions
可复制内置模板或空白创建，编辑委派条件、指令和工具。|Copy a built-in template or start from scratch. Customize delegation criteria, instructions and tools.
子助手会继承父会话的权限、审批与共享预算。|Subagents inherit the parent conversation's permissions, approvals and shared budget.
复制 |Copy\x20
继承父助手全部工具|Inherit all tools from the parent agent
确认删除子助手|Confirm subagent deletion
工具定义|Tool definitions
系统提示词|System prompt
对话消息|Conversation messages
未报告|Not reported
未记录|Not recorded
本会话累计|Conversation total
缓存命中率|Cache hit rate
最近生成 TPS|Latest generation TPS
缓存读取 / 写入|Cache read / write
平均生成 TPS|Average generation TPS
请求整体速度|Overall request speed
最近首字等待|Latest time to first token
%.2f 秒|%.2f s
立即压缩上下文|Compact context now
逐项确认|Ask each time
自动编辑|Auto-approve edits
全部允许|Allow all
每一次写入、编辑和命令执行都需要你批准。|Ask before every write, edit or terminal command.
自动批准文件编辑，终端命令仍需确认。|Approve file edits automatically. Terminal commands still require approval.
自动执行所有工具，请确认工作区与工具可信。|Run all tools automatically. Use with a trusted workspace and tools.
关闭|Off
低|Low
中|Medium
高|High
很高|Very high
最大|Max
未配置|Not configured
关闭提示|Dismiss suggestions
任务处理中|Working
停止|Stop
等待执行 · %d|Queued · %d
 · %d 个附件| · %d attachments
取消排队消息 %d|Remove queued message %d
正在添加附件…|Adding attachments…
描述任务，输入 / 使用命令，@ 引用文件|Describe a task, use / for commands or @ to reference files
先打开工作区，再描述你想完成的任务…|Open a workspace, then describe your task…
追加指令，将在当前回合结束后执行…|Add instructions to run after the current turn…
任务输入|Task input
添加附件|Add attachments
引用文件|Reference files
发送任务|Send task
加入队列|Add to queue
停止任务|Stop task
输入建议|Input suggestions
命令建议|Command suggestions
引用本地文件|Reference local files
↑↓ 选择 · Tab 插入|↑↓ Select · Tab Insert
配置模型|Configure model
切换模型|Switch model
尚未配置模型，添加你的 AI 服务后开始使用。|No models configured. Add an AI service to get started.
切换至 |Switch to\x20
 · 未配置或未启用| · Not configured or disabled
管理模型与服务|Manage models & services
权限模式|Permission mode
上下文窗口详情|Context window details
不可用|Unavailable
思考强度滑条|Reasoning effort slider
思考强度：|Reasoning effort:\x20
选择支持思考强度的模型后可调整|Choose a model with adjustable reasoning effort
刚刚|Just now
%d 分钟|%d min
%d 小时|%d hr
%d 天|%d days
 秒| s
 分 | min\x20
请选择工作区中的文件|Choose a file in the workspace
选择工作区|Choose workspace
选择验收配置|Choose acceptance configuration
无法打开文件|Cannot open file
展开工作区|Expand workspace
查看|View\x20
调整面板宽度|Resize panel
就绪|Ready
运行中|Running
等待确认|Awaiting confirmation
展开左侧面板|Expand sidebar
新建对话|New conversation
新建任务|New conversation
打开工作区|Open workspace
搜索任务|Search conversations
切换主题|Toggle theme
收拢左侧面板|Collapse sidebar
清除任务搜索|Clear conversation search
置顶|Pinned
工作空间|Workspaces
工作区 |Workspace\x20
当前|Current
在此工作区新建任务|New conversation in this workspace
在文件夹中显示|Show in folder
从最近工作区移除|Remove from recent workspaces
暂无任务|No conversations yet
打开本地项目|Open local project
重命名任务|Rename conversation
任务操作|Conversation actions
重命名|Rename
取消置顶|Unpin
删除任务|Delete conversation
想做些什么？|What would you like to do?
关闭通知|Dismiss notification
查看任务|View conversation
改动|Changes
文件|Files
终端|Terminal
任务|Tasks
收拢右侧面板|Collapse right panel
刷新工作区|Refresh workspace
工作区没有未提交的改动。|No uncommitted changes in this workspace.
未提交的改动|Uncommitted changes
已暂存|Staged
全部暂存|Stage all
取消暂存|Unstage
暂存此文件|Stage this file
工作区文件|Workspace files
搜索文件|Search files
返回文件列表|Back to file list
正在读取…|Reading…
在输入框中引用|Reference in composer
用默认应用打开|Open with default app
复制内容|Copy content
没有可显示的差异|No changes to display
Agent 运行的终端命令会显示在这里。|Terminal commands run by the agent appear here.
任务进度|Task progress
任务开始后，这里会显示执行步骤。|Execution steps will appear here after you start a task.
会话统计|Conversation statistics
输入 token|Input tokens
每轮 token|Tokens per round
输出 token|Output tokens
缓存读取 |Cache read\x20
缓存写入 |Cache write\x20
每秒 token|Tokens per second
输出速度|Output speed
请求整体速度（含首字等待）：%.1f tok/s|Overall request speed (including initial wait): %.1f tok/s
生成 TPS = 输出 token / 首个至最后一个生成片段的时间（包含推理与工具参数）。历史记录和单个片段无法计算生成速度；客户端测量可能与网关不同。|Generation TPS = output tokens / time from the first to the last generated chunk, including reasoning and tool arguments. Historical records and single chunks cannot provide a generation rate. Client measurements may differ from the gateway.
请求明细（最近 %d 次）|Request details (latest %d)
输入与输出 token 分布|Input and output token distribution
#%d · 输入 %d · 缓存 %d · 输出 %d|#%d · Input %d · Cached %d · Output %d
#%d  输入 %d  输出 %d  缓存 %d  TPS %s|#%d  Input %d  Output %d  Cached %d  TPS %s
%d 次请求|%d requests
首字等待 %.2fs · 生成 %.2fs|First token %.2fs · Generation %.2fs
工具调用 %d|%d tool calls
轮次 %d|Round %d
待执行|Pending
进行中|In progress
完成|Done
失败|Failed
读取|Read
已读取|Read
浏览|Browse
已浏览|Browsed
搜索|Search
已搜索|Searched
编辑|Edit
已编辑|Edited
写入|Write
已写入|Wrote
运行|Run
已运行|Ran
已委派|Delegated
读取归档|Read archive
已读取归档|Read archive
正在恢复会话…|Restoring conversation…
对话记录|Conversation history
回到最新|Back to latest
复制消息|Copy message
编辑并重新发送|Edit and resend
复制回复|Copy response
重新生成|Regenerate
已完成|Completed
已停止|Stopped
已结束|Finished
 · 用时 | · Took\x20
 个操作| operations
思考过程|Reasoning
模型接口返回的推理内容|Reasoning content returned by the model API
生成中|Generating
已中断|Interrupted
编辑 %d 个文件|Edited %d files
写入 %d 个文件|Wrote %d files
读取 %d 次|%d reads
搜索 %d 次|%d searches
运行 %d 条命令|Ran %d commands
委派 %d 次|%d delegations
%d 个失败|%d failed
已执行 %d 个操作|Executed %d operations
正在执行 %d 个操作|Executing %d operations
等待批准|Awaiting approval
自动批准|Auto-approved
查看子助手会话|View subagent conversation
查看完整归档输出|View full archived output
加载更多|Load more
加载更早历史|Load earlier history
命令输出|Command output
工具输出|Tool output
已拒绝|Denied
已批准|Approved
需要你的批准|Your approval is required
Agent 请求|Agent requests\x20
批准执行|Approve
本会话始终允许 |Always allow in this conversation:\x20
拒绝|Deny
显示全部 %d 行|Show all %d lines
键盘快捷键|Keyboard shortcuts
关闭快捷键|Close shortcuts
删除任务？|Delete conversation?
对话记录、运行事件和归档输出将被永久删除，工作区中的文件不受影响。|Conversation history, run events and archived output will be permanently deleted. Workspace files are unaffected.
操作|Actions
本地目录|Local directory
打开设置|Open settings
查看键盘快捷键|View keyboard shortcuts
切换任务侧栏|Toggle sidebar
切换工作台|Toggle workspace panel
切换深浅主题|Toggle light / dark theme
快捷操作|Quick actions
搜索操作和任务|Search actions and conversations
搜索操作、任务或文件…|Search actions, conversations or files…
没有匹配“|No results for “
”的结果|”
↑ ↓ 选择|↑ ↓ Select
Enter 打开|Enter Open
Esc 关闭|Esc Close
常用|Common
搜索操作与任务|Search actions and conversations
查看快捷键|View shortcuts
发送（可改为 Ctrl Enter）|Send (can be changed to Ctrl Enter)
换行|New line
停止当前任务|Stop current task
在空输入框中浏览历史|Browse history with an empty composer
命令与文件引用|Commands and file references
清空当前视图|Clear current view
审批|Approvals
本会话始终允许该工具|Always allow this tool in this conversation
界面|Interface
收拢任务侧栏|Collapse sidebar
收拢工作台|Collapse workspace panel
切换工作台标签|Switch workspace panel tab
切换到上 / 下一个任务|Switch to previous / next conversation
复制代码|Copy code
文本|Text
文本 / |Text /\x20
最多 10 个附件，总大小不能超过 24 MB|Up to 10 attachments, 24 MB total
当前模型未启用 |This model has not enabled\x20
 输入能力，请确认模型配置或切换模型。| input. Check the model configuration or switch models.
图片、PDF、音频、视频|Images, PDF, audio, video
请先选择工作区|Choose a workspace first
附件正在添加，请稍候|Adding attachments. Please wait.
打开附件 |Open attachment\x20
移除附件 |Remove attachment\x20
添加到任务|Add to task
添加到任务：|Add to task:\x20
请使用 |Use\x20
 子助手调查并报告证据。| subagent to investigate and report evidence.
应用更新|App updates
从 oysterhyd/minicode 的 GitHub Releases 获取签名更新。|Get signed updates from oysterhyd/minicode on GitHub Releases.
自动检查更新|Check for updates automatically
启动后在后台检查，每天最多一次。|Check in the background after startup, at most once per day.
启用自动检查更新|Enable automatic update checks
自动下载并安装|Download and install automatically
新版本在下次启动时启用，不自动打断当前任务。|New versions apply on the next launch. Running tasks are not interrupted.
启用自动下载并安装|Enable automatic download and installation
检查更新|Check for updates
上次检查：|Last checked:\x20
此构建未启用更新，或安装目录不可写。请使用安装版并安装到当前用户可写的目录。|Updates are disabled for this build, or the installation directory is not writable. Use the installer with a directory writable by the current user.
加载图片…|Loading image…
图片无法预览|Image preview unavailable
打开图片 |Open image\x20
Agent bridge 不可用|Agent bridge unavailable
附件不能随斜杠命令发送|Attachments cannot be sent with slash commands
请先选择本地工作区|Choose a local workspace first
正在思考|Thinking
需要审批|Approval required
模型请求暂时失败，%g 秒后重试（第 %g 次）。|Model request failed temporarily. Retrying in %g seconds (attempt %g).
上下文已自动压缩，较早的内容已归档|Context compacted automatically; older content has been archived
验收检查：仍需改进|Acceptance check: improvements needed
验收检查：通过|Acceptance check: passed
后台命令 |Background command\x20
：已完成|: completed
：结果丢失|: result unavailable
工具执行结果需复核：|Tool result needs review:\x20
任务已暂停：|Task paused:\x20
。可输入 /continue 继续|. Enter /continue to resume
任务已结束|Task finished
已取消暂存|Unstaged
\n...[文件预览已截断，使用 Agent 的 read 工具分页查看]|\n...[Preview truncated. Use the agent's read tool to view additional pages.]
`
