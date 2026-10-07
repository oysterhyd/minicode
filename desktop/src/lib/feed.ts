import type { FeedItem } from '../types'

export type FeedRow = { type: 'item'; item: FeedItem; position: number } | { type: 'tools'; id: string; items: FeedItem[] }

/** One pass retains source positions; rendering must never search history per row. */
export function groupFeedItems(items: FeedItem[]): FeedRow[] {
  const result: FeedRow[] = []
  for (let position = 0; position < items.length; position++) {
    const item = items[position]
    const last = result[result.length - 1]
    if (item.kind === 'tool') {
      if (last?.type === 'tools') last.items.push(item)
      else result.push({ type: 'tools', id: `tools-${item.id}`, items: [item] })
    } else result.push({ type: 'item', item, position })
  }
  return result
}
