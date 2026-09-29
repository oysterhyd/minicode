import { Component, type ReactNode } from 'react'
import { RotateCcw, TriangleAlert } from 'lucide-react'

export class ErrorBoundary extends Component<{ children: ReactNode; label?: string; compact?: boolean }, { error: Error | null }> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) { return { error } }
  componentDidCatch(error: Error) { console.error(error) }
  render() {
    if (!this.state.error) return this.props.children
    return <div className={`error-boundary ${this.props.compact ? 'error-boundary-compact' : ''}`} role="alert">
      <TriangleAlert size={this.props.compact ? 16 : 28} />
      <strong>{this.props.label || '界面出现问题'}</strong>
      <p>{this.state.error.message}</p>
      <button className="button button-ghost" onClick={() => this.setState({ error: null })}><RotateCcw size={14} />重试</button>
    </div>
  }
}
