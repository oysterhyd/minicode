import { memo, useMemo, type ReactNode } from 'react'
import Markdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { highlight } from '../lib/highlight'
import { CopyButton } from './CopyButton'

const plugins = [remarkGfm]

function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(textOf).join('')
  if (node && typeof node === 'object' && 'props' in node) return textOf((node as { props: { children?: ReactNode } }).props.children)
  return ''
}

function CodeBlock({ code, language }: { code: string; language: string }) {
  const highlighted = useMemo(() => highlight(code, language || undefined), [code, language])
  return <div className="code-block">
    <div className="code-block-header"><span>{language || 'text'}</span><CopyButton text={code} label="复制代码" /></div>
    <pre><code className="hljs">{highlighted}</code></pre>
  </div>
}

const components: Components = {
  a: ({ children, href }) => <a href={href} onClick={event => {
    event.preventDefault()
    if (href) void window.desktop.request('openExternal', { url: href }).catch(() => window.open(href, '_blank', 'noopener'))
  }}>{children}</a>,
  pre: ({ children }) => {
    const child = Array.isArray(children) ? children[0] : children
    const className = child && typeof child === 'object' && 'props' in child ? String((child as { props: { className?: string } }).props.className || '') : ''
    return <CodeBlock code={textOf(children).replace(/\n$/, '')} language={className.match(/language-([\w+#-]+)/)?.[1] || ''} />
  },
  table: ({ children }) => <div className="table-wrap"><table>{children}</table></div>,
}

const Block = memo(function Block({ text }: { text: string }) {
  return <Markdown remarkPlugins={plugins} components={components}>{text}</Markdown>
})

/** Split markdown at blank lines outside fenced code, so finished blocks never re-render while streaming. */
export function splitBlocks(text: string) {
  const blocks: string[] = []
  let current: string[] = [], fence: string | null = null
  for (const line of text.split('\n')) {
    const marker = line.match(/^\s*(`{3,}|~{3,})/)?.[1]
    if (marker && fence === null) fence = marker
    else if (marker && marker[0] === fence?.[0] && marker.length >= fence.length && /^\s*[`~]+\s*$/.test(line)) fence = null
    if (!fence && line.trim() === '' && current.length && !/^\s*([-*+]|\d+[.)])\s/.test(current[current.length - 1] || '')) {
      blocks.push(current.join('\n')); current = []
      continue
    }
    current.push(line)
  }
  if (current.length) blocks.push(current.join('\n'))
  return blocks
}

const MessageContent = memo(function MessageContent({ text, streaming = false }: { text: string; streaming?: boolean }) {
  if (!streaming) return <Block text={text} />
  const blocks = splitBlocks(text)
  return <>{blocks.map((block, index) => <Block key={index} text={block} />)}</>
})

export default MessageContent
