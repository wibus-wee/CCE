import { Funnel } from '@phosphor-icons/react'
import * as stylex from '@stylexjs/stylex'
import { useMemo, useState } from 'react'
import {
  MOCK_INSIGHT_DASHBOARDS,
  MOCK_INSIGHTS,
  type MockInsight,
  type MockInsightSeries,
} from '../mock/insights'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { ActionToggleGroup } from '../ui/ActionToggleGroup'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FormField } from '../ui/FormField'
import { FormTextInput } from '../ui/FormInputs'
import { FormSearchField } from '../ui/FormSearchField'
import { getHashColorFromString } from '../ui/format'
import {
  IconCaretDown,
  IconDownload,
  IconPlus,
  IconX,
} from '../ui/icons'
import { LayoutBreadcrumb, LayoutDataTable } from '../ui/LayoutStructure'
import { font, vars } from '../ui/tokens.stylex'
import { useColorScheme } from '../ui/useDark'

/**
 * Code Insights — Sourcegraph's trends-over-time surface: dashboards of
 * insights, each insight one or more named query series re-sampled per
 * interval and drawn as lines with a legend, a repo/time-window filter
 * row, and inline drill-down to the per-point data. Legend chips toggle
 * series visibility in both chart sizes; the expanded chart adds a hover
 * crosshair and CSV export of the visible series. MOCK: no
 * `/v1/insights` endpoint; fixtures come from `src/mock/insights.ts` and
 * the screen is labeled `preview` — never index truth. The create form
 * is a local draft: its preview is generated from a hash of the query,
 * and saving waits on the endpoint.
 */

const DAY_MS = 86400_000
const daysAgo = (d: number) => new Date(Date.now() - d * DAY_MS).toISOString().slice(0, 10)
const WINDOW_OPTIONS = [
  { value: '7', label: '7d' },
  { value: '30', label: '30d' },
  { value: '90', label: '90d' },
]

/** Keep points inside `[latest sample - window, latest sample]` per insight. */
function sliceToWindow(insight: MockInsight, windowDays: number): MockInsightSeries[] {
  const latest = Math.max(
    ...insight.series.flatMap((s) => s.points.map((p) => Date.parse(p.at))),
  )
  const cutoff = latest - windowDays * DAY_MS
  return insight.series.map((s) => ({
    ...s,
    points: s.points.filter((p) => Date.parse(p.at) >= cutoff),
  }))
}

/** djb2-style fold — a stable seed so the same query draws the same preview. */
function hashQuery(query: string): number {
  let h = 5381
  for (let i = 0; i < query.length; i++) h = ((h << 5) + h + query.charCodeAt(i)) | 0
  return h >>> 0
}

/**
 * Deterministic preview series for the create form — same shape recipe as
 * the fixture generator (base + linear drift + sinusoidal wobble), but
 * every parameter falls out of `hashQuery(query)` so the chart is honest
 * about being a generated shape, not a query result.
 */
function generatedPreviewSeries(query: string, name: string): MockInsightSeries {
  const h = hashQuery(query)
  const base = 15 + (h % 55)
  const drift = (((h >> 8) % 21) - 10) / 4
  const wobble = 0.06 + ((h >> 16) % 18) / 100
  const phase = (h % 628) / 100
  const count = 12
  const intervalDays = 7
  return {
    name,
    points: Array.from({ length: count }, (_, i) => ({
      at: daysAgo((count - 1 - i) * intervalDays),
      value: Math.max(
        0,
        Math.round(base + drift * i + Math.sin(i * 1.7 + phase) * base * wobble),
      ),
    })),
  }
}

