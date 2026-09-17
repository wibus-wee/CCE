export type ViewState = 'building' | 'ready' | 'partial' | 'stale' | 'unavailable' | 'failed'

export type ViewKind =
  | 'source'
  | 'lexical'
  | 'dense'
  | 'symbols'
  | 'graph'
  | 'history'
  | 'knowledge'
  | 'dataflow'

export interface Capability {
  name: string
  level: string
  reason?: string
}

export interface ViewStatus {
  state: ViewState
  snapshotId: string
  profileHash: string
  updatedAt: string
  capabilities: Capability[]
  artifactDigest?: string
  message?: string
}

export interface ViewManifest {
  repositoryId: string
  snapshotId: string
  views: Record<string, ViewStatus>
}

export interface SourceAddress {
  repositoryId: string
  snapshotId: string
  path: string
  startByte: number
  endByte: number
  startLine: number
  endLine: number
  symbolId?: string
}

// --- search ---------------------------------------------------------------

export type QueryIntent =
  | 'exact_entity'
  | 'natural_language_behavior'
  | 'issue_localization'
  | 'trace'
  | 'impact'
  | 'architecture'
  | 'history'
  | 'precise_dataflow'
  | 'unknown'

export type SearchRoute =
  | 'no_retrieval'
  | 'exact_symbol'
  | 'lexical'
  | 'dense_raw'
  | 'dense_summary'
  | 'hybrid'
  | 'structural'
  | 'knowledge'
  | 'history'
  | 'diff'
  | 'reranked'

export type RetrievalRepresentation =
  | 'raw_code'
  | 'signature'
  | 'file_descriptor'
  | 'symbol_summary'
  | 'role_summary'
  | 'module_summary'
  | 'flow_summary'
  | 'test_behavior'
  | 'commit_summary'
  | 'commit_diff'
  | 'knowledge_page'

export type GraphPolicy =
  | 'none'
  | 'outgoing_trace'
  | 'incoming_impact'
  | 'architecture_boundary'
  | 'dataflow_required'

// Body of POST /v1/search. The daemon resolves repository/snapshot itself
// and always serves fresh-or-failed results; `lang:`/`path:` tokens in the
// query become structured filters engine-side.
export interface SearchInput {
  query: string
  intent?: QueryIntent
  limit?: number
  routes?: SearchRoute[]
}

export interface QueryFilters {
  pathPrefix?: string
  language?: string
  // `type:diff`/`type:commit`/`type:file` — echoed back when parsed from the
  // query text.
  type?: string
}

// The effective request echoed back inside SearchResult.
export interface SearchRequest {
  repositoryId: string
  snapshotId: string
  query: string
  intent?: QueryIntent
  limit: number
  requireFresh: boolean
  routes: SearchRoute[]
  filters: QueryFilters
}

export interface QueryPlan {
  intent: QueryIntent
  routes: SearchRoute[]
  graphPolicy: GraphPolicy
  requiredViews: ViewKind[]
  reasons: string[]
}

export interface SearchHit {
  documentId: string
  entityId: string
  regionId?: string
  symbolName?: string
  representation: RetrievalRepresentation
  route: SearchRoute
  rank: number
  score: number
  contributingRoutes: SearchRoute[]
  address?: SourceAddress
  evidence: SourceAddress[]
  snippet: string
  verifiedCurrent: boolean
  explanation: string[]
}

export interface SearchResult {
  request: SearchRequest
  plan: QueryPlan
  manifest: ViewManifest
  hits: SearchHit[]
  missingCapabilities: string[]
  latencyMs: number
}

// --- providers --------------------------------------------------------------

export type ProviderState = 'ready' | 'missing' | 'not_applicable' | 'failed'

export interface ProviderReport {
  providerId: string
  state: ProviderState
  tool?: string
  message?: string
  artifactDigest?: string
  durationMs?: number
  scipDocuments: number
  scipDefinitions: number
  scipReferenceEdges: number
}

// --- symbol navigation ------------------------------------------------------

export interface DefinitionHit {
  name: string
  kind: string
  qualifiedName?: string
  language?: string
  address?: SourceAddress
}

export interface DefinitionsReport {
  query: string
  snapshotId: string
  definitions: DefinitionHit[]
}

export interface ReferenceHit {
  fromName: string
  fromQualifiedName?: string
  fromKind: string
  via: string
  origin: string
  confidence: number
  evidence: SourceAddress[]
}

