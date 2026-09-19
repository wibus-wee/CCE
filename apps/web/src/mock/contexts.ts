/**
 * MOCK — fixture search contexts for the Contexts surface (Sourcegraph's
 * `context:` selector page). There is no `/v1/contexts` endpoint and the
 * query engine does not resolve a `context:` token yet, so the screen
 * carries a `preview` badge and the scope token is only passed through
 * into the query text — fixtures are never live index truth.
 *
 * When context resolution lands, delete this file and resolve the token
 * against real repo/revision sets; the shape mirrors the intended API.
 */

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()
const daysAgo = (d: number) => new Date(Date.now() - d * 86400_000).toISOString()

/** The repo/revision pattern set a context expands to. */
export interface MockSearchContextSpec {
  /** Repo globs/addresses — '**' means everything in the index. */
  repositories: string[]
  /** Ref patterns inside those repos; absent = default branch only. */
  revisions?: string[]
  excludeForks?: boolean
  excludeArchived?: boolean
}

export interface MockSearchContext {
  /** The name typed into `context:<name>` — '@user' marks an auto context. */
  name: string
  description: string
  spec: MockSearchContextSpec
  /** 'auto' = system-computed (global, @user); public/private are user-defined. */
  visibility: 'auto' | 'public' | 'private'
  owner?: string
  /** Last spec edit; auto contexts are computed, so they carry none. */
  updatedAt?: string
}

/**
 * The working set a repo like this one would actually carry: the two auto
 * contexts Sourcegraph always offers (global + @user), then a handful of
 * user-defined scopes at mixed visibility and spec shape — repo globs,
 * revision pins, and exclusion flags.
 */
export const MOCK_CONTEXTS: MockSearchContext[] = [
  {
    name: 'global',
    description: 'Everything in the index — the scope a bare query runs against.',
    spec: { repositories: ['**'] },
    visibility: 'auto',
  },
  {
    name: '@wibus',
    description: 'Repositories you own or have commits in — the auto user context.',
    spec: { repositories: ['crates/*', 'apps/*', 'research/cce_research'] },
    visibility: 'auto',
    owner: 'wibus',
  },
  {
    name: 'cce-engine',
    description: 'The Rust core — engine, store, retrieval, packing.',
    spec: { repositories: ['crates/*'], revisions: ['main'] },
    visibility: 'public',
    owner: 'wibus',
    updatedAt: daysAgo(3),
  },
  {
    name: 'web-ui',
    description: 'The StyleX frontend — screens, ui primitives, fixtures.',
    spec: { repositories: ['apps/web'] },
    visibility: 'public',
    owner: 'wibus',
    updatedAt: hoursAgo(7),
  },
  {
    name: 'rust-only',
    description: 'Rust sources across the workspace — skips the web and research trees.',
    spec: {
      repositories: ['crates/*', 'apps/daemon', 'apps/cli'],
      revisions: ['main', 'release/*'],
      excludeForks: true,
      excludeArchived: true,
    },
    visibility: 'public',
    owner: 'wibus',
    updatedAt: daysAgo(9),
  },
  {
    name: 'experiments',
    description: 'Research notebooks, benchmarks and the gold corpus — work in progress.',
    spec: {
      repositories: ['research/**'],
      revisions: ['main', 'wip/*'],
      excludeArchived: true,
    },
    visibility: 'private',
    owner: 'wibus',
    updatedAt: hoursAgo(30),
  },
  {
    name: 'release-line',
    description: 'Everything pinned at release refs — what shipped, not what is next.',
    spec: { repositories: ['**'], revisions: ['release/*'], excludeArchived: true },
    visibility: 'public',
    owner: 'team',
    updatedAt: daysAgo(21),
  },
]
