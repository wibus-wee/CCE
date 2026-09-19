import * as stylex from '@stylexjs/stylex'
import { useEffect, useState } from 'react'
import { Link } from 'react-router'
import {
  api,
  apiBase,
  describeError,
  discover,
  gateway,
  type HealthReport,
  type ProviderReport,
  type ServiceMode,
  type ViewManifest,
  type ViewState,
} from '../api'
import { version as WEB_VERSION } from '../../package.json'
import { useAppStore } from '../lib/store'
import { IconArrowUpRight, IconWarning } from '../ui/icons'
import { LogoMark } from '../ui/LogoMark'
import { font, vars } from '../ui/tokens.stylex'

/**
 * About — REAL, not fixture. The only screen where every value is probed
 * live on mount: /healthz for the serving binary's version, /v1/status for
 * repository + snapshot + view freshness, /v1/providers for provider state,
 * /v1/files for the indexed path count, and the /v1/repos probe for the
 * service mode. Each probe fails independently and reports inline — the
 * contract's "unavailable is reported, never faked" applies here too.
 *
 * The capabilities block is the one static part: it states the runtime's
 * architectural contract (what CCE is), not measured state.
 */
export function AboutScreen() {
  const setShortcutsOpen = useAppStore((s) => s.setShortcutsOpen)
  const [mode, setMode] = useState<ServiceMode>()
  const [repos, setRepos] = useState<number>()
  const [health, setHealth] = useState<HealthReport>()
  const [healthErr, setHealthErr] = useState<string>()
  const [manifest, setManifest] = useState<ViewManifest>()
  const [statusErr, setStatusErr] = useState<string>()
  const [providers, setProviders] = useState<ProviderReport[]>()
  const [providersErr, setProvidersErr] = useState<string>()
  const [fileCount, setFileCount] = useState<number>()

  useEffect(() => {
    let dead = false
    const ok = <T,>(set: (v: T) => void) => (v: T) => {
      if (!dead) set(v)
    }
    const err = (set: (v: string) => void) => (e: unknown) => {
      if (!dead) set(describeError(e))
    }
    // discover() never rejects — an unreachable registry reads as 'daemon'.
    void discover().then((m) => {
      ok(setMode)(m)
      if (m === 'gateway') void gateway.repos().then((r) => ok(setRepos)(r.length), () => {})
    })
    void api.health().then(ok(setHealth), err(setHealthErr))
    void api.status().then(ok(setManifest), err(setStatusErr))
    void api.providers().then(ok(setProviders), err(setProvidersErr))
    void api.files().then((r) => ok(setFileCount)(r.files.length), () => {})
    return () => {
      dead = true
    }
  }, [])

  const base = apiBase()
  const views = manifest ? Object.entries(manifest.views) : undefined
  const indexed = views
    ?.map(([, v]) => v.updatedAt)
    .sort()
    .at(-1)

  return (
    <div {...stylex.props(styles.root)}>
      <span {...stylex.props(styles.watermark)} aria-hidden>
        <LogoMark size={420} />
      </span>

      <div {...stylex.props(styles.column)}>
        <header {...stylex.props(styles.hero)}>
          <span {...stylex.props(styles.mark)}>
            <LogoMark size={54} />
          </span>
          <h1 {...stylex.props(styles.name)}>CCE</h1>
          <div {...stylex.props(styles.tagline)}>repository intelligence</div>
          <div {...stylex.props(styles.meta)}>
            <span>v{WEB_VERSION}</span>
            <Dot />
            {health ? (
              <span>runtime v{health.version}</span>
            ) : healthErr ? (
              <span {...stylex.props(styles.err)}>runtime unreachable</span>
            ) : (
              <span {...stylex.props(styles.dim)}>runtime —</span>
            )}
            <Dot />
            <span {...stylex.props(styles.live)}>
              <span {...stylex.props(styles.liveDot)} />
              live
            </span>
            <Dot />
            <span {...stylex.props(styles.dim)}>
              {mode ? (mode === 'gateway' ? 'gateway' : 'standalone daemon') : '…'}
            </span>
          </div>
          <p {...stylex.props(styles.blurb)}>
            Local-first, agent-agnostic repository intelligence. Every indexed
            commit is materialized into views — lexical, dense, symbols, graph,
            history — and every result carries its canonical source address,
            snapshot, freshness, route and score. Deterministic facts,
            framework-derived relations and model inference stay
            distinguishable; stale or unavailable views are reported, never
            silently faked.
          </p>
        </header>

        <section {...stylex.props(styles.band)}>
          <h3 {...stylex.props(styles.bandLabel)}>Service</h3>
          <div {...stylex.props(styles.fields)}>
            <Field
              label="mode"
              value={
                mode
                  ? mode === 'gateway'
                    ? `gateway · ${repos != null ? `${repos} repo${repos === 1 ? '' : 's'}` : '…'}`
                    : 'daemon · standalone'
                  : undefined
              }
            />
            <Field
              label="runtime"
              value={
                health ? (
                  <>
                    v{health.version} <span {...stylex.props(styles.ok)}>{health.status}</span>
                  </>
                ) : (
                  <ErrOr err={healthErr} />
                )
              }
            />
            <Field label="web" value={`v${WEB_VERSION}`} />
            <Field
              label="endpoint"
              value={
                <code {...stylex.props(styles.mono)}>
                  {window.location.host}
                  {base || '/'}
                </code>
              }
            />
          </div>
        </section>

        <section {...stylex.props(styles.band)}>
          <h3 {...stylex.props(styles.bandLabel)}>Repository</h3>
          <div {...stylex.props(styles.fields)}>
            <Field
              label="repository"
              value={
                manifest ? (
                  <code {...stylex.props(styles.mono)}>{manifest.repositoryId}</code>
                ) : (
                  <ErrOr err={statusErr} />
                )
              }
              wide
            />
            <Field
              label="snapshot"
              value={
                manifest ? (
                  <>
                    <code {...stylex.props(styles.mono)}>{manifest.snapshotId.slice(0, 16)}</code>
                    {indexed && <span {...stylex.props(styles.dim)}>{relTime(indexed)}</span>}
                  </>
                ) : (
                  <ErrOr err={statusErr} />
                )
              }
            />
            <Field
              label="files"
              value={fileCount != null ? `${fileCount.toLocaleString()} paths` : <Pending />}
            />
            <Field
              label="views"
              value={
                views ? <ViewCounts entries={views} /> : <ErrOr err={statusErr} />
              }
              link="/index"
            />
            <Field
              label="providers"
              value={
                providers ? (
                  <ProviderCounts providers={providers} />
                ) : (
                  <ErrOr err={providersErr} />
                )
              }
              link="/index"
            />
          </div>
        </section>

        <section {...stylex.props(styles.band)}>
          <h3 {...stylex.props(styles.bandLabel)}>Capabilities</h3>
          <div {...stylex.props(styles.fields, styles.fieldsWide)}>
            <Field
              label="retrieval"
              value="exact-symbol · lexical · dense · structural · history · diff — routed per query"
              wide
            />
            <Field
              label="embeddings"
              value="local ONNX — fetched once, then offline · CCE_DENSE=disabled keeps lexical + structural"
              wide
            />
            <Field
              label="storage"
              value="SQLite FTS metadata · content-addressed artifact store"
              wide
            />
            <Field
              label="integrations"
              value="thin adapters over the engine — markdown · MCP · CLI"
              wide
            />
          </div>
        </section>

        <footer {...stylex.props(styles.foot)}>
          <span>apache-2.0</span>
          <Dot />
          <span>all state local — nothing leaves the machine</span>
          <span {...stylex.props(styles.footSpacer)} />
          <a
            {...stylex.props(styles.link)}
            href={`${base}/docs`}
            target="_blank"
            rel="noreferrer"
          >
            api reference <IconArrowUpRight size={10} />
          </a>
          <button
            type="button"
            {...stylex.props(styles.footBtn)}
            onClick={() => setShortcutsOpen(true)}
          >
            <span {...stylex.props(styles.kbdHint)}>?</span> shortcuts
          </button>
        </footer>
      </div>
    </div>
  )
}

