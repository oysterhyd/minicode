import type { Command, ModelInfo, SessionSummary, SkillInfo } from './types.js'

export type Suggestion = { value: string; detail: string; kind: 'command' | 'value' }

export const SUBMENUS = ['/model', '/effort', '/permissions', '/resume', '/skill'] as const

export const SUBMENU_TITLES: Record<string, string> = {
  '/model': '选择模型',
  '/effort': '选择推理预算',
  '/permissions': '选择权限模式',
  '/resume': '选择要恢复的会话',
  '/skill': '选择技能',
}

export function isSubmenu(value: string): boolean {
  return (SUBMENUS as readonly string[]).includes(value)
}

const EFFORT = ['off', 'low', 'medium', 'high', 'xhigh', 'max']
const PERMISSIONS: Array<[string, string]> = [
  ['default', '逐项确认'],
  ['accept_edits', '自动编辑'],
  ['bypass', '全部允许'],
]

export function slashSuggestions(draft: string, context: {
  commands: Command[]
  models: ModelInfo[]
  sessions: SessionSummary[]
  skills: SkillInfo[]
}): Suggestion[] {
  if (!draft.startsWith('/') || draft.includes('\n')) return []
  const verb = draft.split(/\s/, 1)[0].toLowerCase()
  const space = draft.search(/\s/)
  const hasSpace = space >= 0
  const argument = hasSpace ? draft.slice(space).trimStart() : ''
  const unloading = verb === '/skill' && argument.startsWith('off ')
  const query = (unloading ? argument.slice(4) : argument).toLowerCase()
  if (isSubmenu(verb) && (hasSpace || draft === verb)) {
    const values = verb === '/model'
      ? context.models.map(item => ({ value: item.id, detail: item.available === false ? '未配置' : item.provider || '' }))
      : verb === '/effort'
        ? EFFORT.map(value => ({ value, detail: '思考强度' }))
        : verb === '/permissions'
          ? PERMISSIONS.map(([value, detail]) => ({ value, detail }))
          : verb === '/resume'
            ? context.sessions.slice(0, 30).map(item => ({
              value: item.session_id,
              detail: item.title || item.session_id,
            }))
            : context.skills.map(item => ({ value: unloading ? `off ${item.name}` : item.name, detail: item.description }))
    return values
      .filter(item => `${item.value} ${item.detail}`.toLowerCase().includes(query))
      .slice(0, 12)
      .map(item => ({ ...item, kind: 'value' as const }))
  }
  if (hasSpace) return []
  return context.commands
    .filter(command => command.name.startsWith(verb))
    .slice(0, 12)
    .map(command => ({ value: command.name, detail: command.summary, kind: 'command' as const }))
}

export function applySuggestion(draft: string, option: Suggestion): { kind: 'fill' | 'send'; text: string } {
  if (option.kind === 'command' && isSubmenu(option.value)) {
    return { kind: 'fill', text: `${option.value} ` }
  }
  if (option.kind === 'value') {
    const verb = draft.split(/\s/, 1)[0]
    return { kind: 'send', text: `${verb} ${option.value}` }
  }
  return { kind: 'send', text: option.value }
}
