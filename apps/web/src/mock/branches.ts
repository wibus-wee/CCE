/**
 * MOCK — fixture git refs for the Branches surface (Sourcegraph's repo
 * branch list). No `/v1/branches` endpoint exists and git refs are not
 * wired to the index yet, so every consumer must label the surface
 * `preview` — fixture refs are never presented as live repo truth.
 *
 * When ref listing lands, delete this file and switch the screen to the
 * API; the shape mirrors the intended response.
 */

export interface MockBranch {
  /** Ref shortname, e.g. 'feature/stylex-port'. */
  name: string
  /** The repository's default branch — pinned to the top of the list. */
  isDefault: boolean
  /** Abbreviated sha of the branch tip. */
  headSha: string
  /** Subject line of the tip commit. */
  headMessage: string
  /** Tip commit author handle. */
  author: string
  /** ISO timestamp of the tip commit. */
  at: string
  /** Commits ahead of the default branch. */
  aheadBy: number
  /** Commits behind the default branch. */
  behindBy: number
  /** Push-restricted ref — renders a lock glyph. */
  protected?: boolean
}

export interface MockTag {
  /** Tag name, e.g. 'v0.4.0-rc.2'. */
  name: string
  /** Abbreviated sha the tag points at — overlaps MOCK_COMMITS for coherence. */
  sha: string
  /** ISO timestamp of the tagged commit. */
  at: string
  /** Annotation subject — annotated tags carry one, lightweight tags don't. */
  message?: string
  /** The current release — renders the 'latest' badge. */
  release?: boolean
}

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()
const daysAgo = (d: number) => hoursAgo(d * 24)

/**
 * The working set a repo like this one would actually carry: `main` plus
 * feature/fix/chore/release refs at varied drift from the default branch.
 * `main`'s tip matches MOCK_TIMELINE's latest push for coherence.
 */
export const MOCK_BRANCHES: MockBranch[] = [
  { name: 'main', isDefault: true, protected: true, headSha: 'f0a317c0', headMessage: 'web: atomic components — Base UI + StyleX port', author: 'wibus', at: hoursAgo(5), aheadBy: 0, behindBy: 0 },
  { name: 'feature/stylex-port', isDefault: false, headSha: 'a4c19e27', headMessage: 'web: port Browse tree rows to atomic StyleX', author: 'wibus', at: hoursAgo(2), aheadBy: 14, behindBy: 3 },
  { name: 'fix/lease-guard', isDefault: false, headSha: '9d1b42ea', headMessage: 'daemon: fence index writes behind lease epoch', author: 'wibus', at: hoursAgo(9), aheadBy: 6, behindBy: 1 },
  { name: 'feature/scip-trait-edges', isDefault: false, headSha: 'e3f08b90', headMessage: 'scip: emit reference edges for trait impls', author: 'wibus', at: hoursAgo(16), aheadBy: 9, behindBy: 4 },
  { name: 'chore/gold-v3', isDefault: false, headSha: '77ac0d21', headMessage: 'research: re-label gold corpus against snapshot v3', author: 'wibus', at: hoursAgo(28), aheadBy: 2, behindBy: 11 },
  { name: 'fix/dense-offline', isDefault: false, headSha: 'b8e2f5a1', headMessage: 'engine: mark dense view unavailable with no model', author: 'wibus', at: hoursAgo(41), aheadBy: 3, behindBy: 7 },
  { name: 'feature/gateway-split', isDefault: false, headSha: 'c5e99258', headMessage: 'api: route surface handlers behind the gateway split', author: 'wibus', at: hoursAgo(66), aheadBy: 21, behindBy: 34 },
  { name: 'feature/taint-pass', isDefault: false, headSha: '6f3d8b2c', headMessage: 'dataflow: propagate taint through call edges', author: 'wibus', at: hoursAgo(350), aheadBy: 7, behindBy: 22 },
  { name: 'chore/drop-v0-endpoints', isDefault: false, headSha: '42b6e1f7', headMessage: 'api: remove deprecated /v0 handlers', author: 'wibus', at: hoursAgo(400), aheadBy: 5, behindBy: 48 },
  { name: 'release/0.4', isDefault: false, protected: true, headSha: '0f9d4c8b', headMessage: 'release: cut 0.4 — snapshot manifest freeze', author: 'wibus', at: hoursAgo(500), aheadBy: 1, behindBy: 57 },
]

/**
 * The release line a repo at this stage carries: rc tags on the active 0.4
 * cut plus the shipped 0.x line behind it. Shas deliberately overlap
 * MOCK_COMMITS so each tag lands on a commit the history fixture already
 * shows; `v0.2.1` is lightweight (no annotation) on purpose.
 */
export const MOCK_TAGS: MockTag[] = [
  { name: 'v0.4.0-rc.2', sha: '0f9d4c8b', at: daysAgo(2), message: 'release: 0.4 rc — snapshot manifest freeze', release: true },
  { name: 'v0.4.0-rc.1', sha: '94b6d1e5', at: daysAgo(6), message: 'release: 0.4 rc — history view cutline' },
  { name: 'v0.3.2', sha: '77ac0d21', at: daysAgo(21), message: 'release: 0.3.2 — packing budget split' },
  { name: 'v0.3.0', sha: '42b6e1f7', at: daysAgo(36), message: 'release: 0.3 — StyleX token layer' },
  { name: 'v0.2.1', sha: '3a8d5f09', at: daysAgo(63) },
  { name: 'v0.1.0', sha: 'f3a8c2d9', at: daysAgo(84), message: 'release: first benchmark cut' },
]
