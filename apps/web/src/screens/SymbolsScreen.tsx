import * as stylex from '@stylexjs/stylex'
import { Fragment, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import {
  api,
  type DefinitionHit,
  type ImpactedEntity,
  type ImpactReport,
  type ReferenceHit,
} from '../api'
import { FacetRail } from '../components/FacetRail'
import { emptyFilters, toggleFilter, type ActiveFilters, type FacetGroup } from '../lib/facets'
import { fileUrl, queryUrl } from '../lib/navigation'
import { MOCK_SYMBOLS, type MockSymbolEntry } from '../mock/symbols'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { FormSearchField } from '../ui/FormSearchField'
import { IconArrowUpRight, IconCaretDown } from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Symbols — a source-wide symbol index (Sourcegraph's symbol sidebar as a
 * screen). Rows expand inline to a `REFERENCES` panel — the precise-nav
 * "find references" idiom: the panel is LIVE — call-graph edges via
 * `/v1/refs/<name>` and the definition site via `/v1/def/<name>` deep-link
 * into Browse; the footer carries a copy of the qualified name,
 * and a `find all references` action that re-enters through `type:symbol`.
 * Below it an `IMPACT` section walks the two-hop blast radius via
 * `/v1/impact/<name>` — same `live` badge, engine caveats included.
 * The trailing hover ↗ keeps the definition deep link one click away —
 * the same row-expands/button-navigates split as Commits/BatchChanges.
 *
 * MOCK: `/v1/symbols` doesn't exist yet, so the LIST comes from
 * `src/mock/symbols.ts` and the screen carries a `preview` badge — but the
 * expanded REFERENCES/IMPACT panels and defined-at are live call-graph
 * data, marked `live` in the panel header.
 */
export function SymbolsScreen({
  onOpenFile,
}: {
  /** Deep link into Browse — trailing ↗ opens the definition, ref sites
   *  open their own `path:line` (via `useOpenFile` in the shell). */
  onOpenFile: (path: string, line?: number) => void
}) {
  const [text, setText] = useState('')
  const [filters, setFilters] = useState<ActiveFilters>(emptyFilters())
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set())

  const groups: FacetGroup[] = useMemo(() => {
    const count = (pick: (s: MockSymbolEntry) => string) => {
      const m = new Map<string, number>()
      for (const s of MOCK_SYMBOLS) m.set(pick(s), (m.get(pick(s)) ?? 0) + 1)
      return [...m.entries()]
        .map(([value, n]) => ({ value, label: value, count: n }))
        .sort((a, b) => b.count - a.count || a.value.localeCompare(b.value))
    }
    return [
      { id: 'kind', label: 'Kind', options: count((s) => s.kind) },
      { id: 'language', label: 'Language', options: count((s) => s.language) },
    ]
  }, [])

  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return MOCK_SYMBOLS.filter((s) => {
      if (needle && !`${s.name} ${s.qualifiedName} ${s.path}`.toLowerCase().includes(needle)) {
        return false
      }
      return Object.entries(filters).every(([group, values]) => {
        const v = group === 'kind' ? s.kind : group === 'language' ? s.language : undefined
        return v !== undefined && values.has(v)
      })
    })
  }, [text, filters])

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Symbols</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>{rows.length === MOCK_SYMBOLS.length ? `${rows.length} symbols` : `${rows.length} of ${MOCK_SYMBOLS.length} symbols`}</span>
        <span {...stylex.props(styles.spacer)} />
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="name, path, or qualifier"
            onClear={() => setText('')}
            aria-label="Filter symbols"
          />
        </div>
      </header>

      <div {...stylex.props(styles.body)}>
        <FacetRail
          groups={groups}
          active={filters}
          onToggle={(g, v) => setFilters((f) => toggleFilter(f, g, v))}
        />
        <div {...stylex.props(styles.tableCol)}>
          <div {...stylex.props(styles.tableWrap)}>
            <table {...stylex.props(styles.table)}>
              <thead>
                <tr>
                  <th {...stylex.props(styles.th)}>Symbol</th>
                  <th style={{ width: 96 }} {...stylex.props(styles.th)}>Kind</th>
                  <th {...stylex.props(styles.th)}>Location</th>
                  <th style={{ width: 60, textAlign: 'end' }} {...stylex.props(styles.th)}>Refs</th>
                  <th style={{ width: 64, textAlign: 'end' }} {...stylex.props(styles.th)}>Conf</th>
                  <th style={{ width: 30 }} {...stylex.props(styles.th)} />
                </tr>
              </thead>
              <tbody>
                {rows.map((s) => (
                  <SymbolRow
                    key={s.qualifiedName}
                    symbol={s}
                    open={expanded.has(s.qualifiedName)}
                    onToggle={() => toggle(s.qualifiedName)}
                    onOpenFile={onOpenFile}
                  />
                ))}
              </tbody>
            </table>
          </div>
          {rows.length === 0 && (
            <p {...stylex.props(styles.empty)}>no symbols match the filter</p>
          )}
          <p {...stylex.props(styles.note)}>
            List is fixture data — the /v1/symbols endpoint is pending. Expanded
            REFERENCES, defined-at and IMPACT are live: /v1/refs, /v1/def and
            /v1/impact query the engine call graph, marked 'live' in the panel.
          </p>
        </div>
      </div>
    </div>
  )
}

