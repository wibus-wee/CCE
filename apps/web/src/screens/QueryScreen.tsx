import * as stylex from '@stylexjs/stylex'
import { useHotkeys } from '@tanstack/react-hotkeys'
import { FormEvent, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  api,
  DefinitionsReport,
  describeError,
  GrepReport,
  ReferencesReport,
  SearchHit,
  SearchResult,
  SourceAddress,
} from '../api'
import { useLocation, useNavigate, useSearchParams } from 'react-router'
import { ContextSelector } from '../components/ContextSelector'
import { FacetRail } from '../components/FacetRail'
import { HistoryPopover } from '../components/HistoryPopover'
import {
  computeFacets,
  emptyFilters,
  filterHits,
  toggleFilter,
  type ActiveFilters,
} from '../lib/facets'
import { pushRecent, toggleSaved, useSearchHistory } from '../lib/searchHistory'
import { langFromPath, prewarmSnippet } from '../lib/highlight'
import { fileUrl, queryUrl } from '../lib/navigation'
import { queryTokens } from '../lib/querySyntax'
import { useAppStore } from '../lib/store'
import { SnippetView } from '../lib/snippet'
import { recordQuery } from '../lib/telemetry'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { ActionToggleGroup } from '../ui/ActionToggleGroup'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayProportionBar } from '../ui/DisplayCharts'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { DisplayKeyValue } from '../ui/DisplayKeyValue'
import { DisplayDuration } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FeedbackTip } from '../ui/FeedbackTip'
import { FormNumberInput } from '../ui/FormNumberInput'
import { FormSearchField } from '../ui/FormSearchField'
import {
  IconArrowUpRight,
  IconCaretDown,
  IconDownload,
  IconEllipsis,
  IconSearch,
  IconStar,
} from '../ui/icons'
import { formatAddress } from '../ui/format'
import { OverlayDrawer } from '../ui/OverlayDialogs'
import { iconButtons } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

type SearchType = 'file' | 'diff' | 'commit'
/** engine — the index-backed retrieval stack; grep — regex over live worktree bytes. */
type QueryMode = 'engine' | 'grep'

/**
 * Query — the retrieval workbench, restructured after Sourcegraph's
 * results page: facet rail on the left (client-side, honest counts),
 * result cards with real <mark> highlighting + gutter line numbers, a
 * detail drawer per hit, and local recent/saved searches.
 *
 * Execution is URL-driven: `?q=` is the single trigger for a run — form
 * submits, palette commands, history picks, and shared links all navigate
 * to it. The last result is held in the app store so back/forward and
 * route returns restore it instead of re-running the query.
 */
