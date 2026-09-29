import { memo, useMemo, useState } from 'react'
import { ChevronRight, FileCode2, FileJson, FileText, Folder, FolderOpen, Image } from 'lucide-react'
import { basename, dirname, pathScore } from '../../lib/format'

type Node = { name: string; path: string; children?: Map<string, Node> }

function build(files: string[]) {
  const root: Node = { name: '', path: '', children: new Map() }
  for (const file of files) {
    let node = root
    const parts = file.split('/')
    parts.forEach((part, index) => {
      const path = parts.slice(0, index + 1).join('/')
      if (!node.children!.has(part)) node.children!.set(part, index === parts.length - 1 ? { name: part, path } : { name: part, path, children: new Map() })
      node = node.children!.get(part)!
    })
  }
  return root
}

function sortNodes(children: Map<string, Node>) {
  return [...children.values()].sort((a, b) => Number(Boolean(b.children)) - Number(Boolean(a.children)) || a.name.localeCompare(b.name))
}

export function FileIcon({ path, size = 14 }: { path: string; size?: number }) {
  const extension = path.split('.').pop()?.toLowerCase()
  if (['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'ico'].includes(extension || '')) return <Image size={size} />
  if (['json', 'yaml', 'yml', 'toml', 'lock'].includes(extension || '')) return <FileJson size={size} />
  if (['md', 'txt', 'rst'].includes(extension || '')) return <FileText size={size} />
  return <FileCode2 size={size} />
}

const TreeLevel = memo(function TreeLevel({ node, depth, open, toggle, onOpen, changed }: {
  node: Node; depth: number; open: Set<string>; toggle: (path: string) => void; onOpen: (path: string) => void; changed: Map<string, string>
}) {
  return <>{sortNodes(node.children!).map(child => {
    const isDir = Boolean(child.children)
    const expanded = open.has(child.path)
    const status = changed.get(child.path)
    return <div key={child.path} role="none">
      <button role="treeitem" aria-expanded={isDir ? expanded : undefined} className={`tree-row ${status ? 'is-changed' : ''}`} style={{ paddingLeft: 8 + depth * 14 }}
        onClick={() => isDir ? toggle(child.path) : onOpen(child.path)} title={child.path}>
        {isDir ? <ChevronRight size={12} className={`tree-chevron ${expanded ? 'is-open' : ''}`} /> : <span className="tree-spacer" />}
        {isDir ? expanded ? <FolderOpen size={14} className="tree-folder" /> : <Folder size={14} className="tree-folder" /> : <FileIcon path={child.path} />}
        <span className="truncate">{child.name}</span>
        {status && <span className={`tree-status status-${status}`}>{status}</span>}
      </button>
      {isDir && expanded && <div role="group"><TreeLevel node={child} depth={depth + 1} open={open} toggle={toggle} onOpen={onOpen} changed={changed} /></div>}
    </div>
  })}</>
})

export function FileTree({ files, query, onOpen, changed }: { files: string[]; query: string; onOpen: (path: string) => void; changed: Map<string, string> }) {
  const tree = useMemo(() => build(files), [files])
  const [open, setOpen] = useState<Set<string>>(() => new Set())
  const toggle = (path: string) => setOpen(previous => { const next = new Set(previous); if (next.has(path)) next.delete(path); else next.add(path); return next })
  const results = useMemo(() => query.trim() ? files.map(file => ({ file, score: pathScore(file, query) }))
    .filter(entry => entry.score >= 0).sort((a, b) => b.score - a.score).slice(0, 200).map(entry => entry.file) : null, [files, query])
  if (results) return <div className="file-results" role="list">
    {results.map(file => <button key={file} role="listitem" className="file-row" onClick={() => onOpen(file)} title={file}>
      <FileIcon path={file} /><span className="file-row-label"><strong>{basename(file)}</strong><small>{dirname(file) || '项目根目录'}</small></span>
    </button>)}
    {!results.length && <div className="work-empty"><strong>未找到匹配文件</strong><p>尝试文件名或路径的一部分。</p></div>}
  </div>
  return <div className="file-tree" role="tree" aria-label="项目文件"><TreeLevel node={tree} depth={0} open={open} toggle={toggle} onOpen={onOpen} changed={changed} /></div>
}