/**
 * One symbol: summary row expands inline to the references panel; the
 * trailing hover-revealed ↗ keeps the `/browse` deep link one click away
 * (same split as `BatchChangesScreen`'s changeset rows — which also carry
 * the `tr`+Fragment/`colSpan` expand pattern used here).
 */
function SymbolRow({
  symbol: s,
  open,
  onToggle,
  onOpenFile,
}: {
  symbol: MockSymbolEntry
  open: boolean
  onToggle: () => void
  onOpenFile: (path: string, line?: number) => void
}) {
  // StyleX has no parent-hover/child selectors — reveal the trailing action
  // from row hover in state (same approach as BranchesScreen).
  const [hover, setHover] = useState(false)
  return (
    <Fragment>
      <tr
        tabIndex={0}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onToggle()
          }
        }}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        aria-expanded={open}
        {...stylex.props(styles.tr)}
      >
        <td {...stylex.props(styles.td)}>
          <span {...stylex.props(styles.symCell)}>
            <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
              <IconCaretDown size={10} />
            </span>
            <span {...stylex.props(styles.sym)}>{s.name}</span>
          </span>
        </td>
        <td {...stylex.props(styles.td)}>
          <DisplayBadge text={s.kind}>{s.kind}</DisplayBadge>
        </td>
        <td {...stylex.props(styles.td)}>
          <DisplayFilePath path={s.path} line={s.line} />
        </td>
        <td style={{ textAlign: 'end' }} {...stylex.props(styles.td)}>
          <span {...stylex.props(styles.num)}>{s.refCount}</span>
        </td>
        <td style={{ textAlign: 'end' }} {...stylex.props(styles.td)}>
          <span {...stylex.props(styles.num, s.confidence < 0.8 && styles.lowConf)}>
            {(s.confidence * 100).toFixed(0)}%
          </span>
        </td>
        <td {...stylex.props(styles.td, styles.tdAction)}>
          <span
            {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}
            onClick={(e) => e.stopPropagation()}
          >
            <ActionIconButton
              compact
              icon={<IconArrowUpRight size={10} />}
              tooltip={`open ${s.path}`}
              label={`Open ${s.path}`}
              onClick={() => onOpenFile(s.path, s.line)}
            />
          </span>
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={6} {...stylex.props(styles.tdDetail)}>
            <ReferencesLive s={s} onOpenFile={onOpenFile} />
            <ImpactLive s={s} />
          </td>
        </tr>
      )}
    </Fragment>
  )
}

/**
 * Live references panel — engine call-graph edges via `/v1/refs/<name>`
 * (`via` is the edge kind, `confidence` the resolver's score, `evidence`
 * the call-site address) and the definition site via `/v1/def/<name>`.
 * Fetches lazily on first expand; symbols the graph doesn't know get an
 * honest empty state, never the fixture sample.
 */
