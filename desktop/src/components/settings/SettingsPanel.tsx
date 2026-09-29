import { useEffect, useState } from 'react'
import { motion } from 'motion/react'
import { Activity, Bot, Blocks, Cable, Info, Keyboard, Palette, RefreshCw, Shield, SlidersHorizontal, Sparkles, X } from 'lucide-react'
import { Modal } from '../ui/Modal'
import type { AgentEvent, AgentState, AppInfo, Capabilities, McpDiscovery, Model } from '../../types'
import type { Preferences } from '../../lib/prefs'
import { effortOptions, permissionOptions } from '../composer/Composer'
import { Empty, Row, Section, Segmented, Toggle } from './Controls'
import { ShortcutList } from '../Shortcuts'

export type SettingsTab = 'general' | 'model' | 'permissions' | 'skills' | 'mcp' | 'plugins' | 'subagents' | 'shortcuts' | 'inspector' | 'about'
type Props = {
  tab: SettingsTab; setTab: (tab: SettingsTab) => void; onClose: () => void
  prefs: Preferences; setPref: <K extends keyof Preferences>(key: K, value: Preferences[K]) => void
  models: Model[]; state: AgentState; capabilities: Capabilities | null; mcpDiscovery: McpDiscovery | null; trace: AgentEvent[]; error: string; busy: boolean
  onModel: (model: string) => void; onEffort: (effort: string) => void; onPermission: (mode: string) => void
  onBudget: (budget: AgentState['budget']) => void; onAcceptance: (path: string) => void; onChooseAcceptance: () => Promise<string | null>
  onSkill: (name: string, active: boolean) => void; onPlugin: (name: string, enabled: boolean) => void; onLockPlugins: () => void
  onRefreshMcp: () => Promise<void> | void; onUseAgent: (kind: string) => void; onClearAlwaysAllow: () => void
}

const tabs = [
  ['general', Palette, '通用'], ['model', SlidersHorizontal, '模型与预算'], ['permissions', Shield, '权限'],
  ['skills', Sparkles, '技能'], ['mcp', Cable, 'MCP 服务'], ['plugins', Blocks, '插件'], ['subagents', Bot, '子助手'],
  ['shortcuts', Keyboard, '快捷键'], ['inspector', Activity, '运行记录'], ['about', Info, '关于'],
] as const

