/**
 * MOCK — provisional fixtures for surfaces that have no backend endpoint
 * yet. Organized one export per surface; every consumer must label the
 * surface `preview` so mocked data is never presented as source truth
 * (the same honesty rule as stale/degraded views).
 *
 * When the matching endpoint lands, delete the fixture and switch the
 * screen to the API — the fixtures mirror the intended response shapes.
 */

// --- /v1/symbols lives in mock/symbols.ts (dedicated module owns the
// surface's fixtures, incl. its cross-reference sites) -------------------

// --- /v1/activity (not implemented) -------------------------------------------

export interface MockTimelineEntry {
  sha: string
  message: string
  author: string
  at: string
  /** Views whose freshness this push invalidated. */
  invalidated: string[]
}

export const MOCK_TIMELINE: MockTimelineEntry[] = [
  { sha: 'f0a317c0', message: 'web: atomic components — Base UI + StyleX port', author: 'wibus', at: '2026-09-18T09:12:00Z', invalidated: ['lexical', 'symbols'] },
  { sha: 'c5e99258', message: 'engine: late-fusion rerank over dense+lexical', author: 'wibus', at: '2026-09-17T22:41:00Z', invalidated: ['dense', 'knowledge'] },
  { sha: '9d1b42ea', message: 'daemon: lease-guard concurrent index writes', author: 'wibus', at: '2026-09-17T18:03:00Z', invalidated: ['history'] },
  { sha: '77ac0d21', message: 'packing: deterministic budget split for context packs', author: 'wibus', at: '2026-09-16T15:29:00Z', invalidated: ['lexical', 'graph'] },
  { sha: 'e3f08b90', message: 'scip: emit reference edges for trait impls', author: 'wibus', at: '2026-09-15T11:52:00Z', invalidated: ['graph', 'dataflow'] },
  { sha: '42b6e1f7', message: 'web: redesign — StyleX token layer, dark parity', author: 'wibus', at: '2026-09-14T20:17:00Z', invalidated: [] },
]

// --- query telemetry seed (no endpoint; dashboard shows this labeled preview
// until real client-side measurements exist) ---------------------------------

export interface MockMetricPoint {
  query: string
  latencyMs: number
  hits: number
  route: string
  at: string
}

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()

/** ~9h of plausible latency: slow natural-language runs, fast exact hits. */
export const MOCK_QUERY_METRICS: MockMetricPoint[] = [
  { query: 'snapshot freshness', latencyMs: 9200, hits: 25, route: 'hybrid', at: hoursAgo(8.6) },
  { query: 'IndexLease', latencyMs: 240, hits: 3, route: 'exact_symbol', at: hoursAgo(7.9) },
  { query: 'path:crates/ lease', latencyMs: 6100, hits: 12, route: 'lexical', at: hoursAgo(7.2) },
  { query: 'how are views invalidated', latencyMs: 11400, hits: 18, route: 'dense_summary', at: hoursAgo(6.4) },
  { query: 'score_late_fusion', latencyMs: 310, hits: 1, route: 'exact_symbol', at: hoursAgo(5.8) },
  { query: 'type:commit rerank', latencyMs: 7800, hits: 22, route: 'history', at: hoursAgo(5.1) },
  { query: 'ViewManifest fields', latencyMs: 380, hits: 4, route: 'structural', at: hoursAgo(4.5) },
  { query: 'embedding backend config', latencyMs: 8600, hits: 15, route: 'lexical', at: hoursAgo(3.9) },
  { query: 'caplog conflict', latencyMs: 13100, hits: 9, route: 'hybrid', at: hoursAgo(3.2) },
  { query: 'pack_context', latencyMs: 290, hits: 2, route: 'exact_symbol', at: hoursAgo(2.6) },
  { query: 'taint analysis sinks', latencyMs: 10500, hits: 7, route: 'knowledge', at: hoursAgo(1.8) },
  { query: 'type:diff index.lock', latencyMs: 6900, hits: 11, route: 'diff', at: hoursAgo(1.1) },
  { query: 'snapshot freshness', latencyMs: 9800, hits: 25, route: 'hybrid', at: hoursAgo(0.4) },
]

export interface MockVolumeDay {
  day: string
  queries: number
}

const daysAgo = (d: number) => new Date(Date.now() - d * 86400_000).toISOString().slice(0, 10)

/** Two weeks of query volume — a GitHub-activity-style bar series. */
export const MOCK_QUERY_VOLUME: MockVolumeDay[] = [
  { day: daysAgo(13), queries: 4 },
  { day: daysAgo(12), queries: 9 },
  { day: daysAgo(11), queries: 6 },
  { day: daysAgo(10), queries: 12 },
  { day: daysAgo(9), queries: 3 },
  { day: daysAgo(8), queries: 1 },
  { day: daysAgo(7), queries: 7 },
  { day: daysAgo(6), queries: 15 },
  { day: daysAgo(5), queries: 11 },
  { day: daysAgo(4), queries: 18 },
  { day: daysAgo(3), queries: 8 },
  { day: daysAgo(2), queries: 13 },
  { day: daysAgo(1), queries: 10 },
  { day: daysAgo(0), queries: 6 },
]

export interface MockHitShare {
  route: string
  hits: number
}

/** Route mix across recent result sets — what actually answered queries. */
export const MOCK_HIT_MIX: MockHitShare[] = [
  { route: 'lexical', hits: 34 },
  { route: 'history', hits: 22 },
  { route: 'dense_summary', hits: 18 },
  { route: 'structural', hits: 14 },
  { route: 'knowledge', hits: 8 },
  { route: 'exact_symbol', hits: 4 },
]

// --- deeper SG surfaces -----------------------------------------------------
// Notebooks, Batch Changes, Code Monitoring, Insights, Commits, and Branches
// each own a dedicated fixture module beside this file (mock/notebooks.ts,
// mock/batchChanges.ts, …) — the screens import from those directly.