function ReferencesLive({
  s,
  onOpenFile,
}: {
  s: MockSymbolEntry
  onOpenFile: (path: string, line?: number) => void
}) {
  const navigate = useNavigate()
  const [refs, setRefs] = useState<ReferenceHit[] | null>(null)
  const [defs, setDefs] = useState<DefinitionHit[] | null>(null)
  const [err, setErr] = useState<string>()

  useEffect(() => {
    let dead = false
    // A busy daemon (index lease held) hangs the call rather than failing —
    // race a timeout so the panel reports instead of spinning forever.
    const timeout = new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error('timed out — daemon busy')), 20_000),
    )
    Promise.allSettled([
      Promise.race([api.references(s.name), timeout]),
      Promise.race([api.definitions(s.name), timeout]),
    ]).then(([r, d]) => {
      if (dead) return
      if (r.status === 'fulfilled') setRefs(r.value.references)
      else setErr(r.reason instanceof Error ? r.reason.message : 'references failed')
      if (d.status === 'fulfilled') setDefs(d.value.definitions)
    })
    return () => {
      dead = true
    }
  }, [s.name])

  /** Real definition site when the graph knows one; fixture site otherwise. */
  const def = defs?.[0]
  const defPath = def?.address?.path ?? s.path
  const defLine = def?.address?.startLine ?? s.line

  return (
    <div {...stylex.props(styles.detail)}>
      <div {...stylex.props(styles.refHead)}>
        REFERENCES
        <DisplayBadge severity="low" title="live — engine call graph, not the fixture">
          live
        </DisplayBadge>
        {refs && (
          <span {...stylex.props(styles.refNum)}>
            {refs.length} edge{refs.length === 1 ? '' : 's'}
          </span>
        )}
      </div>
      {err != null && <div {...stylex.props(styles.refEmpty)}>references failed — {err}</div>}
      {refs == null && err == null && (
        <div {...stylex.props(styles.refEmpty)}>loading call graph…</div>
      )}
      {refs?.map((r, i) => {
        const site = r.evidence[0]
        return (
          <button
            key={`${r.fromQualifiedName ?? r.fromName}:${i}`}
            type="button"
            title={site ? `open ${site.path}:${site.startLine}` : r.fromQualifiedName}
            onClick={() => site && onOpenFile(site.path, site.startLine)}
            {...stylex.props(styles.refRow)}
          >
            {site && <DisplayFilePath path={site.path} line={site.startLine} />}
            <span {...stylex.props(styles.refSnippet)}>
              {r.fromQualifiedName ?? r.fromName}
            </span>
            <span {...stylex.props(styles.refVia)}>
              {r.via} · {Math.round(r.confidence * 100)}%
            </span>
          </button>
        )
      })}
      {refs?.length === 0 && (
        <div {...stylex.props(styles.refEmpty)}>
          no call-graph references for '{s.name}' — the row's count is a fixture
          number, this name isn't in the engine graph
        </div>
      )}
      <div {...stylex.props(styles.refFoot)}>
        <span {...stylex.props(styles.meta)}>
          {def ? 'defined at' : 'declared at'}{' '}
          <button
            type="button"
            onClick={() => onOpenFile(defPath, defLine)}
            {...stylex.props(styles.metaLink)}
          >
            {defPath}:{defLine}
          </button>
          {!def && defs != null && (
            <span {...stylex.props(styles.refSample)}> · fixture site</span>
          )}
        </span>
        <CopyButton text={s.qualifiedName} title={`copy ${s.qualifiedName}`} />
        <span {...stylex.props(styles.spacer)} />
        <ActionButton size="sm" onClick={() => navigate(queryUrl(`type:symbol ${s.name}`))}>
          find all references
        </ActionButton>
      </div>
    </div>
  )
}

/** Rows shown per hop group before a `+N more` overflow line. */
const IMPACT_ROW_CAP = 12

/**
 * Live impact panel — the two-hop blast radius via `/v1/impact/<name>`:
 * entities the engine's call graph says a change here would touch,
 * grouped by hop distance with the resolver's `via` edge kind and
 * confidence, deep-linking into Browse when the graph records a source
 * path. Fetches lazily on first expand (same pattern as ReferencesLive);
 * an empty `impacted` set means the symbol resolves outside the call
 * graph — an honest empty line, never an error. Engine `caveats` always
 * render: they're real analysis limitations, not decoration.
 */
