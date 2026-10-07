import type { FeedItem } from './types.js'

type ToolMeta = { verb: string; done: string }

const meta: Record<string, ToolMeta> = {
  read: { verb: '读取', done: 'Read' },
  read_artifact: { verb: '读取归档', done: 'Read' },
  ls: { verb: '列出', done: 'List' },
  grep: { verb: '搜索', done: 'Grep' },
  edit: { verb: '编辑', done: 'Edit' },
  write: { verb: '写入', done: 'Write' },
  bash: { verb: '运行', done: 'Bash' },
  delegate: { verb: '子助手', done: 'Delegate' },
  skills_list: { verb: '查看技能', done: 'Skills' },
  skill_load: { verb: '加载技能', done: 'Skill' },
  skill_unload: { verb: '卸载技能', done: 'Skill' },
  skill_resource: { verb: '读取技能资源', done: 'Skill' },
  memory_list: { verb: '查看记忆', done: 'Memory' },
  task_create: { verb: '创建任务', done: 'Task' },
  task_list: { verb: '查看任务', done: 'Task' },
  task_claim: { verb: '认领任务', done: 'Task' },
  task_complete: { verb: '完成任务', done: 'Task' },
}

export function toolMeta(name = ''): ToolMeta {
  if (meta[name]) return meta[name]
  if (name.startsWith('mcp__')) return { verb: '调用', done: name.split('__').slice(1).join(' · ') }
  return { verb: '调用', done: name || 'Tool' }
}

export function toolTarget(item: Pick<FeedItem, 'name' | 'args'>): string {
  const args = item.args || {}
  const value = args.path ?? args.file_path ?? args.command ?? args.pattern ?? args.task ?? args.title ?? args.name ?? args.kind ?? args.task_id
  if (item.name === 'grep' && args.pattern) return `${args.pattern}${args.path && args.path !== '.' ? `  in ${args.path}` : ''}`
  if (item.name === 'ls' && !args.path) return '.'
  return value === undefined ? '' : String(value)
}

export function toolMark(item: FeedItem): string {
  if (item.pending) return '◌'
  if (item.interrupted) return '◼'
  if (item.success === false) return '✗'
  return '✓'
}