export function QueryScreen({
  onOpenFile,
}: {
  /** Jump a hit to its canonical source file — the retrieval loop's exit. */
  onOpenFile: (path: string, line?: number) => void
}) {
  const [searchParams] = useSearchParams()
  const location = useLocation()
  const navigate = useNavigate()
  const query = useAppStore((s) => s.query)
  const setQuery = useAppStore((s) => s.setQuery)
  const setManifest = useAppStore((s) => s.setManifest)
  const result = useAppStore((s) => s.result)
  const resultFor = useAppStore((s) => s.resultFor)
  const setResult = useAppStore((s) => s.setResult)

  const [searchType, setSearchType] = useState<SearchType>('file')
  const [limit, setLimit] = useState(25)
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const { recents, saved } = useSearchHistory()

  // Worktree grep — a second engine behind the same box. Local state only:
  // the app store's `result` is SearchResult-shaped, and grep hits are not
  // index results, so nothing below touches result/resultFor/manifest.
  const [mode, setMode] = useState<QueryMode>('engine')
  const [grepBusy, setGrepBusy] = useState(false)
  const [grepError, setGrepError] = useState<string>()
  const [grepReport, setGrepReport] = useState<GrepReport>()
  const [grepIgnoreCase, setGrepIgnoreCase] = useState(false)

  function switchMode(next: QueryMode) {
    if (next === mode) return
    setMode(next)
    setHistoryOpen(false)
    // Stale errors from the other engine must not bleed across the switch.
    setError(undefined)
    setGrepError(undefined)
  }

  // The URL is the single execution trigger: form submits, palette runs,
  // history picks, dashboard chips, and shared links all land here.
  const executedQ = searchParams.get('q') ?? ''

  async function runSearch(text: string) {
    const q = text.trim()
    if (!q) return
    setBusy(true)
    setError(undefined)
    setHistoryOpen(false)
    pushRecent(q)
    try {
      const safeLimit = Number.isFinite(limit) ? Math.min(200, Math.max(1, Math.trunc(limit))) : 20
      // A `type:` token typed by hand wins over the selector.
      const effective =
        searchType === 'file' || /\btype:\S+/i.test(q) ? q : `type:${searchType} ${q}`
      const r = await api.search({ query: effective, limit: safeLimit })
      // Highlight all snippets before the list mounts — first paint colored.
      await Promise.all(
        r.hits.map((hit) =>
          hit.snippet
            ? prewarmSnippet(hit.snippet, langFromPath(hit.address?.path), 8)
            : undefined,
        ),
      )
      setResult(r, q)
      setError(undefined)
      setManifest(r.manifest)
      recordQuery({
        query: q,
        latencyMs: r.latencyMs,
        hits: r.hits.length,
        snapshotId: r.request.snapshotId,
        at: new Date().toISOString(),
      })
    } catch (value) {
      setResult(undefined)
      setError(describeError(value))
    } finally {
      setBusy(false)
    }
  }

  // Live-bytes regex — no URL trigger, no history, no telemetry: the report
  // is not a SearchResult and the box text is a pattern, not an engine query.
  async function runGrep() {
    const pattern = query
    if (!pattern.trim()) return
    setGrepBusy(true)
    setGrepError(undefined)
    setHistoryOpen(false)
    try {
      const r = await api.grep({
        pattern,
        ignoreCase: grepIgnoreCase || undefined,
        limit: 200,
      })
      setGrepReport(r)
    } catch (value) {
      setGrepReport(undefined)
      setGrepError(`grep failed — ${describeError(value)}`)
    } finally {
      setGrepBusy(false)
    }
  }

  function search(event: FormEvent) {
    event.preventDefault()
    if (mode === 'grep') void runGrep()
    else navigate(queryUrl(query))
  }

  // Restore the cached result on first mount for this URL (back/forward
  // and route returns must not re-run a dense query); every later
  // navigation — including re-submitting the same q — runs for real.
  // `fired` dedupes StrictMode's double effect invocation per entry.
  const restored = useRef(false)
  const fired = useRef('')
  useEffect(() => {
    if (!executedQ) return
    // A `?q=` landing is always engine semantics (palette, history, links) —
    // leave grep mode so the incoming query is what the box shows and runs.
    setMode('engine')
    setQuery(executedQ)
    const key = `${location.key}:${executedQ}`
    if (fired.current === key) return
    fired.current = key
    if (!restored.current && executedQ === resultFor && result) {
      restored.current = true
      return
    }
    void runSearch(executedQ)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [executedQ, location.key])

  const starred = saved.includes(query.trim())

  return (
    <div {...stylex.props(styles.root)}>
      <form onSubmit={search} {...stylex.props(styles.bar)}>
        <ActionToggleGroup
          aria-label="Search mode"
          value={mode}
          onValueChange={(v) => {
            // ToggleGroup reports '' when a pressed segment deselects — the
            // box always has exactly one engine behind it.
            if (v === 'engine' || v === 'grep') switchMode(v)
          }}
          options={[
            { value: 'engine', label: 'engine' },
            { value: 'grep', label: 'grep' },
          ]}
        />
        <div {...stylex.props(styles.field)}>
          <QueryInput
            value={query}
            onChange={setQuery}
            onFocus={() => setHistoryOpen(true)}
            onClear={() => setQuery('')}
            leading={
              // context: scoping is engine grammar — meaningless to a regex.
              mode === 'engine' ? (
                <ContextSelector value={query} onChange={setQuery} />
              ) : undefined
            }
            trailing={
              mode === 'grep' ? (
                <button
                  type="button"
                  aria-label="Case-insensitive matching"
                  aria-pressed={grepIgnoreCase}
                  title="Match case-insensitively (ignoreCase)"
                  onClick={() => setGrepIgnoreCase((v) => !v)}
                  {...stylex.props(
                    iconButtons.mini,
                    grepIgnoreCase && iconButtons.active,
                    styles.caseToggle,
                  )}
                >
                  i
                </button>
              ) : undefined
            }
            placeholder={
              mode === 'grep' ? 'worktree regex — e.g. unsafe|TODO' : undefined
            }
            ariaLabel={mode === 'grep' ? 'Worktree regex' : undefined}
            highlightSyntax={mode === 'engine'}
          />
          <HistoryPopover
            recents={recents}
            saved={saved}
            open={historyOpen && mode === 'engine'}
            onPick={(q) => navigate(queryUrl(q))}
            onClose={() => setHistoryOpen(false)}
          />
        </div>
        {mode === 'engine' && (
          <>
            <ActionToggleGroup
              aria-label="Result type"
              value={searchType}
              onValueChange={(v) => setSearchType(v as SearchType)}
              options={[
                { value: 'file', label: 'file' },
                { value: 'diff', label: 'diff' },
                { value: 'commit', label: 'commit' },
              ]}
            />
            <div {...stylex.props(styles.limit)}>
              <FormNumberInput
                min={1}
                max={200}
                value={limit}
                onValueChange={(v) => setLimit(v ?? 1)}
                aria-label="Limit"
                title="Result limit"
              />
            </div>
          </>
        )}
        <ActionButton
          variant="primary"
          disabled={busy || grepBusy || !query.trim()}
          loading={busy || grepBusy}
          type="submit"
        >
          Search
        </ActionButton>
        {mode === 'engine' && (
          <ActionIconButton
            tooltip={starred ? 'Remove from saved searches' : 'Save this search'}
            onClick={() => toggleSaved(query)}
            disabled={!query.trim()}
            label="Save search"
            icon={<IconStar size={14} filled={starred} />}
          />
        )}
      </form>

      {mode === 'engine' && error && <FeedbackTip variant="error">{error}</FeedbackTip>}
      {mode === 'grep' && grepError && (
        <FeedbackTip variant="error">{grepError}</FeedbackTip>
      )}
      {mode === 'engine' && result && (
        <Results
          key={result.request.snapshotId + result.request.query}
          result={result}
          onOpenFile={onOpenFile}
        />
      )}
      {mode === 'grep' && grepReport && (
        <GrepResults report={grepReport} onOpenFile={onOpenFile} />
      )}
      {mode === 'engine' && !result && !error && (
        <FeedbackEmptyState
          icon={<IconSearch size={20} />}
          title="Query the committed snapshot"
          description="Lexical, dense, structural and history routes answer over a pinned snapshot. lang:, path: and type: tokens become structured filters."
        />
      )}
      {mode === 'grep' && !grepReport && !grepError && (
        <FeedbackEmptyState
          icon={<IconSearch size={20} />}
          title="Grep the live worktree"
          description="A regular expression over current file bytes — uncommitted edits and unindexed files included. Nothing is read from the index."
        />
      )}
    </div>
  )
}

/** ripgrep-style flat list — `path:line:col` + the matched line, live bytes. */
function GrepResults({
  report,
  onOpenFile,
}: {
  report: GrepReport
  onOpenFile: (path: string, line?: number) => void
}) {
  return (
    <section aria-label="Worktree grep results" {...stylex.props(styles.results)}>
      <div {...stylex.props(styles.meta)}>
        <DisplayBadge severity="low">live</DisplayBadge>
        <strong {...stylex.props(styles.hitCount)}>
          {report.matches.length} match{report.matches.length === 1 ? '' : 'es'}
        </strong>
        <span {...stylex.props(styles.grepStats)}>
          {report.filesScanned} files scanned · {report.skippedBinary} binary skipped
        </span>
        {report.truncated && (
          <DisplayBadge severity="medium" title="Hit the 200-match cap — narrow the pattern">
            truncated
          </DisplayBadge>
        )}
        <span {...stylex.props(styles.metaSep)} />
        <span {...stylex.props(styles.grepNote)} title={`freshness: ${report.freshness}`}>
          worktree bytes — not the index
        </span>
      </div>
      {report.matches.length === 0 ? (
        <p {...stylex.props(styles.grepEmpty)}>no matches</p>
      ) : (
        <div {...stylex.props(styles.hits)}>
          {report.matches.map((hit, i) => (
            <button
              key={`${hit.path}:${hit.line}:${hit.column}:${i}`}
              type="button"
              {...stylex.props(styles.grepRow)}
              onClick={() => onOpenFile(hit.path, hit.line)}
              title={`open ${hit.path}:${hit.line}:${hit.column}`}
            >
              <span {...stylex.props(styles.grepLoc)}>
                {hit.path}:{hit.line}:{hit.column}
              </span>
              <code {...stylex.props(styles.grepText)}>{hit.text}</code>
            </button>
          ))}
        </div>
      )}
    </section>
  )
}

function Results({
  result,
  onOpenFile,
}: {
  result: SearchResult
  onOpenFile: (path: string, line?: number) => void
}) {
  const [filters, setFilters] = useState<ActiveFilters>(emptyFilters())
  const [detail, setDetail] = useState<SearchHit>()
  const [sel, setSel] = useState(-1)
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const groups = useMemo(() => computeFacets(result.hits), [result.hits])
  const hits = useMemo(() => filterHits(result.hits, filters), [result.hits, filters])
  const activeCount = Object.values(filters).reduce((n, s) => n + s.size, 0)

  const openHit = useCallback(
    (hit: SearchHit) => {
      if (hit.address) onOpenFile(hit.address.path, hit.address.startLine)
    },
    [onOpenFile],
  )

  function toggleExpand(hit: SearchHit) {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(hit.documentId)) next.delete(hit.documentId)
      else next.add(hit.documentId)
      return next
    })
  }

  function move(delta: number) {
    setSel((i) => {
      const next = Math.min(hits.length - 1, Math.max(0, i + delta))
      document.getElementById(`hit-${hits[next]?.documentId}`)?.scrollIntoView({ block: 'nearest' })
      return next
    })
  }

  // j/k walk, h/l collapse/expand, Enter toggles, o opens source, ⌘↵ opens
  // in a new tab — Sourcegraph's result-navigation map. Bare keys auto-ignore
  // text inputs; the palette gate replaces the old role=dialog bail. ⌘↵
  // keeps firing while typing (it ran before the inField check).
  const paletteOpen = useAppStore((s) => s.paletteOpen)
  const selHit = () => (sel >= 0 ? hits[sel] : undefined)
  useHotkeys(
    [
      {
        hotkey: 'Mod+Enter',
        callback: () => {
          const hit = selHit()
          if (hit?.address) {
            window.open(fileUrl(hit.address.path, hit.address.startLine), '_blank', 'noopener')
          }
        },
        options: { enabled: true },
      },
      { hotkey: 'J', callback: () => move(1) },
      { hotkey: 'ArrowDown', callback: () => move(1) },
      { hotkey: 'K', callback: () => move(-1) },
      { hotkey: 'ArrowUp', callback: () => move(-1) },
      {
        hotkey: 'H',
        callback: () => {
          const hit = selHit()
          if (hit) {
            setExpanded((prev) => {
              const next = new Set(prev)
              next.delete(hit.documentId)
              return next
            })
          }
        },
      },
      {
        hotkey: 'ArrowLeft',
        callback: () => {
          const hit = selHit()
          if (hit) {
            setExpanded((prev) => {
              const next = new Set(prev)
              next.delete(hit.documentId)
              return next
            })
          }
        },
      },
      {
        hotkey: 'L',
        callback: () => {
          const hit = selHit()
          if (hit) setExpanded((prev) => new Set(prev).add(hit.documentId))
        },
      },
      {
        hotkey: 'ArrowRight',
        callback: () => {
          const hit = selHit()
          if (hit) setExpanded((prev) => new Set(prev).add(hit.documentId))
        },
      },
      {
        hotkey: 'Enter',
        callback: () => {
          const hit = selHit()
          if (hit) toggleExpand(hit)
        },
      },
      {
        hotkey: 'O',
        callback: () => {
          const hit = selHit()
          if (hit) openHit(hit)
        },
      },
      { hotkey: 'Escape', callback: () => setSel(-1) },
    ],
    { enabled: !paletteOpen, conflictBehavior: 'allow' },
  )

  const degradedViews = Object.entries(result.manifest.views).filter(
    ([, view]) => view.state !== 'ready',
  )
  const hasCaveats = result.missingCapabilities.length > 0 || degradedViews.length > 0

  return (
    <section aria-label="Search results" {...stylex.props(styles.results)}>
      <div {...stylex.props(styles.meta)}>
        <DisplayBadge text={result.plan.intent}>
          {result.plan.intent.replaceAll('_', ' ')}
        </DisplayBadge>
        <strong {...stylex.props(styles.hitCount)}>
          {hits.length} hit{hits.length === 1 ? '' : 's'}
          {activeCount > 0 && ` · ${activeCount} filter${activeCount === 1 ? '' : 's'}`}
        </strong>
        {activeCount > 0 && (
          <button type="button" onClick={() => setFilters(emptyFilters())} {...stylex.props(styles.clearFilters)}>
            clear
          </button>
        )}
        <span {...stylex.props(styles.metaSep)} />
        <ActionIconButton
          tooltip="Export hits as JSON"
          label="Export results"
          onClick={() => {
            const payload = {
              query: result.request.query,
              snapshotId: result.request.snapshotId,
              latencyMs: result.latencyMs,
              intent: result.plan.intent,
              routes: result.plan.routes,
              hits: hits.map((h) => ({
                rank: h.rank,
                score: h.score,
                path: h.address?.path,
                startLine: h.address?.startLine,
                endLine: h.address?.endLine,
                symbol: h.symbolName,
                representation: h.representation,
                routes: h.contributingRoutes,
                verifiedCurrent: h.verifiedCurrent,
              })),
            }
            const blob = new Blob([JSON.stringify(payload, null, 2)], {
              type: 'application/json',
            })
            const a = document.createElement('a')
            a.href = URL.createObjectURL(blob)
            a.download = `cce-hits-${result.request.snapshotId.slice(0, 8)}.json`
            a.click()
            URL.revokeObjectURL(a.href)
          }}
          icon={<IconDownload size={13} />}
        />
        <DisplayDuration value={result.latencyMs} colorize />
        <code {...stylex.props(styles.snap)} title={`snapshot ${result.request.snapshotId}`}>
          {result.request.snapshotId.slice(0, 12)}
        </code>
        <span {...stylex.props(styles.kbdHint)}>
          <kbd>j</kbd>/<kbd>k</kbd> select · <kbd>h</kbd>/<kbd>l</kbd> fold · <kbd>o</kbd> source · <kbd>↵</kbd> expand
        </span>
      </div>

      {hasCaveats && (
        <FeedbackTip variant="warning" title="Freshness and capability caveats">
          {result.missingCapabilities.map((capability) => (
            <p key={capability} {...stylex.props(styles.caveat)}>
              {capability}
            </p>
          ))}
          {degradedViews.map(([name, view]) => (
            <p key={name} {...stylex.props(styles.caveat)}>
              view <strong>{name}</strong> is {view.state.replaceAll('_', ' ')}
              {view.message ? ` — ${view.message}` : ''}
            </p>
          ))}
        </FeedbackTip>
      )}

      <div {...stylex.props(styles.plan)}>
        <span {...stylex.props(styles.planLabel)}>routes</span>
        {result.plan.routes.length === 0 ? (
          <DisplayBadge color={false}>none</DisplayBadge>
        ) : (
          result.plan.routes.map((route) => (
            <DisplayBadge key={route} text={route}>
              {route.replaceAll('_', ' ')}
            </DisplayBadge>
          ))
        )}
        <span {...stylex.props(styles.planLabel)}>graph</span>
        <DisplayBadge color={false}>{result.plan.graphPolicy.replaceAll('_', ' ')}</DisplayBadge>
        {result.plan.reasons.length > 0 && (
          <details {...stylex.props(styles.planDetails)}>
            <summary {...stylex.props(styles.planWhy)}>planner rationale</summary>
            <ul {...stylex.props(styles.reasons)}>
              {result.plan.reasons.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          </details>
        )}
      </div>

      {result.hits.length === 0 ? (
        <NoHits query={result.request.query} />
      ) : (
        <>
          {result.hits.length >= 4 && (
            <HitBreakdown hits={result.hits} limit={result.request.limit} />
          )}
          <div {...stylex.props(styles.columns)}>
          <FacetRail
            groups={groups}
            active={filters}
            onToggle={(g, v) => setFilters((f) => toggleFilter(f, g, v))}
          />
          <div {...stylex.props(styles.hits)}>
            {hits.length === 0 ? (
              <FeedbackEmptyState
                title="Filters exclude every hit"
                description="Loosen the rail filters or clear them."
              />
            ) : (
              hits.map((hit, i) => (
                <HitCard
                  key={hit.documentId}
                  hit={hit}
                  selected={i === sel}
                  expanded={expanded.has(hit.documentId)}
                  onToggle={() => {
                    setSel(i)
                    toggleExpand(hit)
                  }}
                  onOpenSource={() => openHit(hit)}
                  onDetail={() => setDetail(hit)}
                  onOpenFile={onOpenFile}
                />
              ))
            )}
          </div>
          </div>
        </>
      )}

      <HitDrawer hit={detail} onClose={() => setDetail(undefined)} />
    </section>
  )
}

