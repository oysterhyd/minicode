import { Bot, BookOpen, Brain, FileEdit, FilePlus2, FileSearch, FileText, FolderTree, ListChecks, Plug, Search, Sparkles, Terminal, Wrench, type LucideIcon } from 'lucide-react'
import type { FeedItem } from '../types'

type ToolMeta = { icon: LucideIcon; verb: string; done: string; category: 'read' | 'search' | 'edit' | 'command' | 'agent' | 'task' | 'other' }

const meta: Record<string, ToolMeta> = {
  read: { icon: FileText, verb: '正在读取', done: '读取', category: 'read' },
  read_artifact: { icon: FileText, verb: '正在读取归档', done: '读取归档', category: 'read' },
  ls: { icon: FolderTree, verb: '正在列出', done: '列出', category: 'read' },
  grep: { icon: Search, verb: '正在搜索', done: '搜索', category: 'search' },
  edit: { icon: FileEdit, verb: '正在编辑', done: '编辑', category: 'edit' },
  write: { icon: FilePlus2, verb: '正在写入', done: '写入', category: 'edit' },
  bash: { icon: Terminal, verb: '正在运行', done: '运行', category: 'command' },
  delegate: { icon: Bot, verb: '子助手处理中', done: '子助手', category: 'agent' },
  skills_list: { icon: Sparkles, verb: '正在查看技能', done: '查看技能', category: 'other' },
  skill_load: { icon: Sparkles, verb: '正在加载技能', done: '加载技能', category: 'other' },
  skill_unload: { icon: Sparkles, verb: '正在卸载技能', done: '卸载技能', category: 'other' },
  skill_resource: { icon: BookOpen, verb: '正在读取技能资源', done: '读取技能资源', category: 'read' },
  memory_list: { icon: Brain, verb: '正在查看记忆', done: '查看记忆', category: 'other' },
  task_create: { icon: ListChecks, verb: '正在创建任务', done: '创建任务', category: 'task' },
  task_list: { icon: ListChecks, verb: '正在查看任务', done: '查看任务', category: 'task' },
  task_claim: { icon: ListChecks, verb: '正在认领任务', done: '认领任务', category: 'task' },
  task_complete: { icon: ListChecks, verb: '正在完成任务', done: '完成任务', category: 'task' },
}

export function toolMeta(name = ''): ToolMeta {
  if (meta[name]) return meta[name]
  if (name.startsWith('mcp__')) return { icon: Plug, verb: '正在调用', done: '调用', category: 'other' }
  return { icon: name.includes('search') ? FileSearch : Wrench, verb: '正在调用', done: '调用', category: 'other' }
}

export function toolLabel(name = '') {
  if (name.startsWith('mcp__')) return name.split('__').slice(1).join(' · ')
  return name
}

export function toolTarget(item: Pick<FeedItem, 'name' | 'args'>) {
  const args = item.args || {}
  const value = args.path ?? args.file_path ?? args.command ?? args.pattern ?? args.task ?? args.title ?? args.name ?? args.kind ?? args.task_id
  if (item.name === 'grep' && args.pattern) return `${args.pattern}${args.path && args.path !== '.' ? `  in ${args.path}` : ''}`
  if (item.name === 'ls' && !args.path) return '.'
  return value === undefined ? '' : String(value)
}

export function toolCategory(name = '') { return toolMeta(name).category }

export function summarizeTools(items: FeedItem[]) {
  const counts = { read: 0, search: 0, edit: 0, command: 0, agent: 0, task: 0, other: 0 }
  for (const item of items) counts[toolCategory(item.name)]++
  const parts = [
    counts.edit && `编辑 ${counts.edit} 个文件`, counts.read && `读取 ${counts.read} 次`, counts.search && `搜索 ${counts.search} 次`,
    counts.command && `运行 ${counts.command} 条命令`, counts.agent && `委派 ${counts.agent} 次`, counts.task && `更新任务 ${counts.task} 次`,
    counts.other && `其他 ${counts.other} 项`,
  ].filter(Boolean)
  return parts.join('，') || `${items.length} 个步骤`
}

/** Short language id for a path, used by syntax highlighting. */
export function languageFor(path: string) {
  const extension = path.split('.').pop()?.toLowerCase() || ''
  const map: Record<string, string> = {
    ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', cjs: 'javascript', mjs: 'javascript',
    py: 'python', rs: 'rust', go: 'go', java: 'java', kt: 'kotlin', rb: 'ruby', php: 'php', cs: 'csharp', c: 'c', h: 'c',
    cpp: 'cpp', hpp: 'cpp', css: 'css', scss: 'scss', html: 'xml', xml: 'xml', svg: 'xml', json: 'json', md: 'markdown',
    yaml: 'yaml', yml: 'yaml', toml: 'ini', ini: 'ini', sh: 'bash', bash: 'bash', ps1: 'powershell', sql: 'sql', swift: 'swift',
  }
  return map[extension] || ''
}