export function InsightsScreen() {
  const dark = useColorScheme() === 'dark'
  const [dashId, setDashId] = useState(MOCK_INSIGHT_DASHBOARDS[0]?.id ?? '')
  const [windowDays, setWindowDays] = useState(90)
  const [text, setText] = useState('')
  const [expanded, setExpanded] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)

  const byId = useMemo(() => new Map(MOCK_INSIGHTS.map((i) => [i.id, i])), [])
  const dashboard =
    MOCK_INSIGHT_DASHBOARDS.find((d) => d.id === dashId) ?? MOCK_INSIGHT_DASHBOARDS[0]

  const insights = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return (dashboard?.insights ?? [])
      .map((id) => byId.get(id))
      .filter((i): i is MockInsight => i !== undefined)
      .filter((i) => !needle || `${i.title} ${i.query}`.toLowerCase().includes(needle))
  }, [dashboard, byId, text])

  /** Repo scope across the dashboard's insights — the fixture's filter chip. */
  const scope = useMemo(() => {
    const repos = [
      ...new Set(
        (dashboard?.insights ?? [])
          .map((id) => byId.get(id))
          .filter((i): i is MockInsight => i !== undefined)
          .flatMap((i) => i.repos),
      ),
    ]
    return repos.length > 0 ? `repo:${repos.join(' or repo:')}` : 'all repos'
  }, [dashboard, byId])

  const seriesCount = insights.reduce((n, i) => n + i.series.length, 0)

  /** One-line summary of the active dashboard — unfiltered, its own totals. */
  const dashSummary = useMemo(() => {
    const list = (dashboard?.insights ?? [])
      .map((id) => byId.get(id))
      .filter((i): i is MockInsight => i !== undefined)
    const series = list.reduce((n, i) => n + i.series.length, 0)
    const latest = Math.max(
      ...list.flatMap((i) => i.series.flatMap((s) => s.points.map((p) => Date.parse(p.at)))),
    )
    return { insights: list.length, series, latest }
  }, [dashboard, byId])

  // Create/edit flows live on their own view with a breadcrumb back —
  // the convention shared with Monitoring/Contexts, not an inline panel.
  if (creating) {
    return <CreateInsightView dark={dark} onBack={() => setCreating(false)} />
  }

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Insights</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>
          {insights.length} insights · {seriesCount} series
        </span>
        <span {...stylex.props(styles.spacer)} />
        <ActionButton
          size="sm"
          icon={<IconPlus size={12} />}
          onClick={() => setCreating(true)}
          title="Open a local-draft create form — the endpoint is pending, so nothing is saved"
        >
          Create insight
        </ActionButton>
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="title or query"
            onClear={() => setText('')}
            aria-label="Filter insights"
          />
        </div>
      </header>

      <div {...stylex.props(styles.tabs)}>
        <ActionToggleGroup
          aria-label="Dashboard"
          options={MOCK_INSIGHT_DASHBOARDS.map((d) => ({
            value: d.id,
            label: `${d.title} · ${d.insights.length}`,
          }))}
          value={dashId}
          onValueChange={(v) => {
            const id = Array.isArray(v) ? v[0] : v
            if (id) {
              setDashId(id)
              setExpanded(null)
            }
          }}
        />
      </div>

      <p {...stylex.props(styles.dashSummary)}>
        {dashSummary.insights} insights · {dashSummary.series} series
        {Number.isFinite(dashSummary.latest) && (
          <>
            {' · latest sample '}
            <DisplayTimeAgo value={dashSummary.latest} />
          </>
        )}
      </p>

      <div {...stylex.props(styles.filters)}>
        <span {...stylex.props(styles.filterIcon)}>
          <Funnel size={12} />
        </span>
        <span {...stylex.props(styles.filterLabel)}>scope</span>
        <DisplayBadge text={scope} color={false} title="Repos resolved at insight creation">
          {scope}
        </DisplayBadge>
        <span {...stylex.props(styles.spacer)} />
        <span {...stylex.props(styles.filterLabel)}>window</span>
        <ActionToggleGroup
          aria-label="Time window"
          options={WINDOW_OPTIONS}
          value={String(windowDays)}
          onValueChange={(v) => {
            const n = Number(Array.isArray(v) ? v[0] : v)
            if (n > 0) setWindowDays(n)
          }}
        />
      </div>

      <div {...stylex.props(styles.list)}>
        {insights.map((insight) => (
          <InsightRow
            key={insight.id}
            insight={insight}
            windowDays={windowDays}
            expanded={expanded === insight.id}
            onToggle={() => setExpanded((e) => (e === insight.id ? null : insight.id))}
            dark={dark}
          />
        ))}
        {insights.length === 0 && (
          <p {...stylex.props(styles.empty)}>No insights match this filter.</p>
        )}
      </div>
      <p {...stylex.props(styles.note)}>
        Fixture data — dashboards and series are generated shape, not measured truth. A real
        insight re-runs its query over each snapshot; the repo scope chip and time window
        slice fixture points only. Legend toggles and CSV export act on the same fixture
        points; the create panel is a local draft — nothing is saved.
      </p>
    </div>
  )
}

