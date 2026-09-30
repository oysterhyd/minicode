import type { AgentState } from '../../types'
import { compactNumber } from '../../lib/format'

export const tpsLabel = (value: number | null | undefined) => value == null ? '未报告' : `${value.toFixed(1)} tok/s`
export function cacheRate(state: Pick<AgentState, 'usage'>) {
  const usage = state.usage
  return !usage || usage.available === false || !usage.input_tokens ? null : Math.min(100, 100 * (usage.cache_read_tokens || 0) / usage.input_tokens)
}

export function SessionUsage({ state }: { state: AgentState }) {
  const usage = state.usage
  const stats = state.statistics
  const known = usage && usage.available !== false
  const rate = cacheRate(state)
  const samples = stats?.samples || []
  const max = Math.max(1, ...samples.filter(s => s.available).map(s => s.input + s.output))
  const speeds = samples.filter(s => s.tps != null)
  const maxTps = Math.max(1, ...speeds.map(s => s.tps!))
  const total = (usage?.input_tokens || 0) + (usage?.output_tokens || 0)
  const inputWidth = total ? 100 * (usage?.input_tokens || 0) / total : 0
  return <section className="session-usage" aria-label="会话统计">
    <div className="work-section-label work-usage-label"><span>会话统计</span><span>{stats?.requests || 0} 次请求</span></div>
    <div className="usage-composition" role="img" aria-label={known ? `输入 ${usage.input_tokens}，输出 ${usage.output_tokens} token` : '用量未报告'}><span className="usage-input" style={{ width: `${known ? inputWidth : 0}%` }} /><span className="usage-output" style={{ width: `${known && total ? 100 - inputWidth : 0}%` }} /></div>
    <div className="usage-grid">
      <div><small>输入 token</small><strong>{known ? compactNumber(usage.input_tokens) : '未报告'}</strong></div>
      <div><small>输出 token</small><strong>{known ? compactNumber(usage.output_tokens) : '未报告'}</strong></div>
      <div><small>缓存命中率</small><strong>{rate == null ? '未报告' : `${rate.toFixed(1)}%`}</strong></div>
      <div><small>平均输出 TPS</small><strong>{tpsLabel(stats?.tps)}</strong></div>
    </div>
    <div className="usage-cache-track" role="meter" aria-label="缓存命中率" aria-valuemin={0} aria-valuemax={100} aria-valuenow={rate ?? undefined} aria-valuetext={rate == null ? '未报告' : `${rate.toFixed(1)}%`}><span style={{ width: `${rate || 0}%` }} /></div>
    <div className="usage-facts"><span>轮次 <b>{state.rounds}</b></span><span>工具调用 <b>{stats?.toolCalls || 0}</b></span><span>缓存读取 <b>{known ? compactNumber(usage.cache_read_tokens || 0) : '—'}</b></span><span>缓存写入 <b>{known ? compactNumber(usage.cache_write_tokens || 0) : '—'}</b></span></div>
    {samples.length > 0 && <>
      <div className="usage-chart-heading"><strong>每轮 token</strong><span><i className="usage-dot usage-input" />非缓存输入 <i className="usage-dot usage-cached" />缓存 <i className="usage-dot usage-output" />输出</span></div>
      <div className="usage-bars" aria-label="最近 32 次请求的 token 分布">{samples.map(s => <div className="usage-bar-column" key={s.round} tabIndex={0} aria-label={`第 ${s.round} 次请求，${s.available ? `输入 ${s.input}，缓存 ${s.cached}，输出 ${s.output}` : '用量未报告'}`}>
        <div className="usage-bar" title={`#${s.round} · 输入 ${s.input.toLocaleString()} · 缓存 ${s.cached.toLocaleString()} · 输出 ${s.output.toLocaleString()}`}>
          {s.available ? <><span className="usage-output" style={{ height: `${100 * s.output / max}%` }} /><span className="usage-cached" style={{ height: `${100 * Math.min(s.input, s.cached) / max}%` }} /><span className="usage-input" style={{ height: `${100 * Math.max(0, s.input - s.cached) / max}%` }} /></> : <span className="usage-unknown" />}
        </div><small>{s.round}</small>
      </div>)}</div>
      {speeds.length > 0 && <><div className="usage-chart-heading"><strong>输出速度</strong><span>最近 {tpsLabel(stats?.lastTps)}</span></div>
        <svg className="usage-speed-chart" viewBox="0 0 300 68" role="img" aria-label="最近请求的输出 TPS 趋势"><title>平均输出 TPS，包含首字等待时间；具体数据见下方明细</title><line x1="0" x2="300" y1="60" y2="60" /><polyline fill="none" points={samples.flatMap((s, i) => s.tps == null ? [] : [`${samples.length === 1 ? 150 : 4 + i * 292 / (samples.length - 1)},${60 - s.tps / maxTps * 52}`]).join(' ')} />{samples.map((s, i) => s.tps == null ? null : <circle key={s.round} cx={samples.length === 1 ? 150 : 4 + i * 292 / (samples.length - 1)} cy={60 - s.tps / maxTps * 52} r="3"><title>#{s.round}: {tpsLabel(s.tps)}</title></circle>)}</svg>
      </>}
      <details className="usage-table"><summary>请求明细（最近 {samples.length} 次）</summary><div className="scrollbar"><table><thead><tr><th>轮次</th><th>输入</th><th>输出</th><th>缓存</th><th>TPS</th></tr></thead><tbody>{samples.map(s => <tr key={s.round}><th>{s.round}</th><td>{s.available ? s.input.toLocaleString() : '—'}</td><td>{s.available ? s.output.toLocaleString() : '—'}</td><td>{s.available ? s.cached.toLocaleString() : '—'}</td><td>{s.tps?.toFixed(1) ?? '—'}</td></tr>)}</tbody></table></div></details>
    </>}
    <p className="usage-note">TPS = 输出 token / 请求耗时（含首字等待）。历史请求未记录耗时时不计算速度。</p>
  </section>
}
