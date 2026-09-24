import { useEffect, useState } from 'react'
import { Activity, Bot, Blocks, Cable, RefreshCw, Shield, Sparkles, X } from 'lucide-react'
import { motion } from 'motion/react'
import { Modal } from './Modal'
import type { AgentEvent, AgentState, Capabilities, Model } from '../types'

export type SettingsTab = 'general' | 'skills' | 'mcp' | 'plugins' | 'subagents' | 'inspector'
type McpDiscovery = { servers: Array<{ server: string; protocol: string | null; server_version: string | null; error: string | null }>; tools: string[] }
type Props = {
  tab: SettingsTab
  setTab: (tab: SettingsTab) => void
  onClose: () => void
  models: Model[]
  state: AgentState
  capabilities: Capabilities | null
  mcpDiscovery: McpDiscovery | null
  trace: AgentEvent[]
  error: string
  onModel: (model: string) => void
  onEffort: (effort: string) => void
  onPermission: (mode: string) => void
  onBudget: (budget: AgentState['budget']) => void
  onAcceptance: (path: string) => void
  onChooseAcceptance: () => Promise<string | null>
  onSkill: (name: string, active: boolean) => void
  onPlugin: (name: string, enabled: boolean) => void
  onLockPlugins: () => void
  onRefreshMcp: () => void
  onUseAgent: (kind: string) => void
}

const tabs = [
  ['general', Shield, '通用'], ['skills', Sparkles, '技能'], ['mcp', Cable, 'MCP 服务'],
  ['plugins', Blocks, '插件'], ['subagents', Bot, '子助手'], ['inspector', Activity, '运行记录'],
] as const

