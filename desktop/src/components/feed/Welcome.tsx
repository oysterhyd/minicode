import type { ReactNode } from 'react'
import { motion } from 'motion/react'
import { ArrowUpRight, Bug, ChevronDown, Code2, FileSearch, FlaskConical, FolderOpen, History } from 'lucide-react'
import type { Session } from '../../types'
import { basename, relativeTime } from '../../lib/format'

const starters = [
  { icon: Code2, label: '了解这个项目', detail: '梳理结构、技术栈与运行方式', prompt: '请阅读当前项目，梳理目录结构、技术栈和主要模块，告诉我如何运行与验证。' },
  { icon: FileSearch, label: '审查当前改动', detail: '发现潜在问题，给出具体建议', prompt: '请审查当前工作区的未提交改动，关注正确性与潜在回归，给出文件位置和改进建议。' },
  { icon: FlaskConical, label: '补充测试', detail: '为关键路径编写并运行测试', prompt: '请找出当前项目中缺少测试覆盖的关键逻辑，补充测试并运行，报告结果。' },
  { icon: Bug, label: '排查一个问题', detail: '定位根因并给出修复方案', prompt: '我遇到了一个问题：' },
]

function greeting() {
  const hour = new Date().getHours()
  return hour < 5 ? '夜深了，还在构建什么？' : hour < 12 ? '早上好，今天想构建什么？' : hour < 18 ? '下午好，接下来做点什么？' : '晚上好，想继续构建什么？'
}

export function Welcome({ workspace, sessions, onChoose, onPrompt, onSession, children }: {
  workspace: string | null; sessions: Session[]; onChoose: () => void; onPrompt: (text: string) => void; onSession: (session: Session) => void; children: ReactNode
}) {
  const recent = sessions.filter(session => session.workspace === workspace).slice(0, 3)
  return <div className="empty-stage scrollbar">
    <div className="welcome">
      <motion.div className="welcome-mark" initial={{ opacity: 0, scale: .8 }} animate={{ opacity: 1, scale: 1 }} transition={{ type: 'spring', stiffness: 300, damping: 22 }}>
        <img src="./app-mark.svg" alt="" />
      </motion.div>
      <motion.h1 initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: .05, duration: .35 }}>{greeting()}</motion.h1>
      <motion.button className="welcome-project" onClick={onChoose} data-tip={workspace || '选择一个本地目录'} initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: .1 }}>
        <FolderOpen size={14} /><span>{workspace ? basename(workspace) : '选择工作区'}</span><ChevronDown size={13} />
      </motion.button>
      {children}
      {workspace ? <>
        <div className="starter-grid">{starters.map((starter, index) => <motion.button key={starter.label} className="starter-card" onClick={() => onPrompt(starter.prompt)}
          aria-label={`${starter.label} ${starter.detail}`}
          initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: .14 + index * .04, duration: .28, ease: [.2, .8, .2, 1] }}>
          <starter.icon size={16} /><span><strong>{starter.label}</strong><small>{starter.detail}</small></span><ArrowUpRight className="starter-arrow" size={14} />
        </motion.button>)}</div>
        {recent.length > 0 && <motion.div className="welcome-recent" initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: .3 }}>
          <div className="section-label"><History size={12} />继续最近的任务</div>
          {recent.map(session => <button key={session.session_id} className="recent-row" onClick={() => onSession(session)}>
            <span className="truncate">{session.title}</span><time>{relativeTime(session.updated_at || session.created_at)}</time>
          </button>)}
        </motion.div>}
      </> : <motion.div className="welcome-empty" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: .15 }}>
        <p>MiniCode 在你的本地目录中读取、编辑并运行代码。每一次写入与命令执行都会先征得你的同意。</p>
        <button className="button button-primary button-large" onClick={onChoose}><FolderOpen size={16} />打开本地项目</button>
      </motion.div>}
    </div>
  </div>
}
