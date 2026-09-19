import * as stylex from '@stylexjs/stylex'
import { useEffect, useMemo, useRef, useState } from 'react'
import { Link } from 'react-router'
import {
  api,
  describeError,
  FileListReport,
  ProviderReport,
  ProviderState,
  ViewManifest,
  ViewState,
} from '../api'
import { useOpenFile } from '../lib/navigation'
import { queryTokens } from '../lib/querySyntax'
import { useSearchHistory } from '../lib/searchHistory'
import { useAppStore } from '../lib/store'
import { useQueryTelemetry } from '../lib/telemetry'
import { DisplayDuration, DisplayTimeAgo } from '../ui/DisplayNumber'
import { DisplayKbd } from '../ui/DisplayKbd'
import { FeedbackSkeleton } from '../ui/FeedbackStates'
import { FeedbackTip } from '../ui/FeedbackTip'
import {
  IconFile,
  IconHistoryClock,
  IconSearch,
  IconStar,
} from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

const STATE_COLOR: Record<ViewState, string> = {
  ready: vars.scaleLow,
  building: vars.scaleMedium,
  partial: vars.scaleMedium,
  stale: vars.scaleHigh,
  unavailable: vars.scaleCritical,
  failed: vars.scaleCritical,
}

const PROVIDER_STATE_COLOR: Record<ProviderState, string> = {
  ready: vars.scaleLow,
  missing: vars.scaleMedium,
  not_applicable: vars.colorFaint,
  failed: vars.scaleCritical,
}

/**
 * Home — a Sourcegraph-style launchpad. The search input is the product:
 * centered hero with recent/saved/suggested queries, a live status ticker
 * above, and a disciplined insight grid below. Latency/volume/hit-mix fall
 * back to organized `preview` fixtures until real measurements exist.
 */
