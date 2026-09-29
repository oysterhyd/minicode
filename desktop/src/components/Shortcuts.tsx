import { X } from 'lucide-react'
import { Modal } from './ui/Modal'
import { modKey } from '../lib/format'

export const shortcuts: Array<[string, Array<[string, string]>]> = [
  ['常用', [[`${modKey} K`, '搜索操作与任务'], [`${modKey} N`, '新建任务'], [`${modKey} ,`, '打开设置'], [`${modKey} /`, '查看快捷键'], [`${modKey} Shift F`, '搜索任务']]],
  ['任务', [['Enter', '发送（可在设置中改为 Ctrl Enter）'], ['Shift Enter', '换行'], [`${modKey} .`, '停止当前任务'], ['↑ / ↓', '在空输入框中浏览历史'], ['/  ·  @', '命令与文件引用'], [`${modKey} L`, '清空当前视图']]],
  ['审批', [['Y', '批准执行'], ['A', '本会话始终允许该工具'], ['N / Esc', '拒绝']]],
  ['界面', [[`${modKey} B`, '收拢任务侧栏'], [`${modKey} Shift B`, '收拢工作台'], [`${modKey} 1-4`, '切换工作台标签'], [`${modKey} Shift L`, '切换深浅主题'], [`${modKey} Shift [ / ]`, '切换到上 / 下一个任务']]],
]

export function ShortcutList() {
  return <div className="shortcut-grid">{shortcuts.map(([group, items]) => <section key={group}>
    <h3>{group}</h3>
    {items.map(([keys, label]) => <div className="shortcut-row" key={label}><span>{label}</span><span className="shortcut-keys">{keys.split(' ').filter(Boolean).map((key, index) => <kbd key={index}>{key}</kbd>)}</span></div>)}
  </section>)}</div>
}

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return <Modal label="键盘快捷键" className="shortcuts-dialog" onClose={onClose}>
    <header className="dialog-header"><h2>键盘快捷键</h2><button className="icon-button" onClick={onClose} aria-label="关闭"><X size={16} /></button></header>
    <ShortcutList />
  </Modal>
}
