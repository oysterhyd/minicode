export class InputHistory {
  private entries: string[] = []
  private index = 0
  private draft = ''
  add(text: string) {
    if (text.trim() && this.entries.at(-1) !== text) this.entries.push(text)
    if (this.entries.length > 1000) this.entries.shift()
    this.index = this.entries.length
  }
  previous(draft: string) {
    if (this.index === this.entries.length) this.draft = draft
    this.index = Math.max(0, this.index - 1)
    return this.entries[this.index] ?? draft
  }
  next() {
    this.index = Math.min(this.entries.length, this.index + 1)
    return this.entries[this.index] ?? this.draft
  }
}
export class PasteStore {
  private sequence = 0
  private values = new Map<number, string>()
  fold(text: string) {
    if (text.length < 4000 && text.split('\n').length < 20) return text
    const id = ++this.sequence
    this.values.set(id, text)
    return `[paste:${id}]`
  }
  expand(text: string) {
    return text.replace(/\[paste:(\d+)\]/g, (_, value) => {
      const stored = this.values.get(Number(value))
      if (stored === undefined) throw Error(`粘贴 ${value} 已取消，请删除引用或重新粘贴`)
      return stored
    })
  }
  preview(id: number) { return this.values.get(id)?.slice(0, 4000) ?? '' }
  cancel(id: number) { this.values.delete(id) }
}
export function fileMention(text: string): { start: number; query: string } | undefined {
  const match = /(?:^|\s)@(?:"([^"]*)"?|([^\s]*))$/.exec(text)
  if (!match) return
  return { start: text.lastIndexOf('@', match.index + match[0].length), query: match[1] ?? match[2] }
}
