/**
 * MOCK — fixture saved searches for the Saved searches surface (Sourcegraph's
 * saved-search library). There is no `/v1/saved-searches` endpoint yet; the
 * shape mirrors the intended API — delete this file when the endpoint lands
 * and switch the screen to the real fetch. Fixtures are never index truth:
 * the screen carries a `preview` badge and Run simply re-issues the query
 * live; stars and alerts are local state, never a write.
 */

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()
const daysAgo = (d: number) => new Date(Date.now() - d * 86400_000).toISOString()

/** Who the search is published under — the owner, everyone, or the team. */
export type MockSavedSearchNamespace = 'user' | 'global' | 'team'

export interface MockSavedSearch {
  id: string
  /** Sourcegraph-style query in the app's own query language — Run re-issues it verbatim. */
  query: string
  description: string
  /** Owner handle — 'wibus' for the viewer's entries, 'team' for shared ones. */
  owner: string
  namespace: MockSavedSearchNamespace
  /** Alert-on-new-results subscription — the screen toggles local state only. */
  alerts: boolean
  /** Whether the current viewer starred it — drives the star button. */
  starred: boolean
  createdAt: string
  updatedAt: string
  /** Last scheduled/manual run; absent means the search was never run. */
  lastRunAt?: string
}

/**
 * The working set a repo like this one would actually carry: the viewer's own
 * watches (diffs on `unsafe`, their commit trail) plus shared team and global
 * entries, mixing alert subscriptions, stars, and run recency.
 */
export const MOCK_SAVED_SEARCHES: MockSavedSearch[] = [
  {
    id: 'ss-unsafe-watch',
    query: 'type:diff unsafe',
    description: 'New diffs touching `unsafe` — the workspace forbids unsafe_code.',
    owner: 'wibus',
    namespace: 'user',
    alerts: true,
    starred: true,
    createdAt: daysAgo(34),
    updatedAt: hoursAgo(4),
    lastRunAt: hoursAgo(4),
  },
  {
    id: 'ss-my-commits',
    query: 'repo:cce$ type:commit author:wibus',
    description: 'Commit stream on cce authored by me — the recent-push trail.',
    owner: 'wibus',
    namespace: 'user',
    alerts: false,
    starred: false,
    createdAt: daysAgo(21),
    updatedAt: hoursAgo(9),
    lastRunAt: hoursAgo(7),
  },
  {
    id: 'ss-engine-symbols',
    query: 'path:crates/cce-engine select:symbol',
    description: 'Every indexed symbol in the engine crate — the API surface at a glance.',
    owner: 'team',
    namespace: 'team',
    alerts: false,
    starred: true,
    createdAt: daysAgo(28),
    updatedAt: hoursAgo(30),
    lastRunAt: daysAgo(1),
  },
  {
    id: 'ss-stylex-drift',
    query: 'lang:tsx stylex',
    description: 'StyleX call sites across the web app — drift check against the ui primitives.',
    owner: 'wibus',
    namespace: 'user',
    alerts: false,
    starred: true,
    createdAt: daysAgo(18),
    updatedAt: daysAgo(2),
    lastRunAt: hoursAgo(26),
  },
  {
    id: 'ss-release-line',
    query: 'repo:cce$ type:commit release',
    description: 'Commits that mention release — what shipped, not what is next.',
    owner: 'team',
    namespace: 'team',
    alerts: true,
    starred: false,
    createdAt: daysAgo(40),
    updatedAt: daysAgo(5),
    lastRunAt: daysAgo(3),
  },
  {
    id: 'ss-freshness',
    query: 'ViewManifest OR snapshot freshness',
    description: 'The freshness contract — manifest fields, staleness, honest fallbacks.',
    owner: 'wibus',
    namespace: 'global',
    alerts: false,
    starred: false,
    createdAt: daysAgo(15),
    updatedAt: daysAgo(6),
  },
  {
    id: 'ss-ui-primitives',
    query: 'type:diff path:apps/web/src/ui',
    description: 'Changes to shared ui primitives — the review surface for the port.',
    owner: 'team',
    namespace: 'team',
    alerts: true,
    starred: false,
    createdAt: daysAgo(25),
    updatedAt: daysAgo(8),
    lastRunAt: daysAgo(8),
  },
  {
    id: 'ss-dense-path',
    query: 'lang:rust embedder',
    description: 'Embedder implementations and the local dense path — ONNX profile included.',
    owner: 'wibus',
    namespace: 'user',
    alerts: false,
    starred: false,
    createdAt: daysAgo(12),
    updatedAt: daysAgo(9),
  },
  {
    id: 'ss-research-py',
    query: 'path:research/ lang:python',
    description: 'Python confined to research/ — datasets, benchmarks, trajectory analysis.',
    owner: 'team',
    namespace: 'global',
    alerts: false,
    starred: false,
    createdAt: daysAgo(30),
    updatedAt: daysAgo(13),
    lastRunAt: daysAgo(13),
  },
  {
    id: 'ss-lease',
    query: 'IndexLease',
    description: 'The index.lock guard — single-writer enforcement behind IndexBusy.',
    owner: 'wibus',
    namespace: 'user',
    alerts: false,
    starred: true,
    createdAt: daysAgo(9),
    updatedAt: daysAgo(14),
  },
]