function ImpactLive({ s }: { s: MockSymbolEntry }) {
  const navigate = useNavigate()
  const [report, setReport] = useState<ImpactReport | null>(null)
  const [err, setErr] = useState<string>()

  useEffect(() => {
    let dead = false
    // Same busy-daemon guard as ReferencesLive — the call can hang while
    // the index lease is held, so race a timeout and report it.
    const timeout = new Promise<never>((_, rej) =>
      setTimeout(() => rej(new Error('timed out — daemon busy')), 20_000),
    )
    Promise.race([api.impact(s.name), timeout])
      .then((r) => {
        if (!dead) setReport(r)
      })
      .catch((e: unknown) => {
        if (!dead) setErr(e instanceof Error ? e.message : 'request failed')
      })
    return () => {
      dead = true
    }
  }, [s.name])

  const hopGroups = useMemo((): [number, ImpactedEntity[]][] => {
    if (!report) return []
    const byHops = new Map<number, ImpactedEntity[]>()
    for (const e of report.impacted) {
      const list = byHops.get(e.hops)
      if (list) list.push(e)
      else byHops.set(e.hops, [e])
    }
    return [...byHops.entries()].sort((a, b) => a[0] - b[0])
  }, [report])

  const n = report?.impacted.length ?? 0

  return (
    <div {...stylex.props(styles.detail, styles.impactDetail)}>
      <div {...stylex.props(styles.refHead)}>
        IMPACT
        <DisplayBadge severity="low" title="live — engine call graph, not the fixture">
          live
        </DisplayBadge>
        {report && (
          <span {...stylex.props(styles.refNum)}>
            {n} {n === 1 ? 'entity' : 'entities'} within 2 hops
          </span>
        )}
      </div>
      {report != null && report.matchedEntities.length > 1 && (
        <div {...stylex.props(styles.refEmpty)}>
          resolved to {report.matchedEntities.length} entities
        </div>
      )}
      {err != null && <div {...stylex.props(styles.refEmpty)}>impact failed — {err}</div>}
      {report == null && err == null && (
        <div {...stylex.props(styles.refEmpty)}>loading impact…</div>
      )}
      {report != null && report.impacted.length === 0 && (
        <div {...stylex.props(styles.refEmpty)}>
          no impact edges — '{s.name}' resolves outside the call graph
        </div>
      )}
      {hopGroups.map(([hops, list]) => (
        <Fragment key={hops}>
          <div {...stylex.props(styles.impactGroupLabel)}>
            {hops} hop{hops === 1 ? '' : 's'}
          </div>
          {list.slice(0, IMPACT_ROW_CAP).map((e) => {
            const p = e.path
            const body = (
              <>
                <span {...stylex.props(styles.impactName)}>{e.name}</span>
                <span {...stylex.props(styles.impactKind)}>
                  <DisplayBadge text={e.kind} color={false} />
                </span>
                <span {...stylex.props(styles.refVia)}>
                  via {e.via} · {Math.round(e.confidence * 100)}%
                </span>
                {p && (
                  <span {...stylex.props(styles.impactPath)}>
                    <DisplayFilePath path={p} />
                  </span>
                )}
              </>
            )
            return p ? (
              <button
                key={`${hops}:${e.name}`}
                type="button"
                title={`open ${p}`}
                onClick={() => navigate(fileUrl(p))}
                {...stylex.props(styles.impactRow, styles.impactRowBtn)}
              >
                {body}
              </button>
            ) : (
              <div
                key={`${hops}:${e.name}`}
                title="no source path recorded"
                {...stylex.props(styles.impactRow)}
              >
                {body}
              </div>
            )
          })}
          {list.length > IMPACT_ROW_CAP && (
            <div {...stylex.props(styles.impactMore)}>
              +{list.length - IMPACT_ROW_CAP} more
            </div>
          )}
        </Fragment>
      ))}
      {report != null && (
        <div {...stylex.props(styles.impactFoot)}>
          <span {...stylex.props(styles.meta)}>
            {report.edgeKinds.length > 0
              ? `via ${report.edgeKinds.join(', ')} — ${report.provenance}`
              : report.provenance}
          </span>
          {report.caveats.map((c, i) => (
            <span key={i} {...stylex.props(styles.meta)}>
              {c}
            </span>
          ))}
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
  head: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  title: {
    margin: 0,
    fontSize: 15,
    fontWeight: 600,
  },
  count: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  spacer: {
    flex: 1,
  },
  search: {
    width: 260,
  },
  body: {
    display: 'flex',
    gap: 16,
    alignItems: 'flex-start',
  },
  tableCol: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  // In-file table — LayoutDataTable can't carry a per-row expansion, so the
  // chrome is replicated from it verbatim (same hairlines/density) with the
  // BatchChanges `tr`+Fragment/`colSpan` expand grafted on.
  tableWrap: {
    overflowX: 'auto',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
  },
  table: {
    width: '100%',
    borderCollapse: 'collapse',
    fontSize: 12,
  },
  th: {
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorFaint,
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
    whiteSpace: 'nowrap',
    textAlign: 'left',
  },
  tr: {
    cursor: 'pointer',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    outline: 'none',
    boxShadow: {
      ':focus-visible': `inset 0 0 0 2px ${vars.ringPrimary}`,
    },
  },
  td: {
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    color: vars.colorBase,
    whiteSpace: 'nowrap',
  },
  tdAction: {
    paddingLeft: 0,
    paddingRight: 4,
  },
  tdDetail: {
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 0,
    paddingRight: 0,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  symCell: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
  },
  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
  // Trailing hover action — space reserved so columns never shift;
  // `:focus-within` keeps it reachable by keyboard.
  rowActions: {
    display: 'inline-flex',
    alignItems: 'center',
    opacity: { default: 0, ':focus-within': 1 },
    pointerEvents: { default: 'none', ':focus-within': 'auto' },
    transitionProperty: 'opacity',
    transitionDuration: '120ms',
  },
  rowActionsShown: {
    opacity: 1,
    pointerEvents: 'auto',
  },
  sym: {
    fontFamily: font.mono,
    fontSize: 12,
    fontWeight: 600,
  },
  num: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
  },
  lowConf: {
    color: vars.scaleMedium,
  },
  // Inline-expanded references panel — same sunken inset detail as
  // CommitsScreen's changed-file list (caret-width left indent).
  detail: {
    display: 'flex',
    flexDirection: 'column',
    backgroundColor: vars.bgSunken,
    paddingTop: 2,
    paddingBottom: 6,
    paddingLeft: 32,
    paddingRight: 12,
  },
  refHead: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 6,
    fontSize: 10,
    fontWeight: 600,
    letterSpacing: '0.06em',
    color: vars.colorFaint,
    paddingTop: 5,
    paddingBottom: 3,
    paddingLeft: 4,
  },
  refNum: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    color: vars.colorMuted,
  },
  refSample: {
    fontWeight: 400,
    letterSpacing: 'normal',
    opacity: vars.opFade,
  },
  refRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 12,
    minWidth: 0,
    borderLeftWidth: 0,
    borderRightWidth: 0,
    borderBottomWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 11,
    color: vars.colorBase,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
    textAlign: 'left',
    borderTopWidth: {
      default: 1,
      ':first-of-type': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  refSnippet: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    textAlign: 'left',
  },
  refEmpty: {
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    fontSize: 11,
    color: vars.colorFaint,
  },
  refFoot: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    paddingTop: 6,
    paddingLeft: 4,
  },
  meta: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  metaLink: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorActive,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    borderWidth: 0,
    borderRadius: 3,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 4,
    paddingRight: 4,
    cursor: 'pointer',
  },
  refVia: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    whiteSpace: 'nowrap',
  },
  // Inline-expanded impact panel — a second sunken detail block below
  // REFERENCES, split by a hairline. Rows reuse the refRow layout idiom;
  // entities the graph records no path for render as static divs (no
  // hover/cursor affordance) instead of dead buttons.
  impactDetail: {
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  impactGroupLabel: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 5,
    paddingBottom: 3,
    paddingLeft: 4,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  impactRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 12,
    minWidth: 0,
    fontSize: 11,
    color: vars.colorBase,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 4,
    textAlign: 'left',
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  impactRowBtn: {
    borderLeftWidth: 0,
    borderRightWidth: 0,
    borderBottomWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    cursor: 'pointer',
  },
  impactName: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorActive,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    minWidth: 0,
  },
  impactKind: {
    flexShrink: 0,
  },
  impactPath: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textAlign: 'end',
  },
  impactMore: {
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    fontSize: 11,
    color: vars.colorFaint,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  impactFoot: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    paddingTop: 6,
    paddingBottom: 2,
    paddingLeft: 4,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  empty: {
    margin: 0,
    paddingTop: 18,
    paddingBottom: 18,
    fontSize: 12,
    color: vars.colorFaint,
    textAlign: 'center',
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
