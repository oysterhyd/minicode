import type { BridgeApi } from './bridge.js'
export type ArtifactPage = { text: string; hasMore: boolean; total: number | null }

export class ArtifactPages {
  page: ArtifactPage = { text: '', hasMore: false, total: null }
  offset = 0
  private offsets: number[] = []
  constructor(private client: BridgeApi, private sessionId: string, private artifactId: string) {}
  async load(offset: number): Promise<ArtifactPage> {
    const page = await this.client.request<ArtifactPage>('readArtifact', { sessionId: this.sessionId, artifactId: this.artifactId, offset })
    if (!page || typeof page.text !== 'string') throw Error('无效的归档页')
    this.page = page; this.offset = offset
    return page
  }
  async next() {
    if (!this.page.hasMore) return this.page
    const old = this.offset
    const page = await this.load(old + [...this.page.text].length)
    this.offsets.push(old)
    return page
  }
  async previous() {
    const offset = this.offsets.at(-1)
    if (offset === undefined) return this.page
    const page = await this.load(offset)
    this.offsets.pop()
    return page
  }
}
