import { useEffect, useRef, useState } from 'react'
import { Bot, Check, ChevronDown, CircleAlert, Code2, LoaderCircle, Terminal, X } from 'lucide-react'
import type { FeedItem } from '../types'
import { ToolApprovalCard } from './ToolApprovalCard'

function ToolStep({ item, sessionId }: { item: FeedItem; sessionId: string | null }) {
  const summary = String(item.args?.path || item.args?.command || item.args?.pattern || item.args?.kind || '')
  const [offset, setOffset] = useState(0)
  const [page, setPage] = useState('')
  const [hasMore, setHasMore] = useState(false)
  const [pageError, setPageError] = useState('')
  async function load(nextOffset: number) {
    if (!sessionId || !item.artifactId) return
    try {
      const result = await window.desktop.request<{ text: string; hasMore: boolean }>('readArtifact', { sessionId, artifactId: item.artifactId, offset: nextOffset })
      setOffset(nextOffset); setPage(result.text); setHasMore(result.hasMore); setPageError('')
    } catch (error) { setPageError(String(error)) }
  }
  return <details className="tool-step" open={item.pending ? true : undefined}>
    <summary className="flex items-center gap-2.5">
      <span className="tool-step-icon">{item.name === 'bash' ? <Terminal size={15} /> : <Code2 size={15} />}</span>
      <span className="font-mono text-[13px] font-semibold text-primary">{item.name}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-xs text-subtle">{summary}</span>
      {item.pending ? <LoaderCircle size={14} className="animate-spin text-primary" /> : item.success ? <Check size={14} className="text-primary" /> : <X size={14} className="text-subtle" />}
      <ChevronDown size={13} className="text-subtle" />
    </summary>
    <div className="tool-step-detail">
      {item.args && <pre className="whitespace-pre-wrap break-all text-xs text-secondary">{JSON.stringify(item.args, null, 2)}</pre>}
      {item.childSessionId && <div className="mt-2 text-xs text-accent">Subagent session: {item.childSessionId.slice(0, 8)}</div>}
      {item.output && <pre className="tool-output">{item.output}</pre>}
      {item.artifactId && <div className="mt-3"><button className="choice-button" onClick={() => void load(0)}>Read archived output</button>{pageError && <p className="mt-2 text-xs text-secondary">{pageError}</p>}{page && <><pre className="tool-output">{page}</pre><div className="mt-2 flex gap-2"><button className="choice-button" disabled={offset === 0} onClick={() => void load(Math.max(0, offset - 20_000))}>Previous</button><button className="choice-button" disabled={!hasMore} onClick={() => void load(offset + page.length)}>Next</button></div></>}</div>}
    </div>
  </details>
}

export function SessionFeed({ items, busy, sessionId, onDecide }: { items: FeedItem[]; busy: boolean; sessionId: string | null; onDecide: (id: string, granted: boolean) => void }) {
  const end = useRef<HTMLDivElement>(null)
  useEffect(() => { end.current?.scrollIntoView({ behavior: 'smooth' }) }, [items])
  if (!items.length) return <div className="empty-stage flex min-h-0 flex-1 flex-col items-center justify-center px-6 pb-20 text-center">
    <img className="empty-orb" src="./app-mark.svg" alt="MiniCode" />
    <h2 className="mt-8 text-[30px] font-semibold tracking-tight text-white">今天想做点什么？</h2>
    <p className="mt-3 max-w-xl text-sm leading-6 text-secondary">选择工作区，描述任务，Agent 的输出、工具与审批会在这里实时显示。</p>
  </div>
  return <div className="scrollbar min-h-0 flex-1 overflow-y-auto px-8 py-7">
    <div className="mx-auto max-w-[850px] space-y-5">
      {items.map(item => {
        if (item.kind === 'approval') return <ToolApprovalCard key={item.id} item={item} onDecide={onDecide} />
        if (item.kind === 'tool') return <ToolStep key={item.id} item={item} sessionId={sessionId} />
        if (item.kind === 'notice') return <div key={item.id} className="system-note"><CircleAlert size={15} />{item.text}</div>
        if (item.kind === 'thinking') return <div key={item.id} className="activity-row"><LoaderCircle size={14} className="animate-spin text-accent" />{item.text}</div>
        if (item.kind === 'user') return <div key={item.id} className="flex justify-end"><div className="user-bubble whitespace-pre-wrap break-words">{item.text}</div></div>
        return <div key={item.id} className="assistant-turn"><div className="assistant-label"><Bot size={15} /> MiniCode</div><div className="whitespace-pre-wrap break-words text-[15px] leading-7 text-primary">{item.text}</div></div>
      })}
      {busy && !items.some(item => item.pending || item.approvalId) && <div className="activity-row"><LoaderCircle size={14} className="animate-spin text-accent" /> Agent 正在处理…</div>}
      <div ref={end} />
    </div>
  </div>
}