type Lookup =
  | { kind: 'def' | 'refs'; status: 'loading' }
  | { kind: 'def'; status: 'done'; report: DefinitionsReport }
  | { kind: 'refs'; status: 'done'; report: ReferencesReport }
  | { kind: 'def' | 'refs'; status: 'error'; error: string }

// History/diff hits carry the commit sha as `symbolName` — a resolvable
// symbol is an identifier, not a hash or a `scheme:value` pseudo-id.
const HASHISH = /^[0-9a-f]{24,}$/i

function isSymbolName(name?: string): boolean {
  return !!name && !name.includes(':') && !HASHISH.test(name)
}

/**
 * Query input with Sourcegraph-style in-place syntax highlighting — a
 * transparent-text input over a mirrored token layer (grammar in
 * `lib/querySyntax`). Filter names paint blue (`accentInfo`), boolean
 * operators/parens green (`colorActive`), quoted strings `scaleLow`,
 * negations `scaleHigh`.
 */

function QueryInput({
  value,
  onChange,
  onFocus,
  onClear,
  leading,
  trailing,
  placeholder = 'symbol, path, or question — lang:rust path:crates/ narrows',
  ariaLabel = 'Search query',
  highlightSyntax = true,
}: {
  value: string
  onChange: (v: string) => void
  onFocus?: () => void
  onClear?: () => void
  /** Slotted inside the field's left edge — the context selector. */
  leading?: ReactNode
  /** Slotted at the field's right edge — grep's case toggle. */
  trailing?: ReactNode
  placeholder?: string
  ariaLabel?: string
  /**
   * Engine-token coloring in the mirror. Off in grep mode — painting a
   * regex with query-grammar colors would lie about what runs.
   */
  highlightSyntax?: boolean
}) {
  const mirrorRef = useRef<HTMLDivElement>(null)
  const tokens = useMemo(() => queryTokens(value), [value])
  const syncScroll = (e: React.UIEvent<HTMLInputElement>) => {
    if (mirrorRef.current) mirrorRef.current.scrollLeft = e.currentTarget.scrollLeft
  }
  return (
    <div {...stylex.props(styles.qbox)}>
      {leading}
      <span {...stylex.props(styles.qicon)}>
        <IconSearch size={14} />
      </span>
      <div {...stylex.props(styles.qfield)}>
        <div ref={mirrorRef} {...stylex.props(styles.qmirror)} aria-hidden>
          {highlightSyntax
            ? tokens.map((t, i) =>
                t.kind ? (
                  <span key={i} {...stylex.props(styles[`qtok_${t.kind}`])}>
                    {t.text}
                  </span>
                ) : (
                  t.text
                ),
              )
            : value}
          {'\u200b'}
        </div>
        <input
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onFocus={onFocus}
          onScroll={syncScroll}
          placeholder={placeholder}
          aria-label={ariaLabel}
          spellCheck={false}
          autoComplete="off"
          {...stylex.props(styles.qinput)}
        />
      </div>
      {value && (
        <button type="button" aria-label="Clear" {...stylex.props(styles.qclear)} onClick={onClear}>
          ×
        </button>
      )}
      {trailing}
    </div>
  )
}

