import { useCallback } from 'react'
import { useNavigate } from 'react-router'

/**
 * URL model — the canonical addresses for every screen and deep link.
 * Route state lives here (not in a store): refreshing, sharing a link, or
 * hitting back restores the same view.
 *
 *   /                     Home dashboard
 *   /query?q=<text>       Query — executed query in the URL (shareable)
 *   /browse/<path>?L=<n>|<a>-<b>  Browse — path splat + line / range anchor
 *   /symbols  /context  /index
 */
export type ScreenId =
  | 'dashboard'
  | 'query'
  | 'browse'
  | 'symbols'
  | 'commits'
  | 'branches'
  | 'changes'
  | 'map'
  | 'context'
  | 'contexts'
  | 'notebooks'
  | 'batch'
  | 'monitoring'
  | 'insights'
  | 'saved'
  | 'index'
  | 'about'

const SCREEN_PATHS: Record<Exclude<ScreenId, 'dashboard'>, string> = {
  query: '/query',
  browse: '/browse',
  symbols: '/symbols',
  commits: '/commits',
  branches: '/branches',
  changes: '/changes',
  map: '/map',
  context: '/context',
  contexts: '/contexts',
  notebooks: '/notebooks',
  batch: '/batch-changes',
  monitoring: '/code-monitoring',
  insights: '/insights',
  saved: '/saved',
  index: '/index',
  about: '/about',
}

const PATH_SCREENS = new Map<string, ScreenId>(
  Object.entries(SCREEN_PATHS).map(([id, p]) => [p.slice(1), id as ScreenId]),
)

export function pathForScreen(id: ScreenId): string {
  return id === 'dashboard' ? '/' : SCREEN_PATHS[id]
}

export function screenForPath(pathname: string): ScreenId {
  const seg = pathname.split('/')[1] ?? ''
  if (seg === 'browse') return 'browse'
  // Detail surfaces share their parent's sidebar highlight.
  if (seg === 'commit' || seg === 'compare') return 'commits'
  return PATH_SCREENS.get(seg) ?? 'dashboard'
}

/** `/browse/<path>` with each segment encoded — `?L=n` or `?L=a-b` anchors. */
export function fileUrl(path: string, line?: number | readonly [number, number]): string {
  const enc = path.split('/').map(encodeURIComponent).join('/')
  const anchor = Array.isArray(line) ? `${line[0]}-${line[1]}` : line
  return `/browse/${enc}${anchor != null ? `?L=${anchor}` : ''}`
}

export function queryUrl(q: string): string {
  return `/query?q=${encodeURIComponent(q)}`
}

/**
 * The retrieval loop's exit — any hit/symbol/evidence row calls this to
 * land on the canonical source file at its line. It's a real navigation,
 * so the location bar and history always reflect where you are.
 */
export function useOpenFile() {
  const navigate = useNavigate()
  return useCallback(
    (path: string, line?: number) => navigate(fileUrl(path, line)),
    [navigate],
  )
}