export interface ReferencesReport {
  query: string
  snapshotId: string
  targets: DefinitionHit[]
  references: ReferenceHit[]
  truncated: boolean
}

// --- indexing ----------------------------------------------------------------

export interface SnapshotIdentity {
  id: string
  repositoryId: string
  baseRevision?: string
  workspaceOverlayHash: string
  indexProfileHash: string
  createdAt: string
  fileCount: number
  sourceBytes: number
}

export interface IndexReport {
  repositoryId: string
  snapshot: SnapshotIdentity
  reusedSnapshot: boolean
  indexedFiles: number
  parsedFiles: number
  reusedFileAnalyses: number
  sourceUnits: number
  relations: number
  retrievalDocuments: number
  skippedLargeFiles: string[]
  skippedBinaryFiles: string[]
  skippedSensitiveFiles: string[]
  skippedBuiltinFiles: [string, string][]
  providers: ProviderReport[]
  manifest: ViewManifest
}

// --- context pack -------------------------------------------------------------

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

// --- gateway registry & code browsing -----------------------------------------

export interface PushRecord {
  pushId: string
  at: string
  revision?: string
  fileCount: number
  bytes: number
}

export interface RepoEntry {
  id: string
  name: string
  slug: string
  workerUrl: string
  createdAt: string
  lastPush?: PushRecord
}

export interface FileListEntry {
  path: string
  language?: string
}

export interface FileListReport {
  snapshotId: string
  files: FileListEntry[]
}

export interface FileContent {
  snapshotId: string
  path: string
  language?: string
  content: string
  truncated: boolean
  binary: boolean
}

// Request prefix: '' talks to a standalone daemon directly, '/{slug}' routes
// through the gateway's per-repo proxy. Set once at startup after discovery.
let base = ''

export function setBase(prefix: string) {
  base = prefix
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(base + path, {
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

// Network failure surfaces as TypeError; HTTP failure carries the daemon's
// `error` message. "Daemon down" and "view unavailable" need different
// remedies, so keep them distinguishable in the UI.
export function describeError(value: unknown): string {
  if (value instanceof TypeError) {
    return 'Cannot reach the CCE service — is `cce-daemon` or `cce-gateway` running?'
  }
  return value instanceof Error ? value.message : String(value)
}

export const api = {
  index: () => request<IndexReport>('/v1/index', { method: 'POST' }),
  status: () => request<ViewManifest>('/v1/status'),
  search: (input: SearchInput) =>
    request<SearchResult>('/v1/search', {
      method: 'POST',
      body: JSON.stringify(input),
    }),
  context: (query: string, budgetTokens: number) =>
    request<ContextPack>('/v1/context', {
      method: 'POST',
      body: JSON.stringify({ query, budgetTokens, maxCandidates: 50, requireFresh: true }),
    }),
  providers: () => request<ProviderReport[]>('/v1/providers'),
  // Query-time regex over stored commit patches — same SearchResult shape
  // as /v1/search pinned to the diff route.
  diff: (pattern: string, limit?: number) =>
    request<SearchResult>('/v1/diff', {
      method: 'POST',
      body: JSON.stringify({ pattern, limit }),
    }),
  definitions: (name: string) =>
    request<DefinitionsReport>(`/v1/def/${encodeURIComponent(name)}`),
  references: (name: string) =>
    request<ReferencesReport>(`/v1/refs/${encodeURIComponent(name)}`),
  files: () => request<FileListReport>('/v1/files'),
  file: (path: string) => request<FileContent>(`/v1/file?path=${encodeURIComponent(path)}`),
}

// Gateway-only endpoints deliberately bypass the repo prefix: the registry
// lives at the root, not under any repo — even after a repo is selected.
export const gateway = {
  repos: async (): Promise<RepoEntry[]> => {
    const response = await fetch('/v1/repos')
    if (!response.ok) throw new Error(`${response.status} ${response.statusText}`)
    return (await response.json()) as RepoEntry[]
  },
}

export type ServiceMode = 'gateway' | 'daemon'

// Probe the service shape once at startup: a registry answer means we're
// behind cce-gateway (repo selection applies); a failure means a standalone
// daemon where calls go straight through.
export async function discover(): Promise<ServiceMode> {
  try {
    const response = await fetch('/v1/repos')
    return response.ok ? 'gateway' : 'daemon'
  } catch {
    return 'daemon'
  }
}