function InsightRow({
  insight,
  windowDays,
  expanded,
  onToggle,
  dark,
}: {
  insight: MockInsight
  windowDays: number
  expanded: boolean
  onToggle: () => void
  dark: boolean
}) {
  /** Legend toggles — series names hidden from both charts (and the CSV). */
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set())
  const sliced = sliceToWindow(insight, windowDays)
  const visible = sliced.filter((s) => !hidden.has(s.name))

  const toggleSeries = (name: string) =>
    setHidden((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })

  return (
    <div {...stylex.props(styles.item)}>
      <div {...stylex.props(styles.row)}>
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={expanded}
          {...stylex.props(styles.rowButton)}
        >
          <span {...stylex.props(styles.caret, expanded && styles.caretOpen)}>
            <IconCaretDown size={12} />
          </span>
          <span {...stylex.props(styles.rowMain)}>
            <span {...stylex.props(styles.rowTitle)}>{insight.title}</span>
            <span {...stylex.props(styles.rowQuery)}>{insight.query}</span>
          </span>
          <span {...stylex.props(styles.chart)}>
            <LineChart series={visible} width={220} height={68} dark={dark} />
          </span>
        </button>
        <span {...stylex.props(styles.legend)}>
          {sliced.map((s) => {
            const latest = s.points[s.points.length - 1]?.value ?? 0
            const prev = s.points[s.points.length - 2]?.value ?? latest
            const delta = latest - prev
            const off = hidden.has(s.name)
            return (
              <button
                key={s.name}
                type="button"
                aria-pressed={!off}
                title={off ? `Show ${s.name}` : `Hide ${s.name}`}
                onClick={() => toggleSeries(s.name)}
                {...stylex.props(styles.chip, off && styles.chipHidden)}
              >
                <span
                  {...stylex.props(styles.chipDot)}
                  style={{ backgroundColor: getHashColorFromString(s.name, 1, dark) }}
                />
                <span {...stylex.props(styles.chipName)}>{s.name}</span>
                <span {...stylex.props(styles.chipValue)}>
                  {latest}
                  <span {...stylex.props(styles.chipUnit)}> {insight.unit}</span>
                </span>
                <span
                  {...stylex.props(
                    styles.chipDelta,
                    delta > 0 ? styles.deltaUp : delta < 0 ? styles.deltaDown : null,
                  )}
                >
                  {delta > 0 ? `+${delta}` : delta}
                </span>
              </button>
            )
          })}
        </span>
      </div>
      {expanded && (
        <InsightDetail insight={insight} sliced={sliced} visible={visible} dark={dark} />
      )}
    </div>
  )
}

