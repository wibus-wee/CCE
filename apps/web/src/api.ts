export type ViewState = 'building' | 'ready' | 'partial' | 'stale' | 'unavailable' | 'failed'

export interface Capability {
  name: string
  level: string
  reason?: string
}

export interface ViewStatus {
  state: ViewState
  snapshotId: string
  updatedAt: string
  capabilities: Capability[]
  message?: string
}

export interface ViewManifest {
  repositoryId: string
  snapshotId: string
  views: Record<string, ViewStatus>
}

export interface SourceAddress {
  path: string
  startLine: number
  endLine: number
  startByte: number
  endByte: number
}

export interface ContextItem {
  id: string
  kind: string
  title: string
  body: string
  estimatedTokens: number
  provenance: {
    whyRetrieved: string
    route: string
    rank: number
    score: number
    verifiedCurrent: boolean
    sourceAddress?: SourceAddress
  }
}

export interface ContextPack {
  snapshotId: string
  intent: string
  budgetTokens: number
  usedTokens: number
  items: ContextItem[]
  uncertainties: Array<{ capability: string; message: string }>
  missingCapabilities: string[]
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: { 'content-type': 'application/json', ...init?.headers },
  })
  const body = (await response.json()) as T | { error?: string }
  if (!response.ok) {
    const message = typeof body === 'object' && body !== null && 'error' in body ? body.error : undefined
    throw new Error(message || `${response.status} ${response.statusText}`)
  }
  return body as T
}

export const api = {
  index: () => request<{ manifest: ViewManifest }>('/v1/index', { method: 'POST' }),
  status: () => request<ViewManifest>('/v1/status'),
  context: (query: string, budgetTokens: number) =>
    request<ContextPack>('/v1/context', {
      method: 'POST',
      body: JSON.stringify({ query, budgetTokens, maxCandidates: 50, requireFresh: true }),
    }),
}