export function DashboardScreen({
  manifest,
  providers,
  providersError,
  statusError,
  loading,
  onRunQuery,
}: {
  manifest?: ViewManifest
  providers?: ProviderReport[]
  providersError?: string
  statusError?: string
  loading: boolean
  onRunQuery: (query: string) => void
}) {
  const [files, setFiles] = useState<FileListReport>()
  const [filesError, setFilesError] = useState<string>()

  useEffect(() => {
    api
      .files()
      .then(setFiles)
      .catch((e) => setFilesError(describeError(e)))
  }, [manifest?.snapshotId])

  const telemetry = useQueryTelemetry()
  const { recents, saved } = useSearchHistory()
  const recentFiles = useAppStore((s) => s.recentFiles)
  const openFile = useOpenFile()

  const views = useMemo(() => (manifest ? Object.entries(manifest.views) : []), [manifest])
  const languages = useMemo(() => {
    if (!files) return []
    const counts = new Map<string, number>()
    for (const file of files.files) {
      const lang = file.language ?? 'other'
      counts.set(lang, (counts.get(lang) ?? 0) + 1)
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1])
  }, [files])

  const readyCount = views.filter(([, v]) => v.state === 'ready').length
  const degradedCount = views.length - readyCount
  const providersReady = providers?.filter((p) => p.state === 'ready').length ?? 0
  const lastUpdate = views.length
    ? views.map(([, v]) => v.updatedAt).reduce((a, b) => (b > a ? b : a))
    : undefined
  const maxViewAge = useMemo(() => {
    const now = Date.now()
    return views.reduce(
      (m, [, v]) => Math.max(m, now - new Date(v.updatedAt).getTime()),
      1,
    )
  }, [views])
  // Ticker latency only reports real measurements — fixtures stay on Index.
  const avgLatency = telemetry.length
    ? Math.round(telemetry.reduce((n, t) => n + t.latencyMs, 0) / telemetry.length)
    : undefined

  // Top directories — real structure, used by examples + the coverage band.
  const topDirs = useMemo(() => {
    if (!files) return []
    const dirs = new Map<string, Map<string, number>>() // dir → lang → count
    for (const f of files.files) {
      const seg = f.path.includes('/') ? f.path.split('/')[0]! : '.'
      const langs = dirs.get(seg) ?? new Map<string, number>()
      langs.set(f.language ?? 'other', (langs.get(f.language ?? 'other') ?? 0) + 1)
      dirs.set(seg, langs)
    }
    return [...dirs.entries()]
      .map(([dir, langs]) => ({
        dir,
        files: [...langs.values()].reduce((a, b) => a + b, 0),
        langs: [...langs.entries()].sort((a, b) => b[1] - a[1]),
      }))
      .sort((a, b) => b.files - a.files)
  }, [files])

  // Runnable examples derived from this snapshot — grouped by what they teach.
  const exampleGroups = useMemo(() => {
    if (!files || files.files.length === 0) return []
    const scope: { q: string; purpose: string; hint: string }[] = []
    const [topLang, topLangCount] = languages[0] ?? []
    if (topLang && topLang !== 'other') {
      scope.push({ q: `lang:${topLang}`, purpose: `${topLangCount} files by language`, hint: 'lexical' })
    }
    const secondLang = languages.find(([l]) => l !== 'other' && l !== topLang)
    if (secondLang) {
      scope.push({ q: `lang:${secondLang[0]}`, purpose: `${secondLang[1]} files by language`, hint: 'lexical' })
    }
    const topRoot = topDirs[0]
    if (topRoot && topRoot.dir !== '.') {
      scope.push({ q: `path:${topRoot.dir}/`, purpose: `scoped to ${topRoot.dir}/ · ${topRoot.files} files`, hint: 'path filter' })
    }
    const secondDir = topDirs.find((d) => d.dir !== '.' && d.dir !== topRoot?.dir)
    if (secondDir) {
      scope.push({ q: `path:${secondDir.dir}/`, purpose: `scoped to ${secondDir.dir}/ · ${secondDir.files} files`, hint: 'path filter' })
    }
    const recentName = recentFiles[0]?.path.split('/').pop()
    const routes = [
      {
        q: `type:file ${recentName ?? 'Cargo.toml'}`,
        purpose: recentName ? 'a file you opened — type:file' : 'files by name — type:file',
        hint: 'exact',
      },
      { q: 'type:symbol snapshot', purpose: 'symbol definitions — type:symbol', hint: 'symbol route' },
      { q: 'type:diff index', purpose: 'history diffs — type:diff', hint: 'history route' },
      { q: 'type:commit pack', purpose: 'commit hits — type:commit', hint: 'history route' },
    ]
    return [
      { label: 'scope & filters', rows: scope },
      { label: 'typed routes', rows: routes },
    ]
  }, [files, languages, topDirs, recentFiles])
  const healthColor =
    views.length === 0
      ? vars.colorFaint
      : degradedCount === 0
        ? vars.scaleLow
        : degradedCount > 2
          ? vars.scaleCritical
          : vars.scaleMedium

  return (
    <div {...stylex.props(styles.root)}>
      {statusError && <FeedbackTip variant="error">{statusError}</FeedbackTip>}

      {/* ── Live status ticker — one mono line, honest state ── */}
      <div {...stylex.props(styles.ticker, anim.rise)}>
        <span {...stylex.props(styles.tickerDot)} style={{ backgroundColor: healthColor }} />
        {manifest ? (
          <>
            <code {...stylex.props(styles.tickerSnap)}>{manifest.snapshotId.slice(0, 12)}</code>
            <TickerSep />
            <span>{files ? `${files.files.length.toLocaleString()} files` : '… files'}</span>
            <TickerSep />
            <span>
              {readyCount}/{views.length} views ready
            </span>
            {degradedCount > 0 && (
              <>
                <TickerSep />
                <span {...stylex.props(styles.tickerDegraded)}>{degradedCount} degraded</span>
              </>
            )}
            <TickerSep />
            <span>
              indexed <DisplayTimeAgo value={lastUpdate ?? manifest.snapshotId} />
            </span>
          </>
        ) : (
          <span>{loading ? 'reading snapshot…' : 'no snapshot'}</span>
        )}
        {avgLatency != null && (
          <>
            <TickerSep />
            <span>avg {fmtMs(avgLatency)}</span>
          </>
        )}
        {providers && providers.length > 0 && (
          <>
            <TickerSep />
            <span>
              {providersReady}/{providers.length} providers
            </span>
          </>
        )}
      </div>

      {/* ── Hero: the search is the product ── */}
      <div {...stylex.props(styles.hero, anim.rise)} style={{ animationDelay: '55ms' }}>
        <div {...stylex.props(styles.heroEyebrow)}>repository intelligence</div>
        <h1 {...stylex.props(styles.heroTitle)}>{manifest?.repositoryId ?? 'CCE'}</h1>
        <HeroSearch onRun={onRunQuery} />
        <div {...stylex.props(styles.chipRows)}>
          {recents.length > 0 && (
            <ChipRow label="recent" icon={<IconHistoryClock size={11} />}>
              {recents.slice(0, 5).map((q) => (
                <Chip key={q} onClick={() => onRunQuery(q)}>
                  {q}
                </Chip>
              ))}
            </ChipRow>
          )}
          {saved.length > 0 && (
            <ChipRow label="saved" icon={<IconStar size={11} filled />}>
              {saved.slice(0, 5).map((q) => (
                <Chip key={q} onClick={() => onRunQuery(q)}>
                  {q}
                </Chip>
              ))}
            </ChipRow>
          )}
          {recentFiles.length > 0 && (
            <ChipRow label="files" icon={<IconFile size={11} />}>
              {recentFiles.slice(0, 5).map((f) => (
                <Chip key={f.path} onClick={() => openFile(f.path)}>
                  {f.path.split('/').pop()}
                </Chip>
              ))}
            </ChipRow>
          )}
        </div>
      </div>

      {/* ── START — runnable examples derived from this snapshot + resume ── */}
      <section {...stylex.props(styles.band, anim.rise)} style={{ animationDelay: '110ms' }}>
        <header {...stylex.props(styles.bandHead)}>
          <span {...stylex.props(styles.bandLabel)}>start</span>
          <span {...stylex.props(styles.bandMeta)}>
            {files ? `examples derived from ${files.files.length.toLocaleString()} indexed files` : 'reading files…'}
          </span>
        </header>
        <div {...stylex.props(styles.bandRow)}>
          <div {...stylex.props(styles.bandCol)} style={{ flexBasis: '54%' }}>
            <BandSub label="query examples" aside="click to run" />
            {filesError ? (
              <FeedbackTip variant="error">{filesError}</FeedbackTip>
            ) : !files ? (
              <FeedbackSkeleton height={180} />
            ) : (
              exampleGroups.map((group) => (
                <div key={group.label} {...stylex.props(styles.exampleGroup)}>
                  <span {...stylex.props(styles.exampleGroupLabel)}>{group.label}</span>
                  <div {...stylex.props(styles.exampleList)}>
                    {group.rows.map((ex) => (
                      <button
                        key={ex.q}
                        type="button"
                        {...stylex.props(styles.exampleRow)}
                        onClick={() => onRunQuery(ex.q)}
                      >
                        <code {...stylex.props(styles.exampleQuery)}>
                          {queryTokens(ex.q).map((t, i) =>
                            t.kind ? (
                              <span key={i} {...stylex.props(styles[`qtok_${t.kind}`])}>
                                {t.text}
                              </span>
                            ) : (
                              t.text
                            ),
                          )}
                        </code>
                        <span {...stylex.props(styles.examplePurpose)}>{ex.purpose}</span>
                        <span {...stylex.props(styles.exampleHint)}>{ex.hint}</span>
                      </button>
                    ))}
                  </div>
                </div>
              ))
            )}
          </div>
          <span {...stylex.props(styles.colSep)} />
          <div {...stylex.props(styles.bandCol)} style={{ flexBasis: '46%' }}>
            <BandSub label="jump back in" aside="your session" />
            <ResumeList
              recents={recents}
              saved={saved}
              recentFiles={recentFiles}
              onRunQuery={onRunQuery}
              onOpenFile={openFile}
            />
          </div>
        </div>
      </section>

      {/* ── STATE — one honest strip; detail lives on Index ── */}
      <section {...stylex.props(styles.band, anim.rise)} style={{ animationDelay: '165ms' }}>
        <header {...stylex.props(styles.bandHead)}>
          <span {...stylex.props(styles.bandLabel)}>snapshot state</span>
          <span {...stylex.props(styles.bandMeta)}>
            {readyCount}/{views.length} views ready
            {degradedCount > 0 && ` · ${degradedCount} degraded`}
            {providers && ` · ${providersReady}/${providers.length} providers`}
          </span>
          <Link to="/index" {...stylex.props(styles.bandLink)}>
            open index →
          </Link>
        </header>
        <div {...stylex.props(styles.freshGrid)}>
          {loading && !manifest ? (
            <FeedbackSkeleton height={24} width={320} />
          ) : (
            views.map(([name, view]) => (
              <FreshnessCell key={name} name={name} view={view} maxAge={maxViewAge} />
            ))
          )}
          {providersError ? (
            <span {...stylex.props(styles.freshMuted)}>{providersError}</span>
          ) : (
            providers &&
            providers.length > 0 && (
              <span {...stylex.props(styles.freshProviders)}>
                {providers.map((p) => (
                  <span key={p.providerId} {...stylex.props(styles.freshProvider)} title={p.message}>
                    <span
                      {...stylex.props(styles.stateDot)}
                      style={{ backgroundColor: PROVIDER_STATE_COLOR[p.state] }}
                    />
                    {p.providerId}
                    {p.durationMs != null && (
                      <span {...stylex.props(styles.freshDim)}>
                        <DisplayDuration value={p.durationMs} />
                      </span>
                    )}
                  </span>
                ))}
              </span>
            )
          )}
        </div>
        {degradedCount > 0 && (
          <p {...stylex.props(styles.stateNote)}>
            degraded views answer from older data — routes list which view backs each on Index.
          </p>
        )}
      </section>
    </div>
  )
}

