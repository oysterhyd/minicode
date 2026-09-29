import { AnimatePresence, motion } from 'motion/react'
import { ChevronRight, PanelLeft, PanelRight, Search, ShieldAlert, SquarePen } from 'lucide-react'
import { basename, modKey } from '../lib/format'

export function TitleBar({ workspace, title, status, leftCollapsed, rightCollapsed, onLeft, onRight, onSearch, onNew }: {
  workspace: string | null; title: string; status: 'idle' | 'running' | 'waiting' | 'loading' | 'none'
  leftCollapsed: boolean; rightCollapsed: boolean; onLeft: () => void; onRight: () => void; onSearch: () => void; onNew: () => void
}) {
  const label = { idle: '就绪', running: '运行中', waiting: '等待审批', loading: '载入中', none: '未选择工作区' }[status]
  return <header className="titlebar">
    <div className="titlebar-left">
      <img className="titlebar-mark" src="./app-mark.svg" alt="" />
      <span className="titlebar-app">MiniCode</span>
      <button className={`titlebar-button ${leftCollapsed ? '' : 'is-on'}`} onClick={onLeft} aria-label={leftCollapsed ? '展开侧栏' : '收拢侧栏'} data-tip={leftCollapsed ? '展开侧栏' : '收拢侧栏'} data-shortcut={`${modKey} B`}><PanelLeft size={15} /></button>
      <button className="titlebar-button" onClick={onNew} aria-label="新建任务" data-tip="新建任务" data-shortcut={`${modKey} N`}><SquarePen size={15} /></button>
    </div>
    <div className="titlebar-center">
      {workspace && <><span className="titlebar-workspace" title={workspace}>{basename(workspace)}</span><ChevronRight size={12} className="titlebar-sep" /></>}
      <AnimatePresence mode="wait" initial={false}><motion.span key={title} className="titlebar-title" title={title}
        initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -4 }} transition={{ duration: .14 }}>{title}</motion.span></AnimatePresence>
      <span className={`connection-status status-${status}`} role="status">{status === 'waiting' ? <ShieldAlert size={11} /> : <i />}{label}</span>
    </div>
    <div className="titlebar-right">
      <button className="titlebar-search" onClick={onSearch} aria-label="打开快捷操作" data-tip="搜索操作、任务与文件"><Search size={13} /><span>搜索</span><kbd>{modKey} K</kbd></button>
      <button className={`titlebar-button ${rightCollapsed ? '' : 'is-on'}`} onClick={onRight} aria-label={rightCollapsed ? '展开工作区' : '收拢工作区'} data-tip={rightCollapsed ? '展开工作台' : '收拢工作台'} data-shortcut={`${modKey} Shift B`}><PanelRight size={15} /></button>
    </div>
  </header>
}
