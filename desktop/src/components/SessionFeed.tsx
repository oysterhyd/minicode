import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { ArrowDown, ArrowUpRight, Check, ChevronDown, CircleAlert, Code2, FileSearch, FolderOpen, LoaderCircle, Pause, Terminal, X } from 'lucide-react'
import type { FeedItem } from '../types'
import { ToolApprovalCard } from './ToolApprovalCard'
import { CopyButton } from './CopyButton'

const MessageContent = lazy(() => import('./MessageContent'))

function ToolStep({ item, sessionId, clientKey }: { item: FeedItem; sessionId: string | null; clientKey: string }) {
  const summary = String(item.args?.path || item.args?.command || item.args?.pattern || item.args?.kind || '')
  const [offset, setOffset] = useState(0)
  const [page, setPage] = useState('')
  const [hasMore, setHasMore] = useState(false)
  const [pageError, setPageError] = useState('')
  async function load(nextOffset: number) {
    if (!sessionId || !item.artifactId) return
    try {
      const result = await window.desktop.request<{ text: string; hasMore: boolean }>('readArtifact', { sessionId, clientKey, artifactId: item.artifactId, offset: nextOffset })
      setOffset(nextOffset); setPage(result.text); setHasMore(result.hasMore); setPageError('')
    } catch (error) { setPageError(String(error)) }
  }
  return <details className="tool-step" open={item.pending ? true : undefined}>
    <summary className="flex items-center gap-2.5">
      <span className="tool-step-icon">{item.name === 'bash' ? <Terminal size={15} /> : <Code2 size={15} />}</span>
      <span className="font-mono text-[13px] font-semibold text-primary">{item.name}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-xs text-subtle">{summary}</span>
      {item.pending ? <LoaderCircle size={14} className="animate-spin text-primary" /> : item.interrupted ? <Pause size={14} className="text-subtle" aria-label="已中止" /> : item.success ? <Check size={14} className="text-primary" /> : <X size={14} className="text-subtle" />}
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

export function SessionFeed({ items, busy, sessionId, clientKey, workspace, onChoose, onPrompt, onDecide, children }: {
  children?: ReactNode; items: FeedItem[]; busy: boolean; sessionId: string | null; clientKey: string; workspace: string | null;
  onChoose: () => void; onPrompt: (text: string) => void; onDecide: (id: string, granted: boolean) => void
}) {
  const viewport = useRef<HTMLDivElement>(null)
  const follow = useRef(true)
  const [showLatest, setShowLatest] = useState(false)
  const reducedMotion = useReducedMotion()
  useLayoutEffect(() => {
    if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight
  }, [items, busy])
  useEffect(() => {
    const scroll = viewport.current
    const content = scroll?.firstElementChild
    if (!scroll || !content) return
    const observer = new ResizeObserver(() => { if (follow.current) scroll.scrollTop = scroll.scrollHeight })
    observer.observe(content)
    return () => observer.disconnect()
  }, [items.length > 0])
  if (!items.length) return <div className="empty-stage scrollbar">
    <motion.div className="welcome" initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: .35 }}>
      <div className="welcome-heading"><button className="welcome-project" onClick={onChoose} title={workspace || '打开本地项目'}><FolderOpen size={16} /><span>{workspace ? workspace.split(/[\\/]/).pop() : '选择工作区'}</span><ChevronDown size={13} /></button><span>本地工作区</span></div>
      <h1>今天，想构建什么？</h1>
      <p className="welcome-description">描述你想构建、修改或探索的内容。<br />从这里开始，和 MiniCode 一起实现。</p>
      {children}
      <div className="starter-heading">或者，从一个具体任务开始</div>
      <div className="starter-list">{[
        { icon: Code2, label: '了解这个项目', detail: '梳理结构、技术栈与运行方式', prompt: '请阅读当前项目，梳理目录结构、技术栈和主要模块，告诉我如何运行与验证。' },
        { icon: FileSearch, label: '审查当前改动', detail: '发现潜在问题，给出具体建议', prompt: '请审查当前工作区的未提交改动，关注正确性与潜在回归，给出文件位置和改进建议。' },
      ].map((starter, index) => <motion.button key={starter.label} className="starter-row" onClick={() => onPrompt(starter.prompt)} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: .08 + index * .05, duration: .25 }}>
        <starter.icon size={18} /><span><strong>{starter.label}</strong><small>{starter.detail}</small></span><ArrowUpRight className="starter-arrow" size={16} />
      </motion.button>)}</div>
    </motion.div>
  </div>
  return <div className="feed-container"><div ref={viewport} className="session-feed scrollbar" onScroll={() => {
    const element = viewport.current!
    follow.current = element.scrollHeight - element.scrollTop - element.clientHeight < 64
    setShowLatest(!follow.current)
  }}>
    <div className="feed-content">
      {items.map(item => {
        if (item.kind === 'approval') return <ToolApprovalCard key={item.id} item={item} onDecide={onDecide} />
        if (item.kind === 'tool') return <ToolStep key={item.id} item={item} sessionId={sessionId} clientKey={clientKey} />
        if (item.kind === 'notice') return <div key={item.id} className="system-note"><CircleAlert size={15} />{item.text}</div>
        if (item.kind === 'thinking') return busy && !items.some(other => other.pending || other.approvalId) ? <div key={item.id} className="activity-row"><span className="thinking-dots" aria-hidden="true"><i /><i /><i /></span>正在思考</div> : null
        if (item.kind === 'user') return <div key={item.id} className="flex justify-end"><div className="user-bubble whitespace-pre-wrap break-words">{item.text}</div></div>
        return <div key={item.id} className="assistant-turn"><div className="assistant-label"><img src="./app-mark.svg" alt="" />MiniCode{item.pending && <span className="streaming-label">正在生成</span>}</div><div className={`message-markdown ${item.pending ? 'is-streaming' : ''}`}>
          {item.pending ? <div className="whitespace-pre-wrap break-words">{item.text}</div> :
            <Suspense fallback={<div className="whitespace-pre-wrap">{item.text}</div>}><MessageContent text={item.text || ''} /></Suspense>}
        </div>{!item.pending && <div className="message-actions"><CopyButton text={item.text || ''} label="复制回复" /></div>}</div>
      })}
      {busy && !items.some(item => item.pending || item.approvalId || item.kind === 'thinking') && <div className="activity-row"><span className="thinking-dots" aria-hidden="true"><i /><i /><i /></span>任务处理中</div>}
    </div>
  </div><AnimatePresence>{showLatest && <motion.button className="jump-latest" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 8 }} onClick={() => {
    follow.current = true; setShowLatest(false); viewport.current?.scrollTo({ top: viewport.current.scrollHeight, behavior: reducedMotion ? 'instant' : 'smooth' })
  }}><ArrowDown size={14} />回到最新</motion.button>}</AnimatePresence></div>
}