// ── helpers ─────────────────────────────────────────────────────────────────

function fmtMs(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`
}

function relTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime()
  const m = Math.round(diff / 60000)
  if (m < 1) return 'now'
  if (m < 60) return `${m}m`
  return `${Math.round(m / 60)}h`
}

function TickerSep() {
  return <span {...stylex.props(styles.tickerSep)}>·</span>
}

/** The hero search field — large, centered, autofocused launchpad input. */
function HeroSearch({ onRun }: { onRun: (query: string) => void }) {
  const [q, setQ] = useState('')
  const ref = useRef<HTMLInputElement>(null)

  useEffect(() => {
    ref.current?.focus()
  }, [])

  return (
    <form
      {...stylex.props(styles.heroForm)}
      onSubmit={(e) => {
        e.preventDefault()
        if (q.trim()) onRun(q)
      }}
    >
      <IconSearch size={15} />
      <input
        ref={ref}
        value={q}
        onChange={(e) => setQ(e.target.value)}
        placeholder="Search this snapshot — try type:symbol, type:file, type:commit"
        aria-label="Search this snapshot"
        {...stylex.props(styles.heroInput)}
      />
      <DisplayKbd keys="Enter" />
    </form>
  )
}

function ChipRow({
  label,
  icon,
  children,
}: {
  label: string
  icon: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <div {...stylex.props(styles.chipRow)}>
      <span {...stylex.props(styles.chipLabel)}>
        {icon}
        {label}
      </span>
      <div {...stylex.props(styles.chipGroup)}>{children}</div>
    </div>
  )
}

function Chip({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <button type="button" onClick={onClick} {...stylex.props(styles.chip)}>
      {children}
    </button>
  )
}

// Staggered mount — cards rise 6px and fade in, 55ms apart.
const riseKeyframes = stylex.keyframes({
  from: { opacity: 0, transform: 'translateY(6px)' },
  to: { opacity: 1, transform: 'translateY(0)' },
})
const anim = stylex.create({
  rise: {
    animationName: riseKeyframes,
    animationDuration: '420ms',
    animationTimingFunction: 'cubic-bezier(0.22, 1, 0.36, 1)',
    animationFillMode: 'backwards',
  },
})

/** Small mono sub-header inside a band column — label + right aside. */
function BandSub({ label, aside }: { label: string; aside?: string }) {
  return (
    <div {...stylex.props(styles.bandSub)}>
      <span {...stylex.props(styles.bandSubLabel)}>{label}</span>
      {aside && <span {...stylex.props(styles.bandSubAside)}>{aside}</span>}
    </div>
  )
}
/**
 * One view = one cell: state dot, name, an age bar (fill = age relative to
 * the oldest view in this snapshot), and the age text. The whole strip is a
 * horizontal "staleness dial" — real freshness data, no summary collapse.
 */
function FreshnessCell({
  name,
  view,
  maxAge,
}: {
  name: string
  view: ViewManifest['views'][string]
  maxAge: number
}) {
  const age = Math.max(Date.now() - new Date(view.updatedAt).getTime(), 0)
  const frac = Math.min(age / maxAge, 1)
  const color = STATE_COLOR[view.state]
  const caps = view.capabilities.map((c) => c.name).join(', ')
  return (
    <div
      {...stylex.props(styles.freshCell)}
      title={`${name} — ${view.state.replaceAll('_', ' ')} · updated ${relTime(view.updatedAt)} ago${caps ? `\n${caps}` : ''}${view.message ? `\n${view.message}` : ''}`}
    >
      <span {...stylex.props(styles.stateDot)} style={{ backgroundColor: color }} />
      <span {...stylex.props(styles.freshName)}>{name}</span>
      <span {...stylex.props(styles.freshTrack)}>
        <span
          {...stylex.props(styles.freshFill)}
          style={{ width: `${Math.max(frac * 100, 6)}%`, backgroundColor: color }}
        />
      </span>
      <span {...stylex.props(styles.freshAge)}>
        <DisplayTimeAgo value={view.updatedAt} />
      </span>
    </div>
  )
}

/**
 * Jump back in — the session resume surface: recent searches rerun, recent
 * files reopen in Browse, saved queries rerun. Real localStorage state,
 * ordered by recency; empty until the session has history.
 */
function ResumeList({
  recents,
  saved,
  recentFiles,
  onRunQuery,
  onOpenFile,
}: {
  recents: string[]
  saved: string[]
  recentFiles: { path: string }[]
  onRunQuery: (q: string) => void
  onOpenFile: (path: string) => void
}) {
  const rows: { kind: string; text: string; icon: React.ReactNode; run: () => void }[] = [
    ...recentFiles.slice(0, 4).map((f) => ({
      kind: 'file',
      text: f.path,
      icon: <IconFile size={12} />,
      run: () => onOpenFile(f.path),
    })),
    ...saved.slice(0, 3).map((q) => ({
      kind: 'saved',
      text: q,
      icon: <IconStar size={12} filled />,
      run: () => onRunQuery(q),
    })),
    ...recents.slice(0, 5).map((q) => ({
      kind: 'query',
      text: q,
      icon: <IconHistoryClock size={12} />,
      run: () => onRunQuery(q),
    })),
  ]
  if (rows.length === 0) {
    return <p {...stylex.props(styles.resumeEmpty)}>no history yet — run a query or open a file</p>
  }
  return (
    <div {...stylex.props(styles.resumeList)}>
      {rows.map((r, i) => (
        <button key={`${r.kind}:${r.text}:${i}`} type="button" onClick={r.run} {...stylex.props(styles.resumeRow)}>
          <span {...stylex.props(styles.resumeIcon)}>{r.icon}</span>
          <span {...stylex.props(styles.resumeText)} title={r.text}>
            {r.text}
          </span>
          <span {...stylex.props(styles.resumeKind)}>{r.kind}</span>
        </button>
      ))}
    </div>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },

  // ── Ticker ──
  ticker: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 12,
    paddingRight: 12,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    backgroundColor: vars.bgSunken,
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorMuted,
    fontVariantNumeric: 'tabular-nums',
  },
  tickerDot: {
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },
  tickerSnap: {
    color: vars.colorBase,
    backgroundColor: vars.bgRaised,
    borderRadius: 4,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
  },
  tickerSep: {
    color: vars.colorFaint,
  },
  tickerDegraded: {
    color: vars.scaleHigh,
  },

  // ── Hero ──
  hero: {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    gap: 10,
    paddingTop: 30,
    paddingBottom: 26,
    textAlign: 'center',
  },
  heroEyebrow: {
    fontSize: 10.5,
    color: vars.colorFaint,
    letterSpacing: '0.14em',
    textTransform: 'uppercase',
  },
  heroTitle: {
    margin: 0,
    fontFamily: font.mono,
    fontSize: 21,
    fontWeight: 600,
    letterSpacing: '-0.02em',
    color: vars.colorBase,
    maxWidth: 640,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  heroForm: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    width: '100%',
    maxWidth: 620,
    marginTop: 10,
    paddingTop: 11,
    paddingBottom: 11,
    paddingLeft: 14,
    paddingRight: 12,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: {
      default: vars.borderMute,
      ':focus-within': vars.primary500,
    },
    backgroundColor: vars.bgRaised,
    color: vars.colorFaint,
    boxShadow: {
      default: 'none',
      ':focus-within': `0 0 0 3px ${vars.ringPrimary}`,
    },
  },
  heroInput: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    padding: 0,
    backgroundColor: 'transparent',
    color: vars.colorBase,
    fontSize: 14.5,
    fontFamily: 'inherit',
    outline: 'none',
    '::placeholder': {
      color: vars.colorFaint,
    },
  },
  chipRows: {
    display: 'flex',
    flexDirection: 'column',
    gap: 7,
    marginTop: 8,
    maxWidth: 620,
    width: '100%',
  },
  chipRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    minWidth: 0,
  },
  chipLabel: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    width: 56,
    flexShrink: 0,
    fontSize: 10,
    color: vars.colorFaint,
    letterSpacing: '0.08em',
    textTransform: 'uppercase',
    textAlign: 'left',
  },
  chipGroup: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
    minWidth: 0,
  },
  chip: {
    display: 'inline-flex',
    alignItems: 'center',
    maxWidth: 220,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 10,
    paddingRight: 10,
    borderRadius: 999,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    backgroundColor: {
      default: vars.bgSunken,
      ':hover': vars.bgHover,
    },
    color: vars.colorMuted,
    fontFamily: font.mono,
    fontSize: 11,
    whiteSpace: 'nowrap',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    cursor: 'pointer',
  },

  // ── Observatory bands — hairline-separated, borderless ──
  band: {
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    paddingTop: 14,
    paddingBottom: 18,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  bandHead: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
  },
  bandLabel: {
    fontFamily: font.mono,
    fontSize: 11,
    fontWeight: 700,
    textTransform: 'uppercase',
    letterSpacing: '0.1em',
    color: vars.colorBase,
  },
  bandMeta: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorFaint,
    fontVariantNumeric: 'tabular-nums',
  },
  bandRow: {
    display: 'flex',
    alignItems: 'stretch',
    gap: 18,
    flexWrap: { default: 'nowrap', '@media (max-width: 980px)': 'wrap' },
  },
  bandCol: {
    flexGrow: 0,
    flexShrink: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  colSep: {
    width: 1,
    alignSelf: 'stretch',
    backgroundColor: vars.borderMute,
    flexShrink: 0,
    display: { default: 'block', '@media (max-width: 980px)': 'none' },
  },
  bandSub: {
    display: 'flex',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    gap: 8,
  },
  bandSubLabel: {
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    color: vars.colorMuted,
  },
  bandSubAside: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
  },
  bandLink: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorActive,
    textDecoration: 'none',
  },

  // ── Query examples — rows of runnable syntax ──
  exampleList: {
    display: 'flex',
    flexDirection: 'column',
  },
  exampleRow: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 0,
    paddingRight: 0,
    borderWidth: 0,
    borderStyle: 'solid',
    borderColor: 'transparent',
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopColor: vars.borderMute,
    backgroundColor: { default: 'transparent', ':hover': vars.bgHover },
    fontFamily: 'inherit',
    fontSize: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    color: vars.colorBase,
  },
  exampleQuery: {
    fontFamily: font.mono,
    fontSize: 12,
    color: vars.colorBase,
    flexShrink: 0,
    minWidth: 200,
  },
  examplePurpose: {
    flex: 1,
    minWidth: 0,
    fontSize: 11.5,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  exampleHint: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  qtok_filter: { color: vars.accentInfo },
  qtok_op: { color: vars.colorActive, fontWeight: 600 },
  qtok_str: { color: vars.scaleLow },
  qtok_paren: { color: vars.colorActive },
  exampleGroup: {
    display: 'flex',
    flexDirection: 'column',
    gap: 0,
    marginTop: { default: 0, ':first-child': 0 },
  },
  exampleGroupLabel: {
    fontFamily: font.mono,
    fontSize: 9,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    letterSpacing: '0.08em',
    paddingTop: 8,
    paddingBottom: 2,
  },

  // ── Resume — session rows ──
  resumeList: {
    display: 'flex',
    flexDirection: 'column',
  },
  resumeRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 9,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 0,
    paddingRight: 0,
    borderWidth: 0,
    borderStyle: 'solid',
    borderColor: 'transparent',
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopColor: vars.borderMute,
    backgroundColor: { default: 'transparent', ':hover': vars.bgHover },
    fontFamily: 'inherit',
    fontSize: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
    color: vars.colorBase,
  },
  resumeIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  resumeText: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 11.5,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  resumeKind: {
    fontFamily: font.mono,
    fontSize: 9,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    flexShrink: 0,
  },
  resumeEmpty: {
    fontSize: 11.5,
    color: vars.colorFaint,
    paddingTop: 8,
    paddingBottom: 8,
  },
  stateNote: {
    margin: '2px 0 0',
    fontSize: 11,
    color: vars.colorMuted,
  },
  stateDot: {
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },

  freshGrid: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
  },

  // ── Freshness strip ──
  freshCell: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 9,
    paddingRight: 9,
    borderRadius: 7,
    backgroundColor: vars.bgSunken,
    flexShrink: 0,
  },
  freshName: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    whiteSpace: 'nowrap',
  },
  freshTrack: {
    width: 34,
    height: 3,
    borderRadius: 2,
    backgroundColor: vars.borderMute,
    overflow: 'hidden',
    flexShrink: 0,
    display: 'block',
  },
  freshFill: {
    display: 'block',
    height: '100%',
    borderRadius: 2,
  },
  freshAge: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    fontVariantNumeric: 'tabular-nums',
    whiteSpace: 'nowrap',
  },
  freshMuted: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorFaint,
  },
  freshProviders: {
    marginLeft: 'auto',
    display: 'flex',
    alignItems: 'center',
    gap: 14,
    flexWrap: 'wrap',
  },
  freshProvider: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorMuted,
    whiteSpace: 'nowrap',
  },
  freshDim: {
    color: vars.colorFaint,
    fontSize: 9.5,
  },
})
