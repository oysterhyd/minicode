import { useEffect, useState } from 'react'
import { Plus, Pencil, Trash2, RefreshCw } from 'lucide-react'
import type { AIService, Configuration, Model, ServiceModel } from '../../types'
import { Empty, Section, Toggle } from './Controls'

type Props = { models: Model[]; selected: string; request: <T>(method: string, params?: Record<string, unknown>) => Promise<T>; onChanged: () => Promise<void>; onModel: (id: string) => void }
const blankModel = (id = ''): ServiceModel => ({ modelId: id, name: id, contextWindow: 200000, maxOutputTokens: 8192, supportsEffort: false })

export function ModelConfiguration({ models, selected, request, onChanged, onModel }: Props) {
  const [config, setConfig] = useState<Configuration | null>(null)
  const [draft, setDraft] = useState<AIService | null>(null)
  const [found, setFound] = useState<ServiceModel[]>([])
  const [query, setQuery] = useState('')
  const [custom, setCustom] = useState('')
  const [working, setWorking] = useState(false)
  const [error, setError] = useState('')
  const [status, setStatus] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  useEffect(() => { void request<Configuration>('getConfiguration').then(setConfig).catch(e => setError(String(e))) }, [])
  async function perform(action: () => Promise<void>) {
    setWorking(true); setError('')
    try { await action() } catch (e) { setError(String(e).replace(/^Error: /, '')) } finally { setWorking(false) }
  }
  function edit(service?: AIService) {
    setDraft(service ? structuredClone(service) : { id: '', name: '', baseUrl: '', apiStyle: 'openai', apiKey: '', enabled: true, models: [] })
    setFound([]); setQuery(''); setCustom(''); setStatus(''); setError(''); setConfirmDelete(null)
  }
  async function save(service: AIService) {
    setConfig(await request<Configuration>('saveService', { service })); await onChanged()
  }
  function add(model: ServiceModel) {
    setDraft(previous => previous && ({ ...previous, models: previous.models.some(m => m.modelId === model.modelId) ? previous.models : [...previous.models, model] }))
  }
  function updateModel(index: number, change: Partial<ServiceModel>) {
    setDraft(previous => previous && ({ ...previous, models: previous.models.map((m, i) => i === index ? { ...m, ...change } : m) }))
  }
  return <>
    {error && <div className="settings-error" role="alert">{error}</div>}
    {draft ? <Section title={draft.id ? '编辑 AI 服务' : '添加 AI 服务'} description="保存后可随时继续添加或编辑模型。已存密钥留空即可保留。">
      <div className="field-grid service-fields">
        <label className="field-label">服务名称<input className="field" value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} /></label>
        <label className="field-label">接口格式<select className="field" value={draft.apiStyle} onChange={e => setDraft({ ...draft, apiStyle: e.target.value as AIService['apiStyle'] })}><option value="openai">OpenAI Chat Completions</option><option value="anthropic">Anthropic Messages</option></select></label>
        <label className="field-label field-wide">接口地址<input className="field" type="url" placeholder="https://api.example.com/v1" value={draft.baseUrl} onChange={e => setDraft({ ...draft, baseUrl: e.target.value })} /></label>
        <label className="field-label field-wide">API 密钥<input className="field" type="password" autoComplete="off" placeholder={draft.hasApiKey ? '已保存密钥，留空保留' : '输入 API 密钥'} value={draft.apiKey} onChange={e => setDraft({ ...draft, apiKey: e.target.value })} /></label>
      </div>
      <div className="field-actions"><button className="button button-ghost button-small" disabled={working} onClick={() => void perform(async () => {
        const result = await request<{ models: ServiceModel[]; message: string }>('fetchServiceModels', { service: draft }); setFound(result.models); setStatus(result.message)
      })}><RefreshCw size={13} className={working ? 'spin' : ''} />测试连接 / 获取列表</button><span className="field-label" role="status">{status}</span></div>
      <div className="service-model-columns">
        <div className="service-model-catalog"><h4>该服务的模型</h4><input className="field" aria-label="搜索服务模型" placeholder="搜索模型 ID…" value={query} onChange={e => setQuery(e.target.value)} />
          <div className="discovered-models scrollbar">{found.filter(m => m.modelId.toLowerCase().includes(query.toLowerCase())).map(model => <label key={model.modelId} className="model-check"><input type="checkbox" checked={draft.models.some(m => m.modelId === model.modelId)} onChange={e => e.target.checked ? add(model) : setDraft({ ...draft, models: draft.models.filter(m => m.modelId !== model.modelId) })} /><code>{model.modelId}</code></label>)}
          {!found.length && <Empty>获取列表，或手动添加模型 ID。</Empty>}</div>
        </div>
        <div className="service-model-selected"><h4>模型设置 · {draft.models.length}</h4>
          {draft.models.map((model, index) => <details className="configured-model" key={index} open={model.modelId === custom}>
            <summary><code>{model.name || model.modelId}</code><span>{(model.contextWindow / 1000).toLocaleString()}K</span></summary>
            <div className="field-grid">
              <label className="field-label field-wide">模型 ID<input className="field" value={model.modelId} onChange={e => updateModel(index, { modelId: e.target.value })} /></label>
              <label className="field-label field-wide">显示名称<input className="field" value={model.name} onChange={e => updateModel(index, { name: e.target.value })} /></label>
              <label className="field-label">上下文窗口<input className="field" type="number" min="2" value={model.contextWindow} onChange={e => updateModel(index, { contextWindow: Number(e.target.value) })} /></label>
              <label className="field-label">输出上限<input className="field" type="number" min="1" value={model.maxOutputTokens} onChange={e => updateModel(index, { maxOutputTokens: Number(e.target.value) })} /></label>
            </div>
            <label className="model-check"><input type="checkbox" disabled={draft.apiStyle !== 'openai'} checked={draft.apiStyle === 'openai' && model.supportsEffort} onChange={e => updateModel(index, { supportsEffort: e.target.checked })} />支持思考强度（OpenAI 兼容接口）</label>
            <button className="button button-ghost button-small" onClick={() => setDraft({ ...draft, models: draft.models.filter((_, i) => i !== index) })}><Trash2 size={13} />移除模型</button>
          </details>)}
          <div className="field-inline"><input className="field" aria-label="自定义模型 ID" placeholder="输入模型 ID" value={custom} onChange={e => setCustom(e.target.value)} /><button className="button button-ghost button-small" disabled={!custom.trim()} onClick={() => { add(blankModel(custom.trim())); setCustom('') }}><Plus size={13} />添加</button></div>
        </div>
      </div>
      <div className="field-actions"><button className="button button-primary button-small" disabled={working} onClick={() => void perform(async () => { await save(draft); setDraft(null) })}>保存服务</button><button className="button button-ghost button-small" disabled={working} onClick={() => setDraft(null)}>取消</button></div>
    </Section> : <>
      <Section title="当前模型" description="切换模型在下一次请求生效；默认模型用于新会话。">
        <label className="field-label">当前会话<select className="field" value={selected} onChange={e => onModel(e.target.value)}><option value="" disabled>选择模型</option>{!models.some(m => m.id === selected) && <option value={selected} disabled>请选择已配置模型</option>}{models.map(m => <option key={m.id} value={m.id} disabled={!m.available}>{m.provider} · {m.name || m.id}{m.available ? '' : '（尚未配置）'}</option>)}</select></label>
        <label className="field-label">默认模型<select className="field" value={config?.defaultModel || ''} disabled={working} onChange={e => void perform(async () => { setConfig(await request<Configuration>('setDefaultModel', { model: e.target.value })); await onChanged() })}><option value="">自动选择可用模型</option>{models.map(m => <option key={m.id} value={m.id} disabled={!m.available}>{m.provider} · {m.name || m.id}</option>)}</select></label>
      </Section>
      <Section title={`AI 服务 · ${config?.services.length || 0}`} action={<button className="button button-primary button-small" onClick={() => edit()}><Plus size={14} />添加服务</button>}>
        {config?.services.map(service => <div className="list-row" key={service.id}>
          <div className="list-row-text"><strong>{service.name}</strong><p>{service.baseUrl} · {service.models.length} 个模型</p></div>
          <button className="icon-button" aria-label={`编辑 ${service.name}`} onClick={() => edit(service)}><Pencil size={14} /></button>
          <Toggle label={`启用 ${service.name}`} checked={service.enabled} onChange={value => void perform(() => save({ ...service, enabled: value }))} />
          <button className="icon-button" aria-label={`删除 ${service.name}`} disabled={working} onClick={() => setConfirmDelete(service.id)}><Trash2 size={14} /></button>
          {confirmDelete === service.id && <div className="inline-confirm"><span>删除此服务配置？</span><button className="button button-small" onClick={() => void perform(async () => { setConfig(await request<Configuration>('deleteService', { id: service.id })); await onChanged(); setConfirmDelete(null) })}>删除</button><button className="button button-ghost button-small" onClick={() => setConfirmDelete(null)}>取消</button></div>}
        </div>)}
        {!config?.services.length && <Empty>添加 AI 服务以开始使用模型。</Empty>}
      </Section>
    </>}
  </>
}
