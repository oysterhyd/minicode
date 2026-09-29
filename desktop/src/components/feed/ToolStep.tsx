import { useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Archive, Check, ChevronRight, CircleSlash, LoaderCircle, ShieldAlert, X, Zap } from 'lucide-react'
import type { FeedItem } from '../../types'
import { toolLabel, toolMeta, toolTarget, languageFor } from '../../lib/tools'
import { diffTexts, looksLikeDiff, parseUnifiedDiff, diffStats } from '../../lib/diff'
import { duration } from '../../lib/format'
import { highlight } from '../../lib/highlight'
import { useNow } from '../../hooks/useNow'
import { DiffStat, DiffView } from '../DiffView'
import { CopyButton } from '../CopyButton'

export const reveal = { initial: { height: 0, opacity: 0 }, animate: { height: 'auto', opacity: 1 }, exit: { height: 0, opacity: 0 },
  transition: { height: { type: 'spring' as const, stiffness: 520, damping: 42 }, opacity: { duration: .16 } } }

function editLines(item: FeedItem) {
  if (item.name === 'edit' && typeof item.args?.old_text === 'string') return diffTexts(String(item.args.old_text), String(item.args.new_text ?? ''))
  if (item.output && looksLikeDiff(item.output)) return parseUnifiedDiff(item.output)
  return null
}

function ArtifactPager({ item, sessionId, clientKey }: { item: FeedItem; sessionId: string | null; clientKey: string }) {
  const [offset, setOffset] = useState(0)
  const [page, setPage] = useState('')
  const [hasMore, setHasMore] = useState(false)
  const [error, setError] = useState('')
  async function load(nextOffset: number) {
    if (!sessionId || !item.artifactId) return
    try {
      const result = await window.desktop.request<{ text: string; hasMore: boolean }>('readArtifact', { sessionId, clientKey, artifactId: item.artifactId, offset: nextOffset })
      setOffset(nextOffset); setPage(result.text); setHasMore(result.hasMore); setError('')
    } catch (e) { setError(String(e)) }
  }
  return <div className="artifact-pager">
    {!page && <button className="button button-ghost button-small" onClick={() => void load(0)}><Archive size={13} />查看完整输出</button>}
    {error && <p className="field-error">{error}</p>}
    {page && <><pre className="tool-output">{page}</pre><div className="artifact-nav">
      <button className="button button-ghost button-small" disabled={offset === 0} onClick={() => void load(Math.max(0, offset - 20_000))}>上一页</button>
      <button className="button button-ghost button-small" disabled={!hasMore} onClick={() => void load(offset + page.length)}>下一页</button>
    </div></>}
  </div>
}

function ToolDetail({ item, sessionId, clientKey }: { item: FeedItem; sessionId: string | null; clientKey: string }) {
  const lines = useMemo(() => editLines(item), [item.name, item.args, item.output])
  const content = item.name === 'write' ? String(item.args?.content ?? '') : ''
  const writePreview = useMemo(() => content ? highlight(content.split('\n').slice(0, 60).join('\n'), languageFor(String(item.args?.path || ''))) : null, [content])
  const other = { ...(item.args || {}) }
  for (const key of ['path', 'command', 'old_text', 'new_text', 'content']) delete other[key]
  return <div className="tool-detail">
    {item.name === 'bash' ? <div className="terminal-block">
      <div className="terminal-line"><span className="terminal-prompt">$</span><code>{String(item.args?.command || '')}</code><CopyButton text={String(item.args?.command || '')} label="复制命令" iconOnly /></div>
      {(item.output || item.pending) && <pre className="terminal-output">{item.output || '正在运行…'}</pre>}
    </div> : lines ? <DiffView lines={lines} maxLines={80} compact /> : item.name === 'write' ? <pre className="code-preview hljs">{writePreview}{content.split('\n').length > 60 && <div className="code-preview-more">…共 {content.split('\n').length} 行</div>}</pre> : <>
      {Object.keys(other).length > 0 && <pre className="tool-args">{JSON.stringify(other, null, 2)}</pre>}
      {item.output && <pre className="tool-output">{item.output}</pre>}
    </>}
    {item.name !== 'bash' && lines && item.output && !looksLikeDiff(item.output) && <pre className="tool-output">{item.output}</pre>}
    {item.childSessionId && <div className="tool-meta-line">子助手会话 {item.childSessionId.slice(0, 8)}</div>}
    {item.artifactId && <ArtifactPager item={item} sessionId={sessionId} clientKey={clientKey} />}
  </div>
}

export function ToolStep({ item, sessionId, clientKey, defaultOpen = false, waiting = false }: { item: FeedItem; sessionId: string | null; clientKey: string; defaultOpen?: boolean; waiting?: boolean }) {
  const [open, setOpen] = useState<boolean | null>(null)
  const now = useNow(Boolean(item.pending) && !waiting)
  const meta = toolMeta(item.name)
  const expanded = open ?? defaultOpen
  const target = toolTarget(item)
  const stats = item.name === 'edit' || item.name === 'write' ? (() => { const lines = editLines(item); return lines ? diffStats(lines) : item.name === 'write' ? { additions: String(item.args?.content ?? '').split('\n').length, deletions: 0 } : null })() : null
  const elapsed = item.startedAt && !waiting ? (item.endedAt || now) - item.startedAt : NaN
  const state = waiting ? 'waiting' : item.pending ? 'running' : item.interrupted ? 'interrupted' : item.success === false ? 'failed' : 'done'
  return <div className={`tool-step tool-${state}`}>
    <button className="tool-row" aria-expanded={expanded} onClick={() => setOpen(!expanded)}>
      <span className="tool-icon">{state === 'running' ? <LoaderCircle size={14} className="spin" /> : state === 'waiting' ? <ShieldAlert size={14} /> : <meta.icon size={14} />}</span>
      <span className={`tool-verb ${state === 'running' ? 'shimmer-text' : ''}`}>{state === 'running' ? meta.verb : state === 'waiting' ? '等待批准' : meta.done}</span>
      {!['read', 'grep', 'bash', 'edit', 'write', 'ls'].includes(item.name || '') && <span className="tool-name">{toolLabel(item.name)}</span>}
      <span className="tool-target" title={target}>{target}</span>
      {stats && <DiffStat {...stats} />}
      {item.auto && <span className="tool-badge" data-tip="已按本会话的始终允许规则自动批准"><Zap size={11} />自动</span>}
      <span className="tool-status">
        {state === 'failed' && <X size={13} className="text-danger" aria-label="失败" />}
        {state === 'interrupted' && <CircleSlash size={13} aria-label="已中止" />}
        {state === 'done' && <Check size={13} className="tool-ok" aria-label="完成" />}
        {Number.isFinite(elapsed) && <time>{duration(elapsed)}</time>}
      </span>
      <ChevronRight size={13} className={`tool-chevron ${expanded ? 'is-open' : ''}`} />
    </button>
    <AnimatePresence initial={false}>{expanded && <motion.div className="tool-reveal" {...reveal}>
      <ToolDetail item={item} sessionId={sessionId} clientKey={clientKey} />
    </motion.div>}</AnimatePresence>
  </div>
}