/** Sourcegraph's NoResultsPage — honest empty state + runnable examples. */
function NoHits({ query }: { query: string }) {
  const navigate = useNavigate()
  const examples = [
    { label: 'snapshot freshness', q: 'snapshot freshness' },
    { label: 'type:symbol index', q: 'type:symbol index' },
    { label: 'type:file Cargo.toml', q: 'type:file Cargo.toml' },
    { label: 'lang:rust provider', q: 'lang:rust provider' },
  ]
  return (
    <div {...stylex.props(styles.noHits)}>
      <FeedbackEmptyState
        icon={<IconSearch size={20} />}
        title={`No hits for “${query}”`}
        description="The committed snapshot answered honestly — the index holds no match. Try a broader term, a structured filter, or refresh the index."
      />
      <div {...stylex.props(styles.noHitsRow)}>
        <span {...stylex.props(styles.noHitsLabel)}>try</span>
        {examples.map((e) => (
          <button
            key={e.q}
            type="button"
            {...stylex.props(styles.exampleChip)}
            onClick={() => navigate(queryUrl(e.q))}
          >
            <code>{e.label}</code>
          </button>
        ))}
      </div>
    </div>
  )
}

function hitTitle(hit: SearchHit): string {
  const name = hit.symbolName
  if (name && isSymbolName(name)) return name
  if (name && HASHISH.test(name)) {
    return `${hit.representation === 'commit_diff' ? 'commit' : 'document'} ${name.slice(0, 10)}`
  }
  return hit.documentId
}

