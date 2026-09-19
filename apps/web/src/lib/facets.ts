import type { SearchHit } from '../api'

/**
 * Client-side facets over the current hit set — Sourcegraph-style filter
 * rail, but counts are honest: they describe only the returned page of
 * hits, never a server-side aggregate that doesn't exist.
 */

export interface FacetOption {
  value: string
  label: string
  count: number
}

export interface FacetGroup {
  id: string
  label: string
  options: FacetOption[]
}

function countBy(hits: SearchHit[], pick: (hit: SearchHit) => string | undefined): FacetOption[] {
  const counts = new Map<string, number>()
  for (const hit of hits) {
    const key = pick(hit)
    if (key) counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  return [...counts.entries()]
    .map(([value, count]) => ({ value, label: value, count }))
    .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value))
}

function topDir(path: string): string {
  const i = path.indexOf('/')
  return i === -1 ? path : path.slice(0, i)
}

function extension(path: string): string | undefined {
  const name = path.split('/').pop() ?? ''
  const i = name.lastIndexOf('.')
  return i > 0 ? name.slice(i + 1) : undefined
}

export function computeFacets(hits: SearchHit[]): FacetGroup[] {
  const groups: FacetGroup[] = [
    { id: 'route', label: 'Route', options: countBy(hits, (h) => h.route) },
    {
      id: 'representation',
      label: 'Representation',
      options: countBy(hits, (h) => h.representation),
    },
    {
      id: 'dir',
      label: 'Path root',
      options: countBy(hits, (h) => h.address && topDir(h.address.path)),
    },
    {
      id: 'ext',
      label: 'Extension',
      options: countBy(hits, (h) => h.address && extension(h.address.path)),
    },
  ]
  return groups.filter((g) => g.options.length > 0)
}

export type ActiveFilters = Record<string, Set<string>>

export function emptyFilters(): ActiveFilters {
  return {}
}

export function toggleFilter(filters: ActiveFilters, groupId: string, value: string): ActiveFilters {
  const next: ActiveFilters = { ...filters }
  const set = new Set(next[groupId] ?? [])
  if (set.has(value)) set.delete(value)
  else set.add(value)
  if (set.size === 0) delete next[groupId]
  else next[groupId] = set
  return next
}

export function filterHits(hits: SearchHit[], filters: ActiveFilters): SearchHit[] {
  const pick: Record<string, (h: SearchHit) => string | undefined> = {
    route: (h) => h.route,
    representation: (h) => h.representation,
    dir: (h) => h.address && topDir(h.address.path),
    ext: (h) => h.address && extension(h.address.path),
  }
  return hits.filter((hit) =>
    Object.entries(filters).every(([groupId, values]) => {
      const value = pick[groupId]?.(hit)
      return value !== undefined && values.has(value)
    }),
  )
}
