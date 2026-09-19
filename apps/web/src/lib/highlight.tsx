import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { Fragment } from 'react'
import { jsx, jsxs } from 'react/jsx-runtime'
import {
  bundledLanguages,
  bundledThemes,
  createBundledHighlighter,
  createJavaScriptRegexEngine,
  createSingletonShorthands,
  isPlainLang,
  type BundledLanguage,
  type DecorationItem,
  type DynamicImportLanguageRegistration,
} from 'shiki'
import { toJsxRuntime } from 'hast-util-to-jsx-runtime'

/**
 * Shiki → React integration, no `dangerouslySetInnerHTML` anywhere:
 *
 * - `createSingletonShorthands` + `createBundledHighlighter` — one cached
 *   highlighter; every language is a lazy dynamic import fetched on first use.
 * - `createJavaScriptRegexEngine` — pure-JS regex engine, no WASM download.
 * - `themes` + `defaultColor: 'light-dark()'` — each token emits
 *   `color: light-dark(#light, #dark)` inline; `color-scheme` is already set
 *   on `<html>` by `useDark`, so one highlight pass serves both themes.
 * - `codeToHast` → `toJsxRuntime` per `span.line` — real React elements that
 *   drop into the existing gutter layout.
 * - Engine `<mark>` ranges become `decorations` (0-indexed offsets), so hit
 *   highlights survive tokenization instead of competing with it.
 *
 * NO first-paint flash: `codeToHast` is synchronous once a grammar is loaded.
 * At module scope we warm the singleton (themes + the languages this repo
 * speaks), so by the time a file or snippet renders the sync path below
 * highlights inside `useMemo` — the very first frame is colored. Only a
 * never-seen language takes the async fallback (its grammar must download).
 */

const { codeToHast, getSingletonHighlighter } = createSingletonShorthands(
  createBundledHighlighter({
    langs: bundledLanguages,
    themes: {
      'vitesse-light': bundledThemes['vitesse-light'],
      'vitesse-dark': bundledThemes['vitesse-dark'],
    },
    engine: () => createJavaScriptRegexEngine({ forgiving: true }),
  }),
)

// Grammars this repo speaks — preloaded at module scope so file/snippet
// renders almost always hit the synchronous path below.
const PRELOAD_LANGS = [
  'rust',
  'typescript',
  'tsx',
  'javascript',
  'jsx',
  'json',
  'jsonc',
  'toml',
  'yaml',
  'markdown',
  'python',
  'bash',
  'css',
  'html',
  'diff',
  'go',
] as const

let highlighter: Awaited<ReturnType<typeof getSingletonHighlighter>> | null = null
// The bundled `themes` map registers factories only — the instance's
// `codeToHast` throws until `loadTheme` actually resolves them.
let themesReady = false
const highlighterReady = getSingletonHighlighter()
highlighterReady
  .then(async (h) => {
    await Promise.all([
      h.loadTheme(bundledThemes['vitesse-light']),
      h.loadTheme(bundledThemes['vitesse-dark']),
    ])
    themesReady = true
    highlighter = h
    for (const lang of PRELOAD_LANGS) {
      const factory = bundledLanguages[lang as BundledLanguage] as
        | DynamicImportLanguageRegistration
        | undefined
      if (factory) void h.loadLanguage(factory).catch(() => {})
    }
  })
  .catch(() => {})

function isLangLoaded(lang: string): boolean {
  return highlighter?.getLoadedLanguages().includes(lang) ?? false
}

type HastRoot = Awaited<ReturnType<typeof codeToHast>>
type HastNode = HastRoot['children'][number]
type HastElement = Extract<HastNode, { type: 'element' }>

function isElement(node: HastNode): node is HastElement {
  return node.type === 'element'
}