function HitCard({
  hit,
  selected,
  expanded,
  onToggle,
  onOpenSource,
  onDetail,
  onOpenFile,
}: {
  hit: SearchHit
  selected: boolean
  expanded: boolean
  onToggle: () => void
  onOpenSource: () => void
  onDetail: () => void
  onOpenFile: (path: string, line?: number) => void
}) {
  return (
    <article
      id={`hit-${hit.documentId}`}
      {...stylex.props(styles.hit, selected && styles.hitSel)}
      onClick={onToggle}
      data-expanded={expanded || undefined}
    >
      <div {...stylex.props(styles.hitHead)}>
        <span {...stylex.props(styles.rank)}>#{hit.rank}</span>
        {hit.address && (
          <DisplayFilePath
            path={hit.address.path}
            line={hit.address.startLine}
            endLine={hit.address.endLine}
            icon
          />
        )}
        <span {...stylex.props(styles.score)} title="fused score">
          {hit.score.toPrecision(3)}
        </span>
        <span {...stylex.props(styles.hitActions)} onClick={(e) => e.stopPropagation()}>
          {hit.address && (
            <ActionIconButton
              tooltip={`Open ${hit.address.path}:${hit.address.startLine}`}
              label="Open source file"
              onClick={onOpenSource}
              icon={<IconArrowUpRight size={13} />}
            />
          )}
          <ActionIconButton
            tooltip="Full provenance"
            label="Open hit details"
            onClick={onDetail}
            icon={<IconEllipsis size={13} />}
          />
        </span>
      </div>
      {hit.symbolName && <h3 {...stylex.props(styles.hitTitle)}>{hitTitle(hit)}</h3>}
      {hit.snippet && (
        <div {...stylex.props(styles.snippetWrap)}>
          <SnippetView
            snippet={hit.snippet}
            startLine={hit.address?.startLine}
            maxLines={expanded ? 40 : 8}
            language={langFromPath(hit.address?.path)}
          />
        </div>
      )}
      <div {...stylex.props(styles.chips)}>
        {hit.contributingRoutes.map((route) => (
          <DisplayBadge key={route} text={route}>
            {route.replaceAll('_', ' ')}
          </DisplayBadge>
        ))}
        <DisplayBadge color={false}>{hit.representation.replaceAll('_', ' ')}</DisplayBadge>
        {!hit.verifiedCurrent && <DisplayBadge severity="medium">unverified</DisplayBadge>}
      </div>
      {hit.explanation.length > 0 && (
        <p {...stylex.props(styles.why)}>{hit.explanation.join(' · ')}</p>
      )}
      {expanded && hit.evidence.length > 0 && (
        <ul {...stylex.props(styles.evidenceList)}>
          {hit.evidence.map((address, i) => (
            <li key={i}>
              <button
                type="button"
                {...stylex.props(styles.evidenceRow)}
                onClick={(e) => {
                  e.stopPropagation()
                  onOpenFile(address.path, address.startLine)
                }}
                title={`open ${formatAddress(address)}`}
              >
                <DisplayFilePath
                  path={address.path}
                  line={address.startLine}
                  endLine={address.endLine}
                  icon
                />
                <IconArrowUpRight size={11} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </article>
  )
}

/**
 * Hit detail drawer — full provenance (Sourcegraph's result expansion as
 * a panel). def/refs lookups moved here so the card stays scannable.
 */
function HitDrawer({ hit, onClose }: { hit?: SearchHit; onClose: () => void }) {
  const [lookup, setLookup] = useState<Lookup>()
  const lastHit = useRef<SearchHit | undefined>(undefined)
  if (hit !== lastHit.current) {
    lastHit.current = hit
    setLookup(undefined)
  }

  async function runLookup(kind: 'def' | 'refs') {
    const name = hit?.symbolName
    if (!name) return
    setLookup({ kind, status: 'loading' })
    try {
      if (kind === 'def') {
        setLookup({ kind, status: 'done', report: await api.definitions(name) })
      } else {
        setLookup({ kind, status: 'done', report: await api.references(name) })
      }
    } catch (value) {
      setLookup({ kind, status: 'error', error: describeError(value) })
    }
  }

  const canLookup = isSymbolName(hit?.symbolName)

  return (
    <OverlayDrawer
      open={hit !== undefined}
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
      title={hit ? hitTitle(hit) : undefined}
      width={440}
    >
      {hit && (
        <div {...stylex.props(styles.drawer)}>
          <div {...stylex.props(styles.chips)}>
            {hit.contributingRoutes.map((route) => (
              <DisplayBadge key={route} text={route}>
                {route.replaceAll('_', ' ')}
              </DisplayBadge>
            ))}
            <DisplayBadge color={false}>{hit.representation.replaceAll('_', ' ')}</DisplayBadge>
            {!hit.verifiedCurrent && <DisplayBadge severity="medium">unverified</DisplayBadge>}
          </div>

          {hit.snippet && (
            <div {...stylex.props(styles.snippetWrap)}>
              <SnippetView
                snippet={hit.snippet}
                startLine={hit.address?.startLine}
                maxLines={40}
                language={langFromPath(hit.address?.path)}
              />
            </div>
          )}

          <section>
            <h4 {...stylex.props(styles.sectionTitle)}>Provenance</h4>
            <div {...stylex.props(styles.kvGrid)}>
              <DisplayKeyValue label="score" value={hit.score.toPrecision(4)} />
              <DisplayKeyValue label="rank" value={`#${hit.rank}`} />
              <DisplayKeyValue label="route" value={hit.route} />
              <DisplayKeyValue label="document" value={<Breakable>{hit.documentId}</Breakable>} />
              <DisplayKeyValue label="entity" value={<Breakable>{hit.entityId}</Breakable>} />
              {hit.regionId && (
                <DisplayKeyValue label="region" value={<Breakable>{hit.regionId}</Breakable>} />
              )}
              {hit.address && (
                <DisplayKeyValue label="address" value={formatAddress(hit.address)} />
              )}
            </div>
          </section>

          {hit.evidence.length > 1 && (
            <section>
              <h4 {...stylex.props(styles.sectionTitle)}>Evidence</h4>
              <ul {...stylex.props(styles.evidence)}>
                {hit.evidence.map((address: SourceAddress, i) => (
                  <li key={i} {...stylex.props(styles.evidenceLine)}>
                    <DisplayFilePath path={address.path} line={address.startLine} endLine={address.endLine} />
                  </li>
                ))}
              </ul>
            </section>
          )}

          {hit.explanation.length > 0 && (
            <section>
              <h4 {...stylex.props(styles.sectionTitle)}>Why this hit</h4>
              <ul {...stylex.props(styles.evidence)}>
                {hit.explanation.map((line, i) => (
                  <li key={i} {...stylex.props(styles.evidenceLine)}>
                    {line}
                  </li>
                ))}
              </ul>
            </section>
          )}

          {canLookup && (
            <section {...stylex.props(styles.lookupActions)}>
              <ActionButton disabled={lookup?.status === 'loading'} onClick={() => void runLookup('def')}>
                definition
              </ActionButton>
              <ActionButton disabled={lookup?.status === 'loading'} onClick={() => void runLookup('refs')}>
                references
              </ActionButton>
            </section>
          )}
          {lookup && <LookupResult lookup={lookup} />}
        </div>
      )}
    </OverlayDrawer>
  )
}

function Breakable({ children }: { children: React.ReactNode }) {
  return <span {...stylex.props(styles.breakable)}>{children}</span>
}

function LookupResult({ lookup }: { lookup: Lookup }) {
  if (lookup.status === 'loading') {
    return <p {...stylex.props(styles.lookup, styles.lookupMuted)}>resolving {lookup.kind}…</p>
  }
  if (lookup.status === 'error') {
    return <p {...stylex.props(styles.lookup, styles.lookupError)}>{lookup.error}</p>
  }
  if (lookup.kind === 'def') {
    return (
      <div {...stylex.props(styles.lookup)}>
        {lookup.report.definitions.length === 0 ? (
          <p {...stylex.props(styles.lookupMuted)}>no definitions found</p>
        ) : (
          lookup.report.definitions.map((definition, index) => (
            <p key={`${definition.name}:${index}`} {...stylex.props(styles.lookupLine)}>
              <strong>{definition.name}</strong>{' '}
              <span {...stylex.props(styles.lookupMuted)}>{definition.kind}</span>
              {definition.qualifiedName && (
                <span {...stylex.props(styles.lookupMuted)}> · {definition.qualifiedName}</span>
              )}
              {definition.address && (
                <>
                  {' — '}
                  <code {...stylex.props(styles.lookupCode)}>{formatAddress(definition.address)}</code>
                </>
              )}
            </p>
          ))
        )}
      </div>
    )
  }
  return (
    <div {...stylex.props(styles.lookup)}>
      {lookup.report.references.length === 0 ? (
        <p {...stylex.props(styles.lookupMuted)}>no references found</p>
      ) : (
        lookup.report.references.map((reference, index) => (
          <p key={`${reference.fromName}:${index}`} {...stylex.props(styles.lookupLine)}>
            <strong>{reference.fromName}</strong>{' '}
            <span {...stylex.props(styles.lookupMuted)}>
              {reference.via} · {reference.origin} · conf {reference.confidence}
            </span>
            {reference.evidence[0] && (
              <>
                {' — '}
                <code {...stylex.props(styles.lookupCode)}>{formatAddress(reference.evidence[0])}</code>
              </>
            )}
          </p>
        ))
      )}
      {lookup.report.truncated && (
        <p {...stylex.props(styles.lookupMuted)}>list truncated — narrow the name</p>
      )}
    </div>
  )
}

/**
 * Hit breakdown — client-side aggregation over the RETURNED hit set
 * (Sourcegraph's `count:`-style grouping, honest scope): top files by hit
 * share, retrieval-route mix, and representation mix. The counts describe
 * the hits the engine returned, not the corpus — the note says so.
 */
function HitBreakdown({ hits, limit }: { hits: SearchHit[]; limit: number }) {
  const [open, setOpen] = useState(false)
  const navigate = useNavigate()

  const byFile = useMemo(() => {
    const m = new Map<string, number>()
    for (const h of hits) {
      const p = h.address?.path
      if (p) m.set(p, (m.get(p) ?? 0) + 1)
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 8)
  }, [hits])

  const byRoute = useMemo(() => {
    const m = new Map<string, number>()
    for (const h of hits) {
      const rs = h.contributingRoutes.length > 0 ? h.contributingRoutes : [h.route]
      for (const r of rs) m.set(r, (m.get(r) ?? 0) + 1)
    }
    return [...m.entries()].sort((a, b) => b[1] - a[1])
  }, [hits])

  const byRep = useMemo(() => {
    const m = new Map<string, number>()
    for (const h of hits) m.set(h.representation, (m.get(h.representation) ?? 0) + 1)
    return [...m.entries()].sort((a, b) => b[1] - a[1])
  }, [hits])

  const fileTotal = byFile.reduce((n, [, c]) => n + c, 0) || 1

  return (
    <div {...stylex.props(styles.bd)}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        {...stylex.props(styles.bdHead)}
      >
        <span {...stylex.props(styles.bdCaret, open && styles.bdCaretOpen)}>
          <IconCaretDown size={10} />
        </span>
        <span {...stylex.props(styles.bdTitle)}>breakdown</span>
        <span {...stylex.props(styles.bdHint)}>
          {byFile.length} files · {byRoute.map(([r, c]) => `${r.replaceAll('_', ' ')} ${c}`).join(' · ')}
        </span>
      </button>
      {open && (
        <div {...stylex.props(styles.bdBody)}>
          <div {...stylex.props(styles.bdCol)}>
            <div {...stylex.props(styles.bdLabel)}>top files</div>
            {byFile.map(([path, n]) => (
              <button
                key={path}
                type="button"
                title={`open ${path}`}
                onClick={() => navigate(fileUrl(path))}
                {...stylex.props(styles.bdFile)}
              >
                <span {...stylex.props(styles.bdFileBar)}>
                  <span
                    {...stylex.props(styles.bdFileFill)}
                    style={{ width: `${(n / fileTotal) * 100}%` }}
                  />
                </span>
                <DisplayFilePath path={path} />
                <span {...stylex.props(styles.bdFileN)}>{n}</span>
              </button>
            ))}
          </div>
          <div {...stylex.props(styles.bdCol)}>
            <div {...stylex.props(styles.bdLabel)}>routes</div>
            <DisplayProportionBar
              segments={byRoute.map(([r, c]) => ({
                value: c,
                label: `${r.replaceAll('_', ' ')} · ${c}`,
              }))}
              showLegend
            />
            <div {...stylex.props(styles.bdLabel)}>representation</div>
            <DisplayProportionBar
              segments={byRep.map(([r, c]) => ({
                value: c,
                label: `${r.replaceAll('_', ' ')} · ${c}`,
              }))}
              showLegend
            />
          </div>
          <div {...stylex.props(styles.bdFoot)}>
            aggregated over the {hits.length} returned hit{hits.length === 1 ? '' : 's'}
            {hits.length >= limit ? ' (limit reached — not corpus-wide)' : ''}
          </div>
        </div>
      )}
    </div>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },
  bar: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
  },
  field: {
    flex: 1,
    minWidth: 220,
    position: 'relative',
  },
  qbox: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    height: 32,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: {
      default: vars.borderBase,
      ':focus-within': vars.borderActive,
    },
    backgroundColor: vars.bgRaised,
    paddingLeft: 8,
    paddingRight: 6,
    boxSizing: 'border-box',
    transitionProperty: 'border-color, box-shadow',
    transitionDuration: '120ms',
    boxShadow: {
      default: 'none',
      ':focus-within': `0 0 0 2px ${vars.ringPrimary}`,
    },
  },
  qicon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  qfield: {
    position: 'relative',
    flex: 1,
    minWidth: 0,
    height: '100%',
  },
  qmirror: {
    position: 'absolute',
    inset: 0,
    overflow: 'hidden',
    whiteSpace: 'pre',
    pointerEvents: 'none',
    fontFamily: font.mono,
    fontSize: 12,
    lineHeight: '18px',
    paddingTop: 6,
    paddingBottom: 6,
    color: vars.colorBase,
  },
  qinput: {
    position: 'absolute',
    inset: 0,
    width: '100%',
    borderWidth: 0,
    outline: 'none',
    backgroundColor: 'transparent',
    color: 'transparent',
    caretColor: vars.colorBase,
    fontFamily: font.mono,
    fontSize: 12,
    lineHeight: '18px',
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 0,
    paddingRight: 0,
    '::placeholder': {
      color: vars.colorFaint,
    },
  },
  qclear: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 16,
    height: 16,
    padding: 0,
    borderWidth: 0,
    borderRadius: '50%',
    backgroundColor: vars.bgAmbient,
    color: vars.colorMuted,
    fontSize: 12,
    lineHeight: 1,
    cursor: 'pointer',
    flexShrink: 0,
  },
  qtok_filter: {
    color: vars.accentInfo,
  },
  qtok_op: {
    color: vars.colorActive,
  },
  qtok_str: {
    color: vars.scaleLow,
  },
  qtok_paren: {
    color: vars.colorActive,
    fontWeight: 600,
  },
  limit: {
    width: 76,
    height: 32,
  },
  results: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  meta: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  hitCount: {
    fontSize: 13,
    fontWeight: 600,
  },
  clearFilters: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorActive,
    fontSize: 11,
    fontFamily: 'inherit',
    padding: 0,
    cursor: 'pointer',
    opacity: { default: vars.opFade, ':hover': 1 },
  },
  metaSep: {
    flex: 1,
  },
  snap: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  kbdHint: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
    display: { default: 'inline', '@media (max-width: 900px)': 'none' },
  },
  caveat: {
    margin: 0,
  },
  plan: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
    fontSize: 11,
    color: vars.colorMuted,
  },
  planLabel: {
    color: vars.colorFaint,
    fontSize: 10,
    textTransform: 'uppercase',
    letterSpacing: '0.04em',
  },
  planDetails: {
    flexBasis: '100%',
    minWidth: 0,
  },
  planWhy: {
    fontSize: 11,
    color: vars.colorMuted,
    fontWeight: 400,
    cursor: 'pointer',
    display: 'inline-block',
  },
  reasons: {
    margin: 0,
    paddingLeft: 18,
    color: vars.colorMuted,
    fontSize: 11,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  columns: {
    display: 'flex',
    gap: 16,
    alignItems: 'flex-start',
  },
  hits: {
    flex: 1,
    minWidth: 0,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  hit: {
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    cursor: 'pointer',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    transitionProperty: 'background-color',
    transitionDuration: '100ms',
  },
  hitSel: {
    boxShadow: `inset 2px 0 0 ${vars.colorActive}`,
    backgroundColor: vars.bgHover,
  },
  hitActions: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
    flexShrink: 0,
  },
  evidenceList: {
    margin: 0,
    marginTop: 6,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  evidenceRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 11,
    color: vars.colorMuted,
    padding: '3px 6px',
    marginLeft: -6,
    borderRadius: 4,
    cursor: 'pointer',
    textAlign: 'left',
    ':hover': {
      backgroundColor: vars.bgHover,
      color: vars.colorBase,
    },
  },
  hitHead: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 8,
    minWidth: 0,
  },
  rank: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  hitTitle: {
    margin: 0,
    marginTop: 2,
    fontSize: 12,
    fontWeight: 600,
    fontFamily: font.mono,
    overflowWrap: 'anywhere',
    minWidth: 0,
  },
  score: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.scaleLow,
    flexShrink: 0,
  },
  snippetWrap: {
    marginTop: 6,
    borderRadius: 6,
    backgroundColor: vars.bgCode,
    overflow: 'hidden',
  },
  chips: {
    display: 'flex',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 4,
    marginTop: 6,
  },
  why: {
    marginTop: 5,
    marginBottom: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  noHits: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    alignItems: 'center',
    paddingTop: 18,
    paddingBottom: 10,
  },
  noHitsRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
    justifyContent: 'center',
  },
  noHitsLabel: {
    color: vars.colorFaint,
    fontSize: 10,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    fontWeight: 600,
  },
  exampleChip: {
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 999,
    backgroundColor: { default: vars.bgRaised, ':hover': vars.bgHover },
    color: vars.colorMuted,
    fontSize: 11,
    fontFamily: 'inherit',
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 10,
    paddingRight: 10,
    cursor: 'pointer',
  },
  drawer: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    fontSize: 12,
  },
  sectionTitle: {
    margin: 0,
    marginBottom: 4,
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    color: vars.colorFaint,
  },
  kvGrid: {
    display: 'grid',
    gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))',
    gap: '4px 12px',
  },
  evidence: {
    margin: 0,
    paddingLeft: 16,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    color: vars.colorMuted,
  },
  evidenceLine: {
    fontSize: 11,
  },
  breakable: {
    overflowWrap: 'anywhere',
    minWidth: 0,
  },
  lookupActions: {
    display: 'flex',
    gap: 8,
  },
  lookup: {
    paddingLeft: 10,
    borderLeftWidth: 2,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderBase,
    fontSize: 11,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    margin: 0,
  },
  lookupLine: {
    margin: 0,
    fontSize: 11,
  },
  lookupMuted: {
    color: vars.colorMuted,
  },
  lookupError: {
    color: vars.scaleCritical,
  },
  lookupCode: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
  },
  // --- worktree grep ---------------------------------------------------------
  caseToggle: {
    fontFamily: font.mono,
    fontSize: 11,
    fontStyle: 'italic',
    flexShrink: 0,
  },
  grepStats: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
  },
  grepNote: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
  },
  grepEmpty: {
    margin: 0,
    fontSize: 12,
    color: vars.colorMuted,
  },
  grepRow: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
    width: '100%',
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 12,
    paddingRight: 12,
    borderWidth: 0,
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    transitionProperty: 'background-color',
    transitionDuration: '100ms',
  },
  grepLoc: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorActive,
    minWidth: 0,
    overflowWrap: 'anywhere',
    flexShrink: 0,
    maxWidth: '55%',
  },
  grepText: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    whiteSpace: 'pre-wrap',
    overflowWrap: 'anywhere',
    minWidth: 0,
    flex: 1,
  },
  bd: {
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
  },
  bdHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    width: '100%',
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 12,
    paddingRight: 12,
    cursor: 'pointer',
    fontFamily: 'inherit',
    textAlign: 'left',
    borderRadius: 8,
  },
  bdCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(-90deg)',
    transition: 'transform 120ms',
  },
  bdCaretOpen: {
    transform: 'rotate(0deg)',
  },
  bdTitle: {
    fontSize: 10,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorMuted,
  },
  bdHint: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  bdBody: {
    display: 'flex',
    gap: 24,
    flexWrap: 'wrap',
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  bdCol: {
    flex: 1,
    minWidth: 220,
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
  },
  bdLabel: {
    fontSize: 9.5,
    fontWeight: 600,
    letterSpacing: '0.06em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingTop: 4,
    paddingBottom: 2,
  },
  bdFile: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    textAlign: 'left',
    borderRadius: 4,
  },
  bdFileBar: {
    width: 40,
    height: 4,
    borderRadius: 2,
    backgroundColor: vars.bgSunken,
    flexShrink: 0,
    overflow: 'hidden',
  },
  bdFileFill: {
    display: 'block',
    height: '100%',
    backgroundColor: vars.accentInfo,
    borderRadius: 2,
  },
  bdFileN: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  bdFoot: {
    flexBasis: '100%',
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 6,
  },
})
