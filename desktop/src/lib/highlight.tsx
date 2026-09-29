import { Fragment, type ReactNode } from 'react'
import { common, createLowlight } from 'lowlight'

const lowlight = createLowlight(common)
type HastNode = { type: string; value?: string; tagName?: string; properties?: { className?: string[] }; children?: HastNode[] }

function render(node: HastNode, key: number): ReactNode {
  if (node.type === 'text') return node.value
  const children = node.children?.map(render)
  if (node.type === 'element') return <span key={key} className={node.properties?.className?.join(' ')}>{children}</span>
  return <Fragment key={key}>{children}</Fragment>
}

/** Highlight source into React nodes. Unknown or huge input is returned as plain text. */
export function highlight(code: string, language?: string): ReactNode {
  if (!code || code.length > 120_000) return code
  try {
    const tree = language && lowlight.registered(language) ? lowlight.highlight(language, code) : code.length < 20_000 ? lowlight.highlightAuto(code) : null
    return tree ? render(tree as unknown as HastNode, 0) : code
  } catch { return code }
}

/** Highlight a file line by line so line numbers stay aligned with tokens. */
export function highlightLines(code: string, language?: string): ReactNode[] {
  const lines = code.replace(/\r\n/g, '\n').split('\n')
  if (!language || !lowlight.registered(language) || code.length > 120_000) return lines
  // Highlight the whole file once for correct multi-line tokens, then split the tree by newlines.
  let tree: HastNode
  try { tree = lowlight.highlight(language, lines.join('\n')) as unknown as HastNode } catch { return lines }
  const out: ReactNode[][] = [[]]
  let key = 0
  function walk(node: HastNode, wrap: (child: ReactNode) => ReactNode) {
    if (node.type === 'text') {
      node.value!.split('\n').forEach((part, index) => {
        if (index > 0) out.push([])
        if (part) out[out.length - 1].push(wrap(part))
      })
      return
    }
    const className = node.properties?.className?.join(' ')
    const next = node.type === 'element' ? (child: ReactNode) => wrap(<span key={key++} className={className}>{child}</span>) : wrap
    node.children?.forEach(child => walk(child, next))
  }
  walk(tree, child => child)
  return out.map((parts, index) => <Fragment key={index}>{parts}</Fragment>)
}
