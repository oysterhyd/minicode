import Markdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

export default function MessageContent({ text }: { text: string }) {
  return <Markdown remarkPlugins={[remarkGfm]} components={{
    a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
    pre: ({ children }) => <div className="message-code"><pre>{children}</pre></div>,
  }}>{text}</Markdown>
}