/** Normalize an API language name or file extension to a bundled lang id. */
const LANG_ALIASES: Record<string, string> = {
  sh: 'bash',
  shell: 'bash',
  zsh: 'bash',
  yml: 'yaml',
  md: 'markdown',
  'c++': 'cpp',
  'c#': 'csharp',
  cs: 'csharp',
  py: 'python',
  rb: 'ruby',
  js: 'javascript',
  ts: 'typescript',
  gql: 'graphql',
  mk: 'makefile',
  dockerfile: 'dockerfile',
  plaintext: 'text',
  txt: 'text',
}

export function langFor(name?: string): string | undefined {
  if (!name) return undefined
  const mapped = LANG_ALIASES[name.toLowerCase().replace(/^\./, '')] ?? name.toLowerCase()
  if (isPlainLang(mapped)) return 'text'
  return mapped in bundledLanguages ? mapped : undefined
}

export function langFromPath(path?: string): string | undefined {
  if (!path) return undefined
  const dot = path.lastIndexOf('.')
  return dot >= 0 ? langFor(path.slice(dot + 1)) : undefined
}

/** Strip engine `<mark>` tags, recording content offsets as decorations. */
export function extractMarks(raw: string): { code: string; decorations: DecorationItem[] } {
  const decorations: DecorationItem[] = []
  // Pair matches become decorations; lone tag remnants are dropped in the
  // same pass so recorded offsets never drift.
  const re = /<mark>([\s\S]*?)<\/mark>|<\/?mark>/g
  let code = ''
  let last = 0
  for (const match of raw.matchAll(re)) {
    code += raw.slice(last, match.index)
    if (match[1] !== undefined) {
      const start = code.length
      code += match[1]
      decorations.push({ start, end: code.length, properties: { class: 'marked' } })
    }
    last = match.index + match[0].length
  }
  code += raw.slice(last)
  return { code, decorations }
}

/** pre > code > span.line extraction shared by both render paths. */
function linesFromHast(hast: HastRoot): ReactNode[] {
  const pre = hast.children.find(
    (c): c is HastElement => isElement(c) && c.tagName === 'pre',
  )
  const codeEl = pre?.children.find(
    (c): c is HastElement => isElement(c) && c.tagName === 'code',
  )
  const lineEls = (codeEl?.children ?? []).filter(isElement)
  return lineEls.map((el, i) => (
    <Fragment key={i}>{toJsxRuntime(el, { Fragment, jsx, jsxs }) as ReactNode}</Fragment>
  ))
}

const HAST_OPTIONS = {
  themes: { light: 'vitesse-light', dark: 'vitesse-dark' },
  defaultColor: 'light-dark()',
} as const

/** Synchronous highlight — requires the grammar already loaded. */
function syncHighlightLines(
  code: string,
  lang: string,
  decorations?: DecorationItem[],
): ReactNode[] | null {
  if (!highlighter) return null
  try {
    const hast = highlighter.codeToHast(code, {
      lang: lang as BundledLanguage,
      ...HAST_OPTIONS,
      decorations,
    })
    return linesFromHast(hast)
  } catch (e) {
    console.warn('[shiki] sync highlight failed:', e)
    return null
  }
}

async function asyncHighlightLines(
  code: string,
  lang: string,
  decorations?: DecorationItem[],
): Promise<ReactNode[]> {
  const hast = await codeToHast(code, {
    lang: lang as BundledLanguage,
    ...HAST_OPTIONS,
    decorations,
  })
  return linesFromHast(hast)
}

// --- node cache -------------------------------------------------------------
// Highlight output keyed by (lang, decorations, code) — the code string is a
// reference, not a copy. LRU-bounded. This is what lets a component mount
// already colored: fetch handlers `prewarm` before setState, and every
// successful highlight also writes here so remounts are instant.
const NODE_CACHE_MAX = 200
const nodeCache = new Map<string, ReactNode[]>()

function decoKeyOf(decorations?: DecorationItem[]): string {
  return decorations?.length ? decorations.map((d) => `${d.start}-${d.end}`).join(',') : ''
}

function cacheKeyOf(code: string, lang: string, decoKey: string): string {
  return `${lang}${decoKey}${code}`
}

