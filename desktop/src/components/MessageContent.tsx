import { memo } from 'react'
import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

const plugins = [remarkGfm]
const components = {
  a: ({ children, href }: { children?: React.ReactNode; href?: string }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
  pre: ({ children }: { children?: React.ReactNode }) => <div className="message-code"><pre>{children}</pre></div>,
}

const MessageContent = memo(function MessageContent({ text }: { text: string }) {
  return <Markdown remarkPlugins={plugins} components={components}>{text}</Markdown>
})

export default MessageContent