/** Expanded drill-down: meta row, large chart, then the raw point table. */
function InsightDetail({
  insight,
  sliced,
  visible,
  dark,
}: {
  insight: MockInsight
  /** Every series in the window — the data table shows them all. */
  sliced: MockInsightSeries[]
  /** Legend-visible subset — what the chart and CSV export draw from. */
  visible: MockInsightSeries[]
  dark: boolean
}) {
  const ats = [...new Set(sliced.flatMap((s) => s.points.map((p) => p.at)))].sort()
  const rows = ats.map((at) => ({
    at,
    values: sliced.map((s) => s.points.find((p) => p.at === at)?.value),
  }))
  const lastAt = ats[ats.length - 1]

  /** `date,series,value` rows for the legend-visible series only. */
  const exportCsv = () => {
    const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s)
    const lines = ['date,series,value']
    for (const s of visible) {
      for (const p of s.points) lines.push(`${p.at},${esc(s.name)},${p.value}`)
    }
    const blob = new Blob([lines.join('\n')], { type: 'text/csv' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${insight.id}.csv`
    a.click()
    URL.revokeObjectURL(a.href)
  }

  return (
    <div {...stylex.props(styles.detail)}>
      <div {...stylex.props(styles.detailMeta)}>
        {insight.repos.map((r) => (
          <DisplayBadge key={r} text={r} color={false}>
            repo:{r}
          </DisplayBadge>
        ))}
        <DisplayBadge text={`${insight.intervalDays}d`} color={false}>
          every {insight.intervalDays}d
        </DisplayBadge>
        {lastAt && (
          <span {...stylex.props(styles.metaText)}>
            last sample <DisplayTimeAgo value={lastAt} />
          </span>
        )}
        <span {...stylex.props(styles.metaNote)}>{insight.note}</span>
        <span {...stylex.props(styles.spacer)} />
        <ActionIconButton
          compact
          icon={<IconDownload size={13} />}
          label="Export CSV"
          tooltip={
            visible.length === 0
              ? 'All series hidden — nothing to export'
              : `Export ${visible.length} visible series as CSV`
          }
          disabled={visible.length === 0}
          onClick={exportCsv}
        />
      </div>
      <div {...stylex.props(styles.detailChart)}>
        {visible.length === 0 ? (
          <p {...stylex.props(styles.emptyChart)}>
            all series hidden — toggle a legend chip to bring one back
          </p>
        ) : (
          <LineChart series={visible} width={680} height={240} dark={dark} detailed />
        )}
      </div>
      <LayoutDataTable
        rows={rows}
        rowKey={(r) => r.at}
        columns={[
          {
            key: 'at',
            label: 'Date',
            width: 120,
            render: (r) => <span {...stylex.props(styles.cell)}>{r.at}</span>,
          },
          ...sliced.map((s, i) => ({
            key: s.name,
            label: (
              <span {...stylex.props(styles.colLabel)}>
                <span
                  {...stylex.props(styles.chipDot)}
                  style={{ backgroundColor: getHashColorFromString(s.name, 1, dark) }}
                />
                {s.name}
              </span>
            ),
            align: 'end' as const,
            render: (r: { at: string; values: (number | undefined)[] }) => (
              <span {...stylex.props(styles.cell)}>{r.values[i] ?? '—'}</span>
            ),
          })),
        ]}
      />
    </div>
  )
}

/**
 * Create-insight view — the draft form on its own screen (the create/edit
 * convention shared with Monitoring/Contexts: reached from the list
 * header, breadcrumb back). `Generate preview` hashes the query into a
 * fixture-shaped series — an honest preview of the chart's form, never a
 * backend run; `Save insight` stays disabled until the endpoint lands.
 */
function CreateInsightView({ dark, onBack }: { dark: boolean; onBack: () => void }) {
  const [title, setTitle] = useState('')
  const [query, setQuery] = useState('')
  const [preview, setPreview] = useState<MockInsightSeries[] | null>(null)

  const generate = () => {
    const q = query.trim()
    if (!q) return
    setPreview([generatedPreviewSeries(q, title.trim() || 'matches')])
  }

  return (
    <div {...stylex.props(styles.root)}>
      <LayoutBreadcrumb
        items={[{ label: 'Insights', onClick: onBack }, { label: 'New insight' }]}
      />

      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>New insight</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.faintNote)}>local draft — nothing is saved</span>
      </header>

      <div {...stylex.props(styles.createForm)}>
        <FormField label="Title">
          <FormTextInput
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="TODOs over time"
          />
        </FormField>
        <FormField
          label="Query"
          description="Re-run once per interval — the series tracks its match count."
        >
          <FormTextInput
            mono
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="TODO|FIXME|HACK"
          />
        </FormField>
      </div>
      <div {...stylex.props(styles.createFoot)}>
        <ActionButton size="sm" onClick={generate} disabled={!query.trim()}>
          Generate preview
        </ActionButton>
        <ActionButton size="sm" variant="primary" disabled title="endpoint pending — /v1/insights">
          Save insight
        </ActionButton>
        <ActionButton size="sm" onClick={onBack}>
          Cancel
        </ActionButton>
        <span {...stylex.props(styles.faintNote)}>endpoint pending — /v1/insights</span>
      </div>
      {preview && (
        <div {...stylex.props(styles.createPreview)}>
          <LineChart series={preview} width={560} height={140} dark={dark} />
          <span {...stylex.props(styles.faintNote)}>
            generated preview shape — hashed from the query, not a backend run
          </span>
        </div>
      )}
    </div>
  )
}

/**
 * Dependency-free multi-series line chart — one polyline per series, x
 * mapped by sample time so series stay aligned. `detailed` adds
 * gridlines, axis labels, per-point markers and a hover crosshair
 * (nearest-sample guide line + highlighted dots + a floating mono label)
 * for the drill-down view.
 */
function LineChart({
  series,
  width,
  height,
  dark,
  detailed = false,
}: {
  series: MockInsightSeries[]
  width: number
  height: number
  dark: boolean
  detailed?: boolean
}) {
  /** Snapped sample time under the cursor — `detailed` charts only. */
  const [hoverT, setHoverT] = useState<number | null>(null)

  const all = series.flatMap((s) => s.points)
  if (all.length === 0) return <svg width={width} height={height} aria-hidden />

  const tMin = Math.min(...all.map((p) => Date.parse(p.at)))
  const tMax = Math.max(...all.map((p) => Date.parse(p.at)))
  const tSpan = tMax - tMin || 1
  const vMin = Math.min(...all.map((p) => p.value))
  const vMax = Math.max(...all.map((p) => p.value))
  const vSpan = vMax - vMin || 1
  const padX = detailed ? 34 : 4
  const padY = detailed ? 16 : 5
  const px = (t: number) => padX + ((t - tMin) / tSpan) * (width - padX * 2)
  const py = (v: number) => height - padY - ((v - vMin) / vSpan) * (height - padY * 2)

  const grid = dark ? 'rgba(255,255,255,0.09)' : 'rgba(0,0,0,0.09)'
  const axisText = '#737373'
  const mid = vMin + vSpan / 2

  // Sorted union of sample times — the crosshair snaps to these.
  const sampleTimes = [...new Set(all.map((p) => Date.parse(p.at)))].sort((a, b) => a - b)

  /** viewBox-space x of the cursor -> nearest sample time. */
  const onMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const rect = e.currentTarget.getBoundingClientRect()
    if (rect.width === 0) return
    const x = ((e.clientX - rect.left) / rect.width) * width
    let best: number | null = null
    let bestDist = Infinity
    for (const t of sampleTimes) {
      const d = Math.abs(px(t) - x)
      if (d < bestDist) {
        bestDist = d
        best = t
      }
    }
    setHoverT(best)
  }

  // Guard against a stale hover time after the window/series changes.
  const hoverAt = detailed && hoverT != null && sampleTimes.includes(hoverT) ? hoverT : null
  const hoverRows =
    hoverAt == null
      ? []
      : series.flatMap((s) => {
          const p = s.points.find((pt) => Date.parse(pt.at) === hoverAt)
          return p
            ? [{ name: s.name, value: p.value, color: getHashColorFromString(s.name, 1, dark) }]
            : []
        })
  const hoverLabel =
    hoverAt == null ? '' : (all.find((p) => Date.parse(p.at) === hoverAt)?.at ?? '')

  // Floating label geometry — mono 9px ≈ 5.6px per char, flipped near the
  // right edge, pinned to the top strip of the plot.
  const labelLines = [hoverLabel, ...hoverRows.map((r) => `${r.name} ${r.value}`)]
  const labelW = Math.ceil(Math.max(...labelLines.map((l) => l.length)) * 5.6 + 14)
  const labelH = 20 + (labelLines.length - 1) * 11
  let labelX = hoverAt == null ? 0 : px(hoverAt) + 8
  if (labelX + labelW > width - 4) labelX = px(hoverAt ?? 0) - labelW - 8
  if (labelX < 4) labelX = 4
  const labelY = 5
  const cross = dark ? 'rgba(255,255,255,0.28)' : 'rgba(0,0,0,0.28)'
  const tipBg = dark ? 'rgba(17,17,17,0.92)' : 'rgba(255,255,255,0.92)'
  const tipStroke = dark ? 'rgba(255,255,255,0.16)' : 'rgba(0,0,0,0.16)'

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      style={detailed ? { display: 'block', width: '100%', height: 'auto' } : { display: 'block' }}
      role="img"
      aria-label="insight series chart"
      onMouseMove={detailed ? onMove : undefined}
      onMouseLeave={detailed ? () => setHoverT(null) : undefined}
    >
      {detailed &&
        [vMin, mid, vMax].map((v) => (
          <g key={v}>
            <line
              x1={padX}
              x2={width - padX}
              y1={py(v)}
              y2={py(v)}
              stroke={grid}
              strokeWidth={1}
            />
            <text
              x={padX - 6}
              y={py(v) + 3}
              textAnchor="end"
              fontSize={9}
              fontFamily={font.mono}
              fill={axisText}
            >
              {Math.round(v)}
            </text>
          </g>
        ))}
      {detailed && (
        <>
          <text x={padX} y={height - 3} fontSize={9} fontFamily={font.mono} fill={axisText}>
            {all.find((p) => Date.parse(p.at) === tMin)?.at}
          </text>
          <text
            x={width - padX}
            y={height - 3}
            textAnchor="end"
            fontSize={9}
            fontFamily={font.mono}
            fill={axisText}
          >
            {all.find((p) => Date.parse(p.at) === tMax)?.at}
          </text>
        </>
      )}
      {series.map((s) => {
        const color = getHashColorFromString(s.name, 1, dark)
        const coords = s.points
          .map((p) => `${px(Date.parse(p.at)).toFixed(1)},${py(p.value).toFixed(1)}`)
          .join(' ')
        const last = s.points[s.points.length - 1]
        return (
          <g key={s.name}>
            {s.points.length > 1 && (
              <polyline
                points={coords}
                fill="none"
                stroke={color}
                strokeWidth={detailed ? 2 : 1.5}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            )}
            {detailed &&
              s.points.map((p) => (
                <circle
                  key={p.at}
                  cx={px(Date.parse(p.at))}
                  cy={py(p.value)}
                  r={2}
                  fill={color}
                />
              ))}
            {!detailed && last && (
              <circle cx={px(Date.parse(last.at))} cy={py(last.value)} r={2} fill={color} />
            )}
            {s.points.length === 1 && s.points[0] && (
              <circle
                cx={px(Date.parse(s.points[0].at))}
                cy={py(s.points[0].value)}
                r={2.5}
                fill={color}
              />
            )}
          </g>
        )
      })}
      {hoverAt != null && (
        <g pointerEvents="none">
          <line
            x1={px(hoverAt)}
            x2={px(hoverAt)}
            y1={padY}
            y2={height - padY}
            stroke={cross}
            strokeWidth={1}
          />
          {hoverRows.map((r) => (
            <circle
              key={r.name}
              cx={px(hoverAt)}
              cy={py(r.value)}
              r={3.5}
              fill={r.color}
              stroke={tipBg}
              strokeWidth={1.5}
            />
          ))}
          <g transform={`translate(${labelX},${labelY})`}>
            <rect
              width={labelW}
              height={labelH}
              rx={4}
              fill={tipBg}
              stroke={tipStroke}
              strokeWidth={1}
            />
            <text x={7} y={15} fontSize={9} fontFamily={font.mono} fill={axisText}>
              {hoverLabel}
            </text>
            {hoverRows.map((r, i) => (
              <text
                key={r.name}
                x={7}
                y={26 + i * 11}
                fontSize={9}
                fontFamily={font.mono}
                fill={axisText}
              >
                <tspan fill={r.color}>●</tspan>
                <tspan dx={5}>{r.name}</tspan>
                <tspan dx={6} fontWeight={600}>
                  {r.value}
                </tspan>
              </text>
            ))}
          </g>
        </g>
      )}
    </svg>
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
    marginTop: 0,
    marginBottom: 0,
    marginLeft: 0,
    marginRight: 0,
    fontSize: 15,
    fontWeight: 600,
  },
  count: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  spacer: {
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
  },
  search: {
    width: 220,
  },
  tabs: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  dashSummary: {
    marginTop: 0,
    marginBottom: 0,
    marginLeft: 0,
    marginRight: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorFaint,
  },
  filters: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
  },
  filterIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  filterLabel: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    letterSpacing: '0.06em',
  },
  list: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderBase,
  },
  item: {
    display: 'flex',
    flexDirection: 'column',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
  },
  row: {
    display: 'flex',
    alignItems: 'center',
    gap: 16,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 2,
    paddingRight: 2,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  rowButton: {
    display: 'flex',
    alignItems: 'center',
    gap: 16,
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
    minWidth: 0,
    paddingTop: 0,
    paddingBottom: 0,
    paddingLeft: 0,
    paddingRight: 0,
    borderTopWidth: 0,
    borderBottomWidth: 0,
    borderLeftWidth: 0,
    borderRightWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
  },
  caret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    width: 14,
    justifyContent: 'center',
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  caretOpen: {
    transform: 'rotate(0deg)',
  },
  rowMain: {
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
    minWidth: 140,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  rowTitle: {
    fontSize: 12,
    fontWeight: 600,
  },
  rowQuery: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  chart: {
    display: 'block',
    flexShrink: 0,
  },
  legend: {
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
    width: 230,
    flexShrink: 0,
  },
  chip: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
    width: '100%',
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 4,
    paddingRight: 4,
    borderTopWidth: 0,
    borderBottomWidth: 0,
    borderLeftWidth: 0,
    borderRightWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: 'inherit',
    textAlign: 'left',
    cursor: 'pointer',
  },
  chipHidden: {
    opacity: vars.opMute,
  },
  chipDot: {
    display: 'inline-block',
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },
  chipName: {
    fontSize: 11,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    flexGrow: 1,
    flexShrink: 1,
    flexBasis: 0,
  },
  chipValue: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    fontWeight: 600,
    flexShrink: 0,
  },
  chipUnit: {
    fontSize: 9,
    fontWeight: 400,
    color: vars.colorFaint,
  },
  chipDelta: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    width: 34,
    textAlign: 'right',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  deltaUp: {
    color: vars.scaleMedium,
  },
  deltaDown: {
    color: vars.scaleLow,
  },
  detail: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    paddingTop: 4,
    paddingBottom: 14,
    paddingLeft: 30,
    paddingRight: 4,
  },
  detailMeta: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
  },
  metaText: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
    display: 'inline-flex',
    gap: 4,
    alignItems: 'center',
  },
  metaNote: {
    fontSize: 11,
    color: vars.colorMuted,
  },
  detailChart: {
    maxWidth: 720,
  },
  colLabel: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
  },
  cell: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
  },
  emptyChart: {
    marginTop: 0,
    marginBottom: 0,
    paddingTop: 24,
    paddingBottom: 24,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
    textAlign: 'center',
  },
  createForm: {
    display: 'flex',
    flexDirection: 'column',
    gap: 12,
    maxWidth: 560,
  },
  createFoot: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  createPreview: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
    alignItems: 'flex-start',
  },
  faintNote: {
    fontSize: 11,
    color: vars.colorFaint,
  },
  empty: {
    marginTop: 0,
    marginBottom: 0,
    paddingTop: 14,
    paddingBottom: 14,
    fontSize: 12,
    color: vars.colorFaint,
  },
  note: {
    marginTop: 0,
    marginBottom: 0,
    marginLeft: 0,
    marginRight: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
})