export function SettingsPanel(props: Props) {
  const [budget, setBudget] = useState(props.state.budget)
  const [acceptance, setAcceptance] = useState(props.state.acceptance)
  const [discovering, setDiscovering] = useState(false)
  const [info, setInfo] = useState<AppInfo | null>(null)
  const [traceQuery, setTraceQuery] = useState('')
  useEffect(() => { setBudget(props.state.budget) }, [props.state.budget])
  useEffect(() => { setAcceptance(props.state.acceptance) }, [props.state.acceptance])
  useEffect(() => { if (props.tab === 'about' && !info) void window.desktop.request<AppInfo>('getAppInfo').then(setInfo).catch(() => {}) }, [props.tab])
  const supportsEffort = props.models.find(model => model.id === props.state.model)?.supportsEffort ?? false
  const budgetDirty = JSON.stringify(budget) !== JSON.stringify(props.state.budget)
  return <Modal label="设置" className="settings-dialog" onClose={props.onClose}>
    <nav className="settings-nav" aria-label="设置分类">
      <div className="settings-nav-title">设置</div>
      {tabs.map(([id, Icon, label]) => <button key={id} aria-current={props.tab === id ? 'page' : undefined} className={props.tab === id ? 'is-active' : ''} onClick={() => props.setTab(id)}>
        {props.tab === id && <motion.span className="settings-selection" layoutId="settings-selection" transition={{ type: 'spring', stiffness: 500, damping: 40 }} />}
        <Icon size={15} /><span>{label}</span>
      </button>)}
    </nav>
    <div className="settings-main">
      <header className="settings-header"><h2>{tabs.find(([id]) => id === props.tab)?.[2]}</h2>
        <button className="icon-button" onClick={props.onClose} aria-label="关闭设置" data-tip="关闭" data-shortcut="Esc"><X size={17} /></button></header>
      {props.error && <div className="settings-error" role="alert">{props.error}</div>}
      {props.busy && ['model', 'permissions', 'skills', 'plugins', 'mcp'].includes(props.tab) && <div className="settings-hint">当前任务运行中，部分设置需等待本回合结束后才能修改。</div>}
      <motion.div key={props.tab} className="settings-content scrollbar" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: .18, ease: [.2, .8, .2, 1] }}>
        {props.tab === 'general' && <>
          <Section title="外观">
            <Row label="主题" description="跟随系统时会随 Windows 深浅色切换"><Segmented label="主题" value={props.prefs.theme} onChange={value => props.setPref('theme', value)} options={[['system', '跟随系统'], ['light', '浅色'], ['dark', '深色']]} /></Row>
            <Row label="界面缩放" description="调整对话与面板的文字大小"><Segmented label="界面缩放" value={String(props.prefs.fontScale)} onChange={value => props.setPref('fontScale', Number(value))} options={[['0.9', '紧凑'], ['1', '标准'], ['1.1', '宽松']]} /></Row>
          </Section>
          <Section title="输入与对话">
            <Row label="发送方式" description={props.prefs.sendKey === 'enter' ? 'Enter 发送，Shift + Enter 换行' : 'Ctrl + Enter 发送，Enter 换行'}>
              <Segmented label="发送方式" value={props.prefs.sendKey} onChange={value => props.setPref('sendKey', value)} options={[['enter', 'Enter'], ['mod-enter', 'Ctrl + Enter']]} /></Row>
            <Row label="默认展开工具详情" description="在对话中直接显示每一步的参数与输出"><Toggle label="默认展开工具详情" checked={props.prefs.expandTools} onChange={value => props.setPref('expandTools', value)} /></Row>
          </Section>
          <Section title="通知">
            <Row label="系统通知" description="窗口不在前台时，任务完成或需要审批会发出桌面通知"><Toggle label="系统通知" checked={props.prefs.notifications} onChange={value => props.setPref('notifications', value)} /></Row>
          </Section>
        </>}
        {props.tab === 'model' && <>
          <Section title="模型" description="切换当前会话使用的模型，下一次模型请求立即生效。">
            <div className="model-list">{props.models.map(model => <button key={model.id} className={`choice-card ${props.state.model === model.id ? 'is-active' : ''}`} aria-pressed={props.state.model === model.id}
              disabled={props.busy || (!model.available && model.id !== props.state.model)} onClick={() => props.onModel(model.id)}>
              <strong>{model.id === 'fake' ? '离线演示' : model.id}</strong><small>{model.available ? model.provider : '尚未配置'}{model.supportsEffort ? ' · 支持思考强度' : ''}</small>
            </button>)}</div>
          </Section>
          <Section title="思考强度" description={supportsEffort ? '控制模型用于分析复杂问题的推理预算。' : '当前模型不支持调整思考强度。'}>
            <Segmented label="思考强度" value={props.state.effort} onChange={value => supportsEffort && props.onEffort(value)} options={effortOptions as Array<[string, string]>} />
          </Section>
          <Section title="运行预算" description="限制单次任务的资源上限。0 表示不限制，下一回合生效。">
            <div className="field-grid">{([['max_rounds', '轮次上限'], ['max_total_tokens', 'Token 上限'], ['max_seconds', '时长（秒）']] as const).map(([key, label]) => <label key={key} className="field-label">{label}
              <input className="field" type="number" min="0" step={key === 'max_seconds' ? '0.1' : '1'} value={budget[key]} onChange={event => setBudget(previous => ({ ...previous, [key]: Number(event.target.value) }))} /></label>)}</div>
            <div className="field-actions"><button className="button button-primary button-small" disabled={!budgetDirty || props.busy} onClick={() => props.onBudget(budget)}>应用预算</button>
              {budgetDirty && <button className="button button-ghost button-small" onClick={() => setBudget(props.state.budget)}>还原</button>}</div>
          </Section>
          <Section title="验收标准" description="选择 YAML 验收配置，任务完成时自动检查结果。需在新会话开始前设置。">
            <div className="field-inline"><input aria-label="验收文件路径" className="field" value={acceptance} onChange={event => setAcceptance(event.target.value)} placeholder="选择 acceptance.yaml" />
              <button className="button button-ghost button-small" onClick={async () => { const path = await props.onChooseAcceptance(); if (path) setAcceptance(path) }}>选择文件</button></div>
            <div className="field-actions"><button className="button button-primary button-small" onClick={() => props.onAcceptance(acceptance)}>应用</button>
              <button className="button button-ghost button-small" onClick={() => { setAcceptance(''); props.onAcceptance('') }}>清除</button></div>
          </Section>
        </>}
        {props.tab === 'permissions' && <>
          <Section title="工具权限" description="选择文件编辑与终端命令的批准方式。只读工具始终直接执行。">
            <div className="choice-stack">{permissionOptions.map(option => <button key={option.value} className={`choice-card ${props.state.permissionMode === option.value ? 'is-active' : ''} ${option.value === 'bypass' ? 'is-risky' : ''}`}
              aria-pressed={props.state.permissionMode === option.value} disabled={props.busy} onClick={() => props.onPermission(option.value)}>
              <strong>{option.label}</strong><small>{option.detail}</small></button>)}</div>
          </Section>
          <Section title="本会话始终允许" description="在审批卡片中选择“本会话始终允许”的工具，会在当前会话内自动批准。"
            action={props.state.alwaysAllow?.length ? <button className="button button-ghost button-small" onClick={props.onClearAlwaysAllow}>全部撤销</button> : undefined}>
            {props.state.alwaysAllow?.length ? <div className="chip-list">{props.state.alwaysAllow.map(tool => <span key={tool} className="tool-chip">{tool}</span>)}</div> : <Empty>当前会话没有自动批准的工具。</Empty>}
          </Section>
        </>}
        {props.tab === 'skills' && <Section title="Skills" description="来自用户、项目和已启用插件。激活后会加入当前 Agent 的指令上下文。">
          {props.capabilities?.skills.length ? props.capabilities.skills.map(skill => <div className="list-row" key={skill.name}>
            <div className="list-row-text"><strong>{skill.name}<span className="tag">{skill.origin}</span></strong><p>{skill.description}</p></div>
            <Toggle label={`${skill.active ? '停用' : '启用'} ${skill.name}`} checked={skill.active} onChange={value => props.onSkill(skill.name, value)} />
          </div>) : <Empty>当前工作区没有发现 Skills。</Empty>}
        </Section>}
        {props.tab === 'mcp' && <Section title="MCP 服务" description="MCP 由已启用的本地插件提供，工具会在 Agent 回合中自动发现。"
          action={<button className="button button-ghost button-small" disabled={discovering} onClick={async () => { setDiscovering(true); try { await props.onRefreshMcp() } finally { setDiscovering(false) } }}>
            <RefreshCw size={13} className={discovering ? 'spin' : ''} />重新发现</button>}>
          {props.capabilities?.mcp.length ? props.capabilities.mcp.map(server => { const status = props.mcpDiscovery?.servers.find(item => item.server === server.name); return <div className="list-row" key={server.name}>
            <div className="list-row-text"><strong>{server.name}</strong><p>插件：{server.plugin}{status?.server_version ? ` · v${status.server_version}` : ''}</p>{status?.error && <p className="field-error">{status.error}</p>}</div>
            <span className={`status-pill ${status ? status.error ? 'is-error' : 'is-ok' : ''}`}>{status ? status.error ? '连接失败' : `已连接 ${status.protocol || ''}` : '已配置'}</span>
          </div> }) : <Empty>当前工作区没有已启用的 MCP 服务。</Empty>}
          {props.mcpDiscovery?.tools.length ? <div className="chip-list">{props.mcpDiscovery.tools.map(tool => <span className="tool-chip" key={tool}>{tool}</span>)}</div> : null}
        </Section>}
        {props.tab === 'plugins' && <Section title="本地插件" description="插件清单位于工作区 .minicode/plugins，切换后下一回合重载工具。"
          action={<button className="button button-ghost button-small" onClick={props.onLockPlugins}>更新锁定</button>}>
          {props.capabilities?.plugins.length ? props.capabilities.plugins.map(plugin => <div className="list-row" key={plugin.name}>
            <div className="list-row-text"><strong>{plugin.name}<span className="tag">v{plugin.version}</span></strong>
              <p>{[plugin.skills && 'Skills', plugin.agents && '子助手', plugin.servers.length > 0 && `${plugin.servers.length} 个 MCP`].filter(Boolean).join(' · ') || '无扩展'} · {plugin.digest}</p>
              <p className="list-row-path" title={plugin.path}>{plugin.path}</p></div>
            <Toggle label={`${plugin.enabled ? '停用' : '启用'} ${plugin.name}`} checked={plugin.enabled} onChange={value => props.onPlugin(plugin.name, value)} />
          </div>) : <Empty>当前工作区没有本地插件。</Empty>}
        </Section>}
        {props.tab === 'subagents' && <Section title="只读子助手" description="Agent 可通过 delegate 工具调用这些子助手，它们只能读取、搜索和审查工作区。">
          {(props.capabilities?.agents || ['explore', 'review']).map(kind => <div className="list-row" key={kind}>
            <div className="list-row-text"><strong>{kind}</strong><p>{kind === 'explore' ? '调查项目结构和实现' : kind === 'review' ? '审查代码与问题' : '插件提供的子助手'}</p></div>
            <button className="button button-ghost button-small" onClick={() => props.onUseAgent(kind)}>添加到任务</button>
          </div>)}
        </Section>}
        {props.tab === 'shortcuts' && <ShortcutList />}
        {props.tab === 'inspector' && <Section title="会话运行记录" description={`会话 ${props.state.sessionId?.slice(0, 8) || '尚未开始'} · ${props.state.rounds} 轮 · 上下文 ${props.state.contextTokens.toLocaleString()} tokens`}>
          <input className="field" aria-label="筛选事件" placeholder="按事件类型筛选…" value={traceQuery} onChange={event => setTraceQuery(event.target.value)} />
          <div className="trace-list">{props.trace.filter(event => event.type.includes(traceQuery.trim())).slice(-150).reverse().map(event => <details key={event.seq} className="trace-item">
            <summary><span className="trace-seq">#{event.seq}</span><span className="trace-type">{event.type}</span><time>{new Date(event.timestamp).toLocaleTimeString()}</time></summary>
            <pre>{JSON.stringify(event.data, null, 2)}</pre></details>)}
            {!props.trace.length && <Empty>开始任务后，这里会记录每一个运行事件。</Empty>}</div>
        </Section>}
        {props.tab === 'about' && <Section title="MiniCode Desktop" description="本地优先的 Coding Agent 工作台。会话保存在 ~/.minicode/sessions.db。">
          <div className="about-grid">{[['版本', info?.version], ['Electron', info?.electron], ['Chromium', info?.chrome], ['平台', info?.platform]].map(([label, value]) => <div key={label}><small>{label}</small><strong>{value || '—'}</strong></div>)}</div>
        </Section>}
      </motion.div>
    </div>
  </Modal>
}
