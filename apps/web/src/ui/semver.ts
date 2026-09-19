/**
 * Port of `@antfu/design/utils/semver` — minimal semver compare and range
 * parsing for `DisplayVersion`.
 */

export interface ParsedSemver {
  valid: boolean
  raw: string
  highest?: string
  lowest?: string
  parts?: string[]
  bare?: string[]
}

const SEMVER_RE = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([\w.-]+))?/

function parseVersion(version: string): [number, number, number, string] | null {
  const m = version.trim().match(SEMVER_RE)
  if (!m) return null
  return [Number(m[1] ?? 0), Number(m[2] ?? 0), Number(m[3] ?? 0), m[4] ?? '']
}

export function compareSemver(a: string, b: string): number {
  if (a === b) return 0
  const pa = parseVersion(a)
  const pb = parseVersion(b)
  if (!pa || !pb) return 0
  const core = [[pa[0], pb[0]], [pa[1], pb[1]], [pa[2], pb[2]]] as const
  for (const [x, y] of core) {
    if (x !== y) return x < y ? -1 : 1
  }
  // Equal core: a version without prerelease is greater than one with.
  if (pa[3] === pb[3]) return 0
  if (!pa[3]) return 1
  if (!pb[3]) return -1
  return pa[3] < pb[3] ? -1 : 1
}

const rangeCache = new Map<string, ParsedSemver>()

export function parseSemverRange(range: string): ParsedSemver {
  const cached = rangeCache.get(range)
  if (cached) return cached
  const result: ParsedSemver = { valid: false, raw: range }
  rangeCache.set(range, result)
  const parts = range
    .split(/\|\|/g)
    .map(i => i.replace(/\s+/g, ''))
    .filter(Boolean)
  if (!parts.length) return result
  const bare = parts
    .map(i => i.replace(/^[\^~>=<]+/, '').replace(/\.[*x]$/i, '').trim())
    .map((i) => {
      const seg = i.split('.')
      if (seg.length === 1) return `${i}.0.0`
      if (seg.length === 2) return `${i}.0`
      return i
    })
    .filter(i => SEMVER_RE.test(i))
  if (!bare.length) return result
  const sorted = [...bare].sort(compareSemver)
  result.valid = true
  result.parts = parts
  result.bare = bare
  result.lowest = sorted[0]
  result.highest = sorted[sorted.length - 1]
  return result
}