function cacheGet(key: string): ReactNode[] | undefined {
  const value = nodeCache.get(key)
  if (value) {
    nodeCache.delete(key)
    nodeCache.set(key, value)
  }
  return value
}

function cacheSet(key: string, value: ReactNode[]): void {
  nodeCache.set(key, value)
  if (nodeCache.size > NODE_CACHE_MAX) {
    nodeCache.delete(nodeCache.keys().next().value!)
  }
}

/**
 * Pre-highlight before the component mounts — call from fetch handlers
 * (between `api.file()`/search resolving and setState). Cold grammars load
 * here, so the pane appears colored the moment it appears; warm calls cost
 * ~ms. Best-effort: failures leave the normal fallback path intact.
 */
export async function prewarmHighlight(
  code: string,
  lang?: string,
  decorations?: DecorationItem[],
): Promise<void> {
  const resolved = langFor(lang)
  if (!resolved || resolved === 'text') return
  const key = cacheKeyOf(code, resolved, decoKeyOf(decorations))
  if (nodeCache.has(key)) return
  try {
    const nodes = await asyncHighlightLines(code, resolved, decorations)
    if (nodes) cacheSet(key, nodes)
  } catch {
    // leave uncached — the hook's fallback renders plain text
  }
}

/** Same transform SnippetView applies — shared so prewarm and render agree. */
export function buildSnippetInput(snippet: string, maxLines: number) {
  const lines = snippet.split('\n').slice(0, maxLines)
  return extractMarks(lines.join('\n'))
}

/** Pre-highlight a hit snippet (list view uses maxLines=8). */
export function prewarmSnippet(
  snippet: string,
  lang: string | undefined,
  maxLines = 8,
): Promise<void> {
  const { code, decorations } = buildSnippetInput(snippet, maxLines)
  return prewarmHighlight(code, lang, decorations)
}

/**
 * Highlight `code` into per-line React nodes (one per `\n`-line).
 *
 * Resolution order: node cache (prewarmed / previously highlighted) →
 * synchronous `codeToHast` inside `useMemo` when the grammar is warm → async
 * shorthand for cold grammars. Returns null for plain text or unresolvable
 * grammars — callers render their existing unhighlighted fallback.
 */
export function useHighlightLines(
  code: string,
  lang?: string,
  decorations?: DecorationItem[],
): ReactNode[] | null {
  const resolved = langFor(lang)
  const decoKey = useMemo(() => decoKeyOf(decorations), [decorations])
  const cacheKey = resolved ? cacheKeyOf(code, resolved, decoKey) : ''
  const cached = cacheKey ? cacheGet(cacheKey) : undefined
  const syncReady =
    resolved != null && resolved !== 'text' && themesReady && isLangLoaded(resolved)

  const syncNodes = useMemo(() => {
    if (cached) return cached
    if (!syncReady || !resolved) return null
    const nodes = syncHighlightLines(code, resolved, decorations)
    if (nodes) cacheSet(cacheKey, nodes)
    return nodes
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, resolved, decoKey, syncReady, cached, cacheKey])

  const [asyncNodes, setAsyncNodes] = useState<ReactNode[] | null>(null)
  useEffect(() => {
    if (!resolved || resolved === 'text' || cached) return
    // Sync covered it — no async work needed. If sync threw (cold theme,
    // grammar mid-load), the async path rescues it instead of going blank.
    if (syncReady && syncNodes != null) return
    let live = true
    asyncHighlightLines(code, resolved, decorations)
      .then((lines) => {
        if (live && lines) {
          cacheSet(cacheKey, lines)
          setAsyncNodes(lines)
        }
      })
      .catch((e) => {
        console.warn('[shiki] async highlight failed:', e)
        if (live) setAsyncNodes(null)
      })
    return () => {
      live = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, resolved, decoKey, syncReady, syncNodes, cached, cacheKey])

  return cached ?? syncNodes ?? asyncNodes
}
