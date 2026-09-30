import { useEffect, useState } from 'react'
import { Copy, Pencil, Plus, Trash2 } from 'lucide-react'
import type { AgentDefinition, Configuration } from '../../types'
import { Empty, Section, Toggle } from './Controls'

const tools = [['read', 'Read'], ['ls', 'Ls'], ['grep', 'Grep'], ['read_artifact', 'ReadArtifact'], ['bash', 'Bash'], ['edit', 'Edit'], ['write', 'Write']] as const
const blank = (): AgentDefinition => ({ name: '', label: '', description: '', instructions: '', tools: ['read', 'ls', 'grep'], inheritTools: false, enabled: true })
type Props = { request: <T>(method: string, params?: Record<string, unknown>) => Promise<T>; onChanged: () => Promise<void>; onUse: (name: string) => void }

export function AgentConfiguration({ request, onChanged, onUse }: Props) {
  const [agents, setAgents] = useState<AgentDefinition[]>([])
  const [draft, setDraft] = useState<AgentDefinition | null>(null)
  const [originalName, setOriginalName] = useState<string | null>(null)
  const [working, setWorking] = useState(false)
  const [error, setError] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  useEffect(() => { void request<Configuration>('getConfiguration').then(c => setAgents(c.agents)).catch(e => setError(String(e))) }, [])
  function edit(agent?: AgentDefinition) { setOriginalName(agent && !agent.builtin ? agent.name : null); setDraft(agent ? { ...structuredClone(agent), name: agent.builtin ? `${agent.name}-custom` : agent.name, builtin: false } : blank()); setError('') }
  async function perform(action: () => Promise<void>) {
    setWorking(true); setError('')
    try { await action(); await onChanged() } catch (e) { setError(String(e).replace(/^Error: /, '')) } finally { setWorking(false) }
  }
  return <>
    {error && <div className="settings-error" role="alert">{error}</div>}
    {draft ? <Section title={originalName ? '编辑子助手' : '新建子助手'} description="自定义配置适用于所有工作区；委派时沿用父会话的权限与共享预算。">
      {!originalName && <><h4>从模板开始</h4><div className="chip-list">{agents.filter(a => a.builtin).map(agent => <button key={agent.name} className="button button-ghost button-small" onClick={() => edit(agent)}>{agent.label}</button>)}<button className="button button-ghost button-small" onClick={() => edit()}>空白开始</button></div></>}
      <div className="field-grid service-fields">
        <label className="field-label">名称 / 委派 ID<input className="field" value={draft.name} placeholder="doc-reviewer" onChange={e => setDraft({ ...draft, name: e.target.value })} /></label>
        <label className="field-label">显示名称<input className="field" value={draft.label} onChange={e => setDraft({ ...draft, label: e.target.value })} /></label>
        <label className="field-label field-wide">何时委派给它<textarea className="field" rows={3} value={draft.description} onChange={e => setDraft({ ...draft, description: e.target.value })} /></label>
      </div>
      <h4>可用工具</h4><label className="model-check"><input type="checkbox" checked={draft.inheritTools} onChange={e => setDraft({ ...draft, inheritTools: e.target.checked })} />继承主会话工具</label>
      <div className="chip-list">{tools.map(([id, label]) => <label key={id} className="tool-chip model-check"><input type="checkbox" disabled={draft.inheritTools} checked={draft.tools.includes(id)} onChange={e => setDraft({ ...draft, tools: e.target.checked ? [...draft.tools, id] : draft.tools.filter(t => t !== id) })} />{label}</label>)}</div>
      <label className="field-label agent-instructions">指令 · {new TextEncoder().encode(draft.instructions).length.toLocaleString()} / 32,000 字节<textarea className="field" rows={12} value={draft.instructions} placeholder="描述它应该做什么、按什么顺序检查，以及向主助手报告什么。" onChange={e => setDraft({ ...draft, instructions: e.target.value })} /></label>
      <div className="field-actions"><button className="button button-primary button-small" disabled={working} onClick={() => void perform(async () => { setAgents(await request<AgentDefinition[]>('saveAgent', { agent: { ...draft, originalName } })); setDraft(null) })}>保存</button><button className="button button-ghost button-small" disabled={working} onClick={() => setDraft(null)}>取消</button></div>
    </Section> : <>
      <Section title="内置子助手" description="可启停内置模板，也可以复制为自己的子助手。修改在下一轮委派中生效。">
        {agents.filter(a => a.builtin).map(row)}
      </Section>
      <Section title="自定义子助手 · 全局" action={<button className="button button-primary button-small" onClick={() => edit()}><Plus size={14} />新建子助手</button>}>
        {agents.filter(a => !a.builtin).map(row)}
        {!agents.some(a => !a.builtin) && <Empty>从内置模板或空白指令创建自己的子助手。</Empty>}
      </Section>
    </>}
  </>
  function row(agent: AgentDefinition) {
    return <div className="list-row agent-row" key={agent.name}>
      <div className="list-row-text"><strong>{agent.label}<span className="tag">{agent.builtin ? '内置' : '自定义'}</span></strong><p><code>delegate({agent.name})</code></p><p>{agent.description}</p><div className="chip-list">{agent.inheritTools ? <span className="tool-chip">继承主会话工具</span> : agent.tools.map(t => <span key={t} className="tool-chip">{t}</span>)}</div></div>
      <div className="agent-actions"><button className="icon-button" aria-label={agent.builtin ? `复制 ${agent.label}` : `编辑 ${agent.label}`} onClick={() => edit(agent)}>{agent.builtin ? <Copy size={14} /> : <Pencil size={14} />}</button>
        <Toggle label={`启用 ${agent.label}`} checked={agent.enabled} onChange={enabled => void perform(async () => { setAgents(await request<AgentDefinition[]>('setAgentEnabled', { name: agent.name, enabled })) })} />
        {agent.enabled && <button className="button button-ghost button-small" onClick={() => onUse(agent.name)}>添加到任务</button>}
        {!agent.builtin && <button className="icon-button" aria-label={`删除 ${agent.label}`} onClick={() => setConfirmDelete(agent.name)}><Trash2 size={14} /></button>}
      </div>
      {confirmDelete === agent.name && <div className="inline-confirm"><span>删除此子助手？</span><button className="button button-small" disabled={working} onClick={() => void perform(async () => { setAgents(await request<AgentDefinition[]>('deleteAgent', { name: agent.name })); setConfirmDelete(null) })}>删除</button><button className="button button-ghost button-small" onClick={() => setConfirmDelete(null)}>取消</button></div>}
    </div>
  }
}
