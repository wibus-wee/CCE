/**
 * MOCK — fixture per-line authorship for the Browse file view's blame
 * gutter (Sourcegraph/GitHub's blame column). No `/v1/blame` endpoint
 * exists and `git blame` is not wired to the index yet, so the gutter is
 * opt-in and carries a `preview` note — fixture annotations are never
 * presented as live git truth.
 *
 * Deterministic by path: the generator seeds itself from the file path,
 * so the same file always blames to the same blocks. Authors, shas and
 * timestamps come straight out of MOCK_COMMITS (mock/commits.ts) so the
 * gutter tells the same story as the Commits surface. When real blame
 * lands, delete this file and switch the screen to the API.
 */

import { MOCK_COMMITS, type MockCommit } from './commits'

/** One line's blame annotation — the fixture commit that "last touched" it. */
export interface BlameLine {
  sha: string
  author: string
  /** ISO timestamp of the attributed commit — drives the age tint. */
  at: string
}

/** FNV-1a — small stable hash so a path always seeds the same gutter. */
function hashPath(path: string): number {
  let h = 2166136261
  for (let i = 0; i < path.length; i++) {
    h ^= path.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return h >>> 0
}

/** mulberry32 — a tiny seeded PRNG, plenty for fixture layout. */
function seeded(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/**
 * Blame for `lineCount` lines, shaped like real `git blame` output: a
 * seeded pick of 2–4 distinct fixture authors owns contiguous 4–15-line
 * blocks rather than per-line noise, and the same author never owns two
 * adjacent blocks. Deterministic for a given path — reloads and
 * re-renders never reshuffle authorship.
 */
export function blameFor(path: string, lineCount: number): BlameLine[] {
  const rand = seeded(hashPath(path))
  const picks: MockCommit[] = []
  const seen = new Set<string>()
  const want = Math.min(2 + Math.floor(rand() * 3), MOCK_COMMITS.length)
  for (let guard = 0; guard < 64 && picks.length < want; guard++) {
    const c = MOCK_COMMITS[Math.floor(rand() * MOCK_COMMITS.length)]
    if (!c || seen.has(c.author)) continue
    seen.add(c.author)
    picks.push(c)
  }
  const lines: BlameLine[] = new Array<BlameLine>(lineCount)
  let i = 0
  let prev = -1
  while (i < lineCount) {
    let pick = Math.floor(rand() * picks.length)
    if (picks.length > 1 && pick === prev) pick = (pick + 1) % picks.length
    const c = picks[pick]
    const end = Math.min(lineCount, i + 4 + Math.floor(rand() * 12))
    for (; i < end; i++) {
      lines[i] = { sha: c?.sha ?? '', author: c?.author ?? '', at: c?.at ?? '' }
    }
    prev = pick
  }
  return lines
}