/** Label-above-value spec field — the product spec-sheet pattern. */
function Field({
  label,
  value,
  wide,
  link,
}: {
  label: string
  value?: React.ReactNode
  wide?: boolean
  link?: string
}) {
  return (
    <div {...stylex.props(styles.field, wide && styles.fieldWide)}>
      <span {...stylex.props(styles.fieldLabel)}>
        {label}
        {link && (
          <Link {...stylex.props(styles.fieldLink)} to={link}>
            →
          </Link>
        )}
      </span>
      <span {...stylex.props(styles.fieldValue)}>{value ?? <Pending />}</span>
    </div>
  )
}

function Dot() {
  return <span {...stylex.props(styles.dot)}>·</span>
}

function Pending() {
  return <span {...stylex.props(styles.dim)}>—</span>
}

function ErrOr({ err }: { err?: string }) {
  return err ? (
    <span {...stylex.props(styles.err)}>
      <IconWarning size={11} /> {err}
    </span>
  ) : (
    <Pending />
  )
}

/** `5 ready · 1 stale` — non-ready states colored, ready stays quiet. */
function ViewCounts({ entries }: { entries: [string, { state: ViewState }][] }) {
  const order: ViewState[] = ['ready', 'building', 'partial', 'stale', 'unavailable', 'failed']
  const counts = new Map<ViewState, number>()
  for (const [, v] of entries) counts.set(v.state, (counts.get(v.state) ?? 0) + 1)
  return (
    <span {...stylex.props(styles.counts)}>
      {order
        .filter((s) => counts.get(s))
        .map((s) => (
          <span
            key={s}
            {...stylex.props(s === 'ready' ? styles.okText : s === 'failed' ? styles.bad : styles.warn)}
          >
            {counts.get(s)} {s.replaceAll('_', ' ')}
          </span>
        ))}
    </span>
  )
}

