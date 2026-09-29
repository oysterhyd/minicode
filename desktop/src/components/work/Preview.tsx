import { useEffect, useMemo, useState } from 'react'
import { ArrowLeft, AtSign, Check, ExternalLink, FolderSearch, LoaderCircle, Minus, Plus } from 'lucide-react'
import { parseUnifiedDiff, diffStats } from '../../lib/diff'
import { highlightLines } from '../../lib/highlight'
import { languageFor } from '../../lib/tools'
import { basename } from '../../lib/format'
import { DiffStat, DiffView } from '../DiffView'
import { CopyButton } from '../CopyButton'
import { useToast } from '../ui/Toast'

export type PreviewTarget = { kind: 'diff' | 'file'; path: string; status?: string }

export function Preview({ target, workspace, version, onBack, onMention, onChanged }: {
  target: PreviewTarget; workspace: string | null; version: unknown; onBack: () => void; onMention: (path: string) => void; onChanged: () => Promise<unknown> | void
}) {
  const [content, setContent] = useState<string | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState<'stage' | 'unstage' | null>(null)
  const toast = useToast()
  useEffect(() => {
    let active = true
    setError('')
    window.desktop.request<string>(target.kind === 'file' ? 'readFile' : 'diff', { path: target.path, workspace })
      .then(value => { if (active) setContent(value) })
      .catch(e => { if (active) { setError(String(e).replace(/^Error: /, '')); setContent('') } })
    return () => { active = false }
  }, [target.kind, target.path, workspace, version])
  const diff = useMemo(() => target.kind === 'diff' && content ? parseUnifiedDiff(content) : null, [content, target.kind])
  const lines = useMemo(() => target.kind === 'file' && content !== null ? highlightLines(content, languageFor(target.path)) : null, [content, target])
  const staged = Boolean(target.status && target.status[0] !== ' ' && target.status[0] !== '?')
  async function act(method: 'confirmDiff' | 'unstageFile') {
    setBusy(method === 'confirmDiff' ? 'stage' : 'unstage')
    try {
      await window.desktop.request(method, { path: target.path, workspace }); await onChanged()
      toast({ tone: 'success', title: method === 'confirmDiff' ? '已暂存' : '已取消暂存', detail: target.path })
    } catch (e) { setError(String(e).replace(/^Error: /, '')) }
    finally { setBusy(null) }
  }
  async function shellAction(method: 'revealPath' | 'openPath') {
    try { await window.desktop.request(method, { path: target.path, workspace }) } catch (e) { toast({ tone: 'error', title: '无法打开文件', detail: String(e).replace(/^Error: /, '') }) }
  }
  return <div className="preview">
    <div className="preview-toolbar">
      <button className="icon-button" aria-label={target.kind === 'file' ? '返回文件列表' : '返回改动列表'} data-tip="返回" onClick={onBack}><ArrowLeft size={15} /></button>
      <div className="preview-title"><strong title={target.path}>{basename(target.path)}</strong><small title={target.path}>{target.path}</small></div>
      {diff && <DiffStat {...diffStats(diff)} />}
      <button className="icon-button icon-button-small" aria-label="在输入框中引用" data-tip="在输入框中引用" onClick={() => onMention(target.path)}><AtSign size={14} /></button>
      <button className="icon-button icon-button-small" aria-label="在文件夹中显示" data-tip="在文件夹中显示" onClick={() => void shellAction('revealPath')}><FolderSearch size={14} /></button>
      <button className="icon-button icon-button-small" aria-label="用默认应用打开" data-tip="用默认应用打开" onClick={() => void shellAction('openPath')}><ExternalLink size={14} /></button>
      <CopyButton text={content || ''} label="复制内容" iconOnly />
    </div>
    <div className="preview-body scrollbar">
      {content === null ? <div className="preview-loading" role="status"><LoaderCircle size={15} className="spin" />正在读取…</div>
        : error ? <div className="field-error preview-error">{error}</div>
        : diff ? diff.length ? <DiffView lines={diff} /> : <div className="diff-empty">{content}</div>
        : <pre className="file-preview hljs">{lines!.map((line, index) => <div key={index} className="file-line"><span className="file-line-number" aria-hidden="true">{index + 1}</span><span className="file-line-text">{line || ' '}</span></div>)}</pre>}
    </div>
    {target.kind === 'diff' && <div className="preview-actions">
      {staged && <span className="preview-staged"><Check size={13} />已暂存</span>}
      <span className="toolbar-spacer" />
      {staged && <button className="button button-ghost button-small" disabled={Boolean(busy)} onClick={() => void act('unstageFile')}>{busy === 'unstage' ? <LoaderCircle size={13} className="spin" /> : <Minus size={13} />}取消暂存</button>}
      <button className="button button-primary button-small" disabled={Boolean(busy) || Boolean(error)} onClick={() => void act('confirmDiff')}>{busy === 'stage' ? <LoaderCircle size={13} className="spin" /> : <Plus size={13} />}{staged ? '再次暂存' : '暂存此文件'}</button>
    </div>}
  </div>
}