export function SettingsPanel(props: Props) {
  const [budget, setBudget] = useState(props.state.budget)
  const [acceptance, setAcceptance] = useState(props.state.acceptance)
  useEffect(() => { setBudget(props.state.budget) }, [props.state.budget])
  useEffect(() => { setAcceptance(props.state.acceptance) }, [props.state.acceptance])
  return <Modal label="设置" className="settings-dialog" onClose={props.onClose}>
      <header className="settings-header"><div><h2>设置</h2><p>工作区偏好与 Agent 配置</p></div><button className="icon-button" onClick={props.onClose} aria-label="关闭设置"><X size={18} /></button></header>
      {props.error && <div className="mx-6 mt-3 rounded-lg border border-white/25 bg-white/5 px-3 py-2 text-xs text-primary">{props.error}</div>}
      <div className="flex min-h-0 flex-1">
        <nav className="settings-nav" aria-label="设置分类">{tabs.map(([id, Icon, label]) => <button key={id} aria-current={props.tab === id ? 'page' : undefined} className={props.tab === id ? 'settings-nav-active' : ''} onClick={() => props.setTab(id)}>{props.tab === id && <motion.span className="settings-selection" layoutId="settings-selection" />}<Icon size={16} /><span>{label}</span></button>)}</nav>
        <div key={props.tab} className="settings-content scrollbar min-w-0 flex-1 overflow-y-auto p-6">
          {props.tab === 'general' && <div className="settings-stack">
            <div><h3 className="settings-heading">模型</h3><p className="settings-description">切换当前会话使用的模型。下一次模型请求立即生效。</p><select aria-label="模型" className="field mt-3 w-full" value={props.state.model} onChange={event => props.onModel(event.target.value)}>{props.models.map(model => <option key={model.id} value={model.id} disabled={!model.available && model.id !== props.state.model}>{model.id}{model.available ? '' : ' · 未配置'}</option>)}</select></div>
            <div><h3 className="settings-heading">推理强度</h3><p className="settings-description">控制模型用于分析复杂问题的推理预算。</p><select aria-label="推理强度" className="field mt-3 w-full" value={props.state.effort} onChange={event => props.onEffort(event.target.value)}>{['off', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => <option key={value} value={value}>{value}</option>)}</select></div>
            <div><h3 className="settings-heading">工具权限</h3><p className="settings-description">选择文件编辑和终端命令的批准方式。</p><div className="mt-3 grid grid-cols-3 gap-2">{['default', 'accept_edits', 'bypass'].map(mode => <button key={mode} className={`choice-button ${props.state.permissionMode === mode ? 'choice-active' : ''}`} aria-pressed={props.state.permissionMode === mode} onClick={() => props.onPermission(mode)}>{{ default: '逐项确认', accept_edits: '自动编辑', bypass: '全部允许' }[mode]}</button>)}</div></div>
            <div><h3 className="settings-heading">运行预算</h3><p className="settings-description">设置单次任务的资源上限。0 表示不限制，下一回合生效。</p><div className="mt-3 grid grid-cols-3 gap-3">{([
              ['max_rounds', '轮次上限'], ['max_total_tokens', 'Token 上限'], ['max_seconds', '时长（秒）'],
            ] as const).map(([key, label]) => <label key={key} className="text-xs text-secondary">{label}<input className="field mt-1 w-full" type="number" min="0" step={key === 'max_seconds' ? '0.1' : '1'} value={budget[key]} onChange={event => setBudget(previous => ({ ...previous, [key]: Number(event.target.value) }))} /></label>)}</div><button className="choice-button mt-3" onClick={() => props.onBudget(budget)}>应用预算</button></div>
            <div><h3 className="settings-heading">验收标准</h3><p className="settings-description">选择 YAML 验收配置，在任务完成时自动检查结果。请在新会话开始前设置。</p><div className="mt-3 flex gap-2"><input aria-label="验收文件路径" className="field min-w-0 flex-1" value={acceptance} onChange={event => setAcceptance(event.target.value)} placeholder="选择 acceptance.yaml" /><button className="choice-button" onClick={async () => { const path = await props.onChooseAcceptance(); if (path) setAcceptance(path) }}>选择文件</button></div><div className="mt-2 flex gap-2"><button className="choice-button" onClick={() => props.onAcceptance(acceptance)}>应用</button><button className="choice-button" onClick={() => { setAcceptance(''); props.onAcceptance('') }}>清除</button></div></div>
            <div><h3 className="settings-heading">上下文</h3><p className="settings-description">{props.state.contextTokens.toLocaleString()} / {props.state.contextWindow.toLocaleString()} tokens · 当前会话累计输入 {props.state.usage?.input_tokens.toLocaleString() || 0}，输出 {props.state.usage?.output_tokens.toLocaleString() || 0}</p></div>
          </div>}
          {props.tab === 'skills' && <div><h3 className="settings-heading">Skills</h3><p className="settings-description mb-4">来自用户、项目和已启用插件。激活后会加入当前 Agent 的指令上下文。</p>{props.capabilities?.skills.length ? props.capabilities.skills.map(skill => <div className="settings-list-row" key={skill.name}><div className="min-w-0 flex-1"><div className="font-medium text-primary">{skill.name}<span className="ml-2 text-[11px] text-subtle">{skill.origin}</span></div><p className="mt-1 text-xs leading-5 text-secondary">{skill.description}</p></div><button className={`choice-button shrink-0 ${skill.active ? 'choice-active' : ''}`} onClick={() => props.onSkill(skill.name, !skill.active)}>{skill.active ? '停用' : '启用'}</button></div>) : <p className="panel-empty">当前工作区没有发现 Skills。</p>}</div>}
          {props.tab === 'mcp' && <div><div className="flex items-center justify-between"><div><h3 className="settings-heading">MCP servers</h3><p className="settings-description">MCP 由已启用的本地插件提供，工具在 Agent 回合中自动发现。</p></div><button className="choice-button flex items-center gap-2" onClick={props.onRefreshMcp}><RefreshCw size={14} /> Discover</button></div>{props.capabilities?.mcp.length ? props.capabilities.mcp.map(server => { const status = props.mcpDiscovery?.servers.find(item => item.server === server.name); return <div className="settings-list-row" key={server.name}><div><div className="font-medium text-primary">{server.name}</div><p className="mt-1 text-xs text-secondary">Plugin: {server.plugin}</p>{status?.error && <p className="mt-1 text-xs text-primary">{status.error}</p>}</div><span className="text-xs text-secondary">{status ? status.error ? 'Error' : `Connected · ${status.protocol || ''}` : 'Configured'}</span></div> }) : <p className="panel-empty">当前工作区没有已启用的 MCP servers。</p>}{props.mcpDiscovery?.tools.length ? <div className="mt-5"><h4 className="mb-2 text-xs font-semibold text-secondary">Discovered tools</h4><div className="flex flex-wrap gap-2">{props.mcpDiscovery.tools.map(tool => <span className="tool-chip" key={tool}>{tool}</span>)}</div></div> : null}</div>}
          {props.tab === 'plugins' && <div><div className="flex items-center justify-between"><div><h3 className="settings-heading">本地插件</h3><p className="settings-description">插件清单位于工作区 .minicode/plugins。切换后下一回合重载工具。</p></div><button className="choice-button" onClick={props.onLockPlugins}>更新锁定</button></div>{props.capabilities?.plugins.length ? props.capabilities.plugins.map(plugin => <div className="settings-list-row" key={plugin.name}><div className="min-w-0 flex-1"><div className="font-medium text-primary">{plugin.name} <span className="text-xs text-subtle">v{plugin.version} · {plugin.digest}</span></div><p className="mt-1 truncate text-xs text-subtle" title={plugin.path}>{plugin.path}</p><p className="mt-1 text-xs text-secondary">{[plugin.skills && 'Skills', plugin.agents && 'Subagents', plugin.servers.length > 0 && `${plugin.servers.length} MCP`].filter(Boolean).join(' · ') || 'No extensions'}</p></div><button className={`choice-button shrink-0 ${plugin.enabled ? 'choice-active' : ''}`} onClick={() => props.onPlugin(plugin.name, !plugin.enabled)}>{plugin.enabled ? '已启用' : '已停用'}</button></div>) : <p className="panel-empty">当前工作区没有本地插件。</p>}</div>}
          {props.tab === 'subagents' && <div><h3 className="settings-heading">只读子助手</h3><p className="settings-description mb-4">Agent 可通过 delegate 工具调用这些子助手。子助手只能读取、搜索和审查工作区。</p>{(props.capabilities?.agents || ['explore', 'review']).map(kind => <div className="settings-list-row" key={kind}><div><div className="font-medium text-primary">{kind}</div><p className="mt-1 text-xs text-secondary">{kind === 'explore' ? '调查项目结构和实现' : kind === 'review' ? '审查代码与问题' : '插件提供的子助手'}</p></div><button className="choice-button" onClick={() => props.onUseAgent(kind)}>添加到任务</button></div>)}</div>}
          {props.tab === 'inspector' && <div><h3 className="settings-heading">会话运行记录</h3><p className="settings-description">Session: {props.state.sessionId || '尚未开始'} · {props.state.rounds} rounds · {props.state.contextTokens.toLocaleString()} context tokens</p><div className="mt-4 space-y-2">{props.trace.slice(-100).map(event => <details key={event.seq} className="rounded-lg border border-edge bg-work px-3 py-2"><summary className="cursor-pointer font-mono text-xs text-secondary">#{event.seq} {event.type} <span className="ml-2 text-subtle">{event.timestamp}</span></summary><pre className="mt-2 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-primary">{JSON.stringify(event.data, null, 2)}</pre></details>)}</div></div>}
        </div>
      </div>
  </Modal>
}