function ProviderCounts({ providers }: { providers: ProviderReport[] }) {
  const counts = new Map<ProviderReport['state'], number>()
  for (const p of providers) counts.set(p.state, (counts.get(p.state) ?? 0) + 1)
  const parts: [ProviderReport['state'], 'okText' | 'dim' | 'bad'][] = [
    ['ready', 'okText'],
    ['missing', 'dim'],
    ['not_applicable', 'dim'],
    ['failed', 'bad'],
  ]
  return (
    <span {...stylex.props(styles.counts)}>
      {parts
        .filter(([s]) => counts.get(s))
        .map(([s, tone]) => (
          <span key={s} {...stylex.props(styles[tone])}>
            {counts.get(s)} {s.replaceAll('_', ' ')}
          </span>
        ))}
    </span>
  )
}

function relTime(iso: string): string {
  const m = Math.round((Date.now() - new Date(iso).getTime()) / 60000)
  if (m < 1) return 'now'
  if (m < 60) return `${m}m ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.round(h / 24)}d ago`
}

const styles = stylex.create({
  root: {
    position: 'relative',
    height: '100%',
    overflowY: 'auto',
    overflowX: 'hidden',
  },
  // The product mark bleeding off the top-right edge at whisper opacity —
  // spatial depth without color washes or shaders.
  watermark: {
    position: 'absolute',
    top: -90,
    right: -120,
    color: vars.colorActive,
    opacity: 0.05,
    pointerEvents: 'none',
  },
  column: {
    position: 'relative',
    maxWidth: 700,
    marginLeft: 'auto',
    marginRight: 'auto',
    paddingLeft: 40,
    paddingRight: 40,
    paddingTop: 'clamp(56px, 11vh, 120px)',
    paddingBottom: 56,
    display: 'flex',
    flexDirection: 'column',
  },
  hero: {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'flex-start',
  },
  mark: {
    color: vars.colorActive,
    marginBottom: 20,
  },
  name: {
    margin: 0,
    fontSize: 26,
    fontWeight: 680,
    letterSpacing: '0.06em',
    color: vars.colorBase,
    lineHeight: 1,
  },
  tagline: {
    marginTop: 8,
    fontSize: 13,
    color: vars.colorMuted,
    letterSpacing: '0.01em',
  },
  meta: {
    marginTop: 14,
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    fontSize: 11.5,
    fontFamily: font.mono,
    color: vars.colorMuted,
    flexWrap: 'wrap',
  },
  dot: {
    color: vars.colorFaint,
  },
  live: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    color: vars.accentSuccess,
  },
  liveDot: {
    width: 5,
    height: 5,
    borderRadius: '50%',
    backgroundColor: vars.accentSuccess,
  },
  dim: {
    color: vars.colorFaint,
  },
  ok: {
    color: vars.accentSuccess,
    fontSize: 11,
  },
  okText: {
    color: vars.accentSuccess,
  },
  warn: {
    color: vars.accentWarning,
  },
  bad: {
    color: vars.accentError,
  },
  err: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    color: vars.accentError,
    fontSize: 11.5,
  },
  blurb: {
    margin: '26px 0 0',
    maxWidth: '54ch',
    fontSize: 13,
    lineHeight: 1.7,
    color: vars.colorMuted,
  },
  band: {
    marginTop: 44,
    paddingTop: 18,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  bandLabel: {
    margin: '0 0 16px',
    fontSize: 10,
    fontFamily: font.mono,
    fontWeight: 500,
    letterSpacing: '0.1em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
  },
  fields: {
    display: 'grid',
    gridTemplateColumns: 'repeat(3, 1fr)',
    gap: '20px 28px',
  },
  fieldsWide: {
    gridTemplateColumns: 'repeat(2, 1fr)',
  },
  field: {
    display: 'flex',
    flexDirection: 'column',
    gap: 5,
    minWidth: 0,
  },
  fieldWide: {
    gridColumn: 'span 2',
  },
  fieldLabel: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 10,
    fontFamily: font.mono,
    letterSpacing: '0.08em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
  },
  fieldLink: {
    color: vars.colorFaint,
    textDecoration: 'none',
    ':hover': {
      color: vars.colorActive,
    },
  },
  fieldValue: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 7,
    flexWrap: 'wrap',
    minWidth: 0,
    fontSize: 12.5,
    color: vars.colorBase,
    lineHeight: 1.45,
  },
  mono: {
    fontFamily: font.mono,
    fontSize: 11.5,
  },
  counts: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
  },
  link: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 3,
    color: vars.colorActive,
    fontSize: 11.5,
    textDecoration: 'none',
  },
  foot: {
    marginTop: 48,
    paddingTop: 16,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    fontSize: 11,
    color: vars.colorFaint,
  },
  footSpacer: {
    flex: 1,
  },
  footBtn: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    padding: 0,
    border: 'none',
    background: 'none',
    color: vars.colorFaint,
    fontSize: 11,
    fontFamily: 'inherit',
    cursor: 'pointer',
    ':hover': {
      color: vars.colorMuted,
    },
  },
  kbdHint: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    minWidth: 15,
    height: 15,
    paddingInline: 3,
    borderRadius: 3,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    fontSize: 10,
    fontFamily: font.mono,
    color: vars.colorMuted,
  },
})
