import * as stylex from '@stylexjs/stylex'
import { Fragment, useMemo, useState } from 'react'
import { useNavigate } from 'react-router'
import { fileUrl } from '../lib/navigation'
import {
  MOCK_BATCH_CHANGES,
  type MockBatchChange,
  type MockChangeset,
  type MockChangesetState,
  type MockCheckState,
  type MockMergePoint,
  type MockReviewState,
} from '../mock/batchChanges'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { ActionToggleGroup } from '../ui/ActionToggleGroup'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayFilePath } from '../ui/DisplayFilePath'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FormCheckbox } from '../ui/FormCheckbox'
import { FormSearchField } from '../ui/FormSearchField'
import {
  IconArrowUpRight,
  IconCaretDown,
  IconCheck,
  IconError,
  IconMinus,
  IconPlus,
  IconSpinner,
} from '../ui/icons'
import { LayoutBreadcrumb, LayoutDataTable } from '../ui/LayoutStructure'
import { font, vars, type Severity } from '../ui/tokens.stylex'

/** Display order for the proportion bar, legend, and filter chips. */
const STATE_ORDER: MockChangesetState[] = ['open', 'merged', 'closed', 'draft', 'failed']

const STATE_SEVERITY: Record<MockChangesetState, Severity> = {
  open: 'low',
  merged: 'high',
  closed: 'medium',
  draft: 'neutral',
  failed: 'critical',
}

const CHECK_ICON = {
  passed: IconCheck,
  failed: IconError,
  pending: IconSpinner,
  none: IconMinus,
} as const

const CHECK_TITLE: Record<MockCheckState, string> = {
  passed: 'checks passed',
  failed: 'checks failed',
  pending: 'checks pending',
  none: 'no checks',
}

const REVIEW_SEVERITY: Record<Exclude<MockReviewState, 'none'>, Severity> = {
  approved: 'low',
  changes_requested: 'high',
  pending: 'medium',
}

function countStates(changesets: MockChangeset[]): Map<MockChangesetState, number> {
  const m = new Map<MockChangesetState, number>()
  for (const c of changesets) m.set(c.state, (m.get(c.state) ?? 0) + 1)
  return m
}

/** '2026-09-18T04:23:45.123Z' -> '09-18 04:23Z' — compact log timestamp. */
function logTime(iso: string): string {
  return `${iso.slice(5, 16).replace('T', ' ')}Z`
}

/** '2026-09-18T…' -> '09-18' — compact axis tick for the burndown chart. */
function dayOf(iso: string): string {
  return iso.slice(5, 10)
}

/** Detail-body tabs — spec+progress+burndown / changeset table / log. */
type DetailTab = 'overview' | 'changesets' | 'executions'

const BULK_ACTIONS = ['Publish', 'Merge', 'Close', 'Retry'] as const

/**
 * Batch Changes — Sourcegraph's fleet-edit surface (a spec produces
 * changesets across the codebase). MOCK: no `/v1/batch-changes` endpoint;
 * fixtures come from `src/mock/batchChanges.ts` and the screen is labeled
 * `preview`. Master-detail: the list drills into a batch change's spec,
 * progress, changesets and execution history.
 */
export function BatchChangesScreen() {
  const [selected, setSelected] = useState<string | null>(null)
  const batch = selected
    ? MOCK_BATCH_CHANGES.find((b) => b.name === selected)
    : undefined

  if (batch) {
    // Keyed so tab/selection/expansion state resets between batch changes.
    return (
      <BatchChangeDetail
        key={batch.name}
        batch={batch}
        onBack={() => setSelected(null)}
      />
    )
  }
  return <BatchChangeList onSelect={(b) => setSelected(b.name)} />
}

/** List view — every batch change with its changeset-state proportion bar. */
function BatchChangeList({ onSelect }: { onSelect: (b: MockBatchChange) => void }) {
  const [text, setText] = useState('')
  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return MOCK_BATCH_CHANGES.filter(
      (b) => !needle || `${b.name} ${b.description}`.toLowerCase().includes(needle),
    )
  }, [text])

  /** Fleet-wide rollup — fixture totals, not a live reconcile. */
  const summary = useMemo(() => {
    const open = MOCK_BATCH_CHANGES.filter((b) => b.state === 'open').length
    let merged = 0
    for (const b of MOCK_BATCH_CHANGES) {
      merged += b.changesets.filter((c) => c.state === 'merged').length
    }
    return { open, merged }
  }, [])

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Batch Changes</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>{rows.length === MOCK_BATCH_CHANGES.length ? `${rows.length} batch changes` : `${rows.length} of ${MOCK_BATCH_CHANGES.length} batch changes`}</span>
        <span {...stylex.props(styles.spacer)} />
        <ActionButton
          size="sm"
          icon={<IconPlus size={12} />}
          disabled
          title="endpoint pending — /v1/batch-changes"
        >
          New batch change
        </ActionButton>
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="name"
            onClear={() => setText('')}
            aria-label="Filter batch changes"
          />
        </div>
      </header>

      <p {...stylex.props(styles.summary)}>
        {MOCK_BATCH_CHANGES.length} batch changes · {summary.open} open ·{' '}
        {summary.merged} merged changesets
      </p>

      <LayoutDataTable
        rows={rows}
        rowKey={(b) => b.name}
        onRowClick={onSelect}
        columns={[
          {
            key: 'name',
            label: 'Batch change',
            render: (b) => (
              <div {...stylex.props(styles.cell)}>
                <span {...stylex.props(styles.name)}>{b.name}</span>
                <span {...stylex.props(styles.desc)}>{b.description}</span>
              </div>
            ),
          },
          {
            key: 'state',
            label: 'State',
            width: 90,
            render: (b) => (
              <DisplayBadge severity={STATE_SEVERITY[b.state]}>{b.state}</DisplayBadge>
            ),
          },
          {
            key: 'changesets',
            label: 'Changesets',
            width: 210,
            render: (b) => <ChangesetBar changesets={b.changesets} />,
          },
          {
            key: 'updated',
            label: 'Updated',
            align: 'end',
            width: 90,
            render: (b) => <DisplayTimeAgo value={b.updatedAt} />,
          },
        ]}
      />
      <p {...stylex.props(styles.note)}>
        Fixture data — the /v1/batch-changes endpoint is pending. Click a row for the
        mock detail view; a real batch change derives its bar from indexed diffs.
      </p>
    </div>
  )
}

/** Detail view — tabbed: overview (spec, progress, burndown), changesets, executions. */
function BatchChangeDetail({
  batch,
  onBack,
}: {
  batch: MockBatchChange
  onBack: () => void
}) {
  const [tab, setTab] = useState<DetailTab>('overview')
  const [states, setStates] = useState<Set<MockChangesetState>>(new Set())
  const [checked, setChecked] = useState<ReadonlySet<string>>(new Set())
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())
  const counts = useMemo(() => countStates(batch.changesets), [batch])
  const rows = useMemo(
    () =>
      states.size === 0
        ? batch.changesets
        : batch.changesets.filter((c) => states.has(c.state)),
    [batch, states],
  )
  const merged = counts.get('merged') ?? 0

  const toggle = (s: MockChangesetState) =>
    setStates((prev) => {
      const next = new Set(prev)
      if (next.has(s)) next.delete(s)
      else next.add(s)
      return next
    })

  const toggleCheck = (path: string, on: boolean) =>
    setChecked((prev) => {
      const next = new Set(prev)
      if (on) next.add(path)
      else next.delete(path)
      return next
    })

  /** Select-all applies to the currently filtered rows. */
  const toggleAll = (on: boolean) =>
    setChecked(on ? new Set(rows.map((c) => c.path)) : new Set())

  const toggleExpand = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })

  return (
    <div {...stylex.props(styles.root)}>
      <LayoutBreadcrumb
        items={[{ label: 'Batch Changes', onClick: onBack }, { label: batch.name }]}
      />
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>{batch.name}</h2>
        <DisplayBadge severity={STATE_SEVERITY[batch.state]}>{batch.state}</DisplayBadge>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.spacer)} />
        <span {...stylex.props(styles.count)}>
          updated <DisplayTimeAgo value={batch.updatedAt} />
        </span>
      </header>

      <p {...stylex.props(styles.desc)}>{batch.description}</p>

      <div>
        <ActionToggleGroup
          aria-label="Batch change sections"
          options={[
            { value: 'overview', label: 'Overview' },
            {
              value: 'changesets',
              label: `Changesets · ${batch.changesets.length}`,
            },
            {
              value: 'executions',
              label: `Executions · ${batch.executions.length}`,
            },
          ]}
          value={tab}
          onValueChange={(v) => {
            const next = (Array.isArray(v) ? v[0] : v) as DetailTab | ''
            if (next) setTab(next)
          }}
        />
      </div>

      {tab === 'overview' && (
        <>
          <section {...stylex.props(styles.section)}>
            <div {...stylex.props(styles.sectionHead)}>
              <span {...stylex.props(styles.label)}>Batch spec</span>
              <span {...stylex.props(styles.hint)}>yaml excerpt</span>
            </div>
            <pre {...stylex.props(styles.spec)}>{batch.spec}</pre>
          </section>

          <section {...stylex.props(styles.section)}>
            <div {...stylex.props(styles.sectionHead)}>
              <span {...stylex.props(styles.label)}>Progress</span>
              <span {...stylex.props(styles.hint)}>
                {merged} of {batch.changesets.length} merged
              </span>
            </div>
            <span {...stylex.props(styles.barLg)}>
              {STATE_ORDER.map((s) => {
                const n = counts.get(s) ?? 0
                if (n === 0) return null
                return (
                  <span
                    key={s}
                    {...stylex.props(styles.seg, STATE_SEG[s])}
                    style={{ width: `${(n / batch.changesets.length) * 100}%` }}
                  />
                )
              })}
            </span>
            <div {...stylex.props(styles.legend)}>
              {STATE_ORDER.map((s) => {
                const n = counts.get(s) ?? 0
                if (n === 0) return null
                return (
                  <span key={s} {...stylex.props(styles.legendItem)}>
                    <span {...stylex.props(styles.dot, STATE_SEG[s])} />
                    {n} {s}
                  </span>
                )
              })}
            </div>
            <MergedChart
              points={batch.mergedOverTime}
              total={batch.changesets.length}
              width={640}
              height={76}
            />
          </section>
        </>
      )}

      {tab === 'changesets' && (
        <section {...stylex.props(styles.section)}>
          <div {...stylex.props(styles.sectionHead)}>
            <span {...stylex.props(styles.label)}>Changesets</span>
            <span {...stylex.props(styles.hint)}>
              {rows.length} of {batch.changesets.length}
            </span>
            <span {...stylex.props(styles.spacer)} />
            <div {...stylex.props(styles.chips)} role="group" aria-label="Filter by state">
              <button
                type="button"
                aria-pressed={states.size === 0}
                onClick={() => setStates(new Set())}
                {...stylex.props(styles.chip, states.size === 0 && styles.chipOn)}
              >
                all
                <span {...stylex.props(styles.chipCount)}>{batch.changesets.length}</span>
              </button>
              {STATE_ORDER.map((s) => {
                const n = counts.get(s) ?? 0
                if (n === 0) return null
                const on = states.has(s)
                return (
                  <button
                    key={s}
                    type="button"
                    aria-pressed={on}
                    onClick={() => toggle(s)}
                    {...stylex.props(styles.chip, on && styles.chipOn)}
                  >
                    <span {...stylex.props(styles.dot, STATE_SEG[s])} />
                    {s}
                    <span {...stylex.props(styles.chipCount)}>{n}</span>
                  </button>
                )
              })}
            </div>
          </div>
          {checked.size > 0 && (
            <div {...stylex.props(styles.bulk)}>
              <span {...stylex.props(styles.bulkCount)}>{checked.size} selected</span>
              {BULK_ACTIONS.map((action) => (
                <ActionButton
                  key={action}
                  size="sm"
                  disabled
                  title="endpoint pending — /v1/batch-changes"
                >
                  {action}
                </ActionButton>
              ))}
              <ActionButton
                size="sm"
                variant="text"
                onClick={() => setChecked(new Set())}
              >
                clear
              </ActionButton>
              <span {...stylex.props(styles.spacer)} />
              <span {...stylex.props(styles.hint)}>preview — bulk ops not wired</span>
            </div>
          )}
          <ChangesetTable
            rows={rows}
            checked={checked}
            expanded={expanded}
            onToggleCheck={toggleCheck}
            onToggleAll={toggleAll}
            onToggleExpand={toggleExpand}
          />
          {rows.length === 0 && (
            <p {...stylex.props(styles.note)}>No changesets match the selected states.</p>
          )}
        </section>
      )}

      {tab === 'executions' && (
        <section {...stylex.props(styles.section)}>
          <div {...stylex.props(styles.sectionHead)}>
            <span {...stylex.props(styles.label)}>Execution history</span>
          </div>
          <div {...stylex.props(styles.log)}>
            {batch.executions.map((e, i) => (
              <div key={`${e.at}-${i}`} {...stylex.props(styles.logLine)}>
                <span {...stylex.props(styles.logTs)}>{logTime(e.at)}</span>
                <span {...stylex.props(styles.logAction)}>{e.action}</span>
                <span {...stylex.props(styles.logDetail)}>{e.detail}</span>
              </div>
            ))}
          </div>
        </section>
      )}

      <p {...stylex.props(styles.note)}>
        Fixture data — spec, checks, reviews, diffs, burndown and execution history
        are mocked. A real batch change reconciles changesets against the code host
        on every sync.
      </p>
    </div>
  )
}

/**
 * Changeset table — mirrors `LayoutDataTable`'s hairline look but adds a
 * selection column and inline-expanding diff rows (`tr` + `colSpan`), which
 * the shared table can't express.
 */
function ChangesetTable({
  rows,
  checked,
  expanded,
  onToggleCheck,
  onToggleAll,
  onToggleExpand,
}: {
  rows: MockChangeset[]
  checked: ReadonlySet<string>
  expanded: ReadonlySet<string>
  onToggleCheck: (path: string, on: boolean) => void
  onToggleAll: (on: boolean) => void
  onToggleExpand: (path: string) => void
}) {
  const allChecked = rows.length > 0 && rows.every((c) => checked.has(c.path))
  const someChecked = rows.some((c) => checked.has(c.path))
  return (
    <div {...stylex.props(styles.tableWrap)}>
      <table {...stylex.props(styles.table)}>
        <thead>
          <tr>
            <th style={{ width: 30 }} {...stylex.props(styles.th, styles.thCheck)}>
              <FormCheckbox
                checked={allChecked}
                indeterminate={!allChecked && someChecked}
                onCheckedChange={onToggleAll}
                label={<span {...stylex.props(styles.srOnly)}>select all visible</span>}
              />
            </th>
            <th {...stylex.props(styles.th)}>Changeset</th>
            <th style={{ width: 100 }} {...stylex.props(styles.th)}>State</th>
            <th
              style={{ width: 60, textAlign: 'center' }}
              {...stylex.props(styles.th)}
            >
              Checks
            </th>
            <th style={{ width: 140 }} {...stylex.props(styles.th)}>Review</th>
            <th
              style={{ width: 110, textAlign: 'end' }}
              {...stylex.props(styles.th)}
            >
              Diff
            </th>
            <th
              style={{ width: 90, textAlign: 'end' }}
              {...stylex.props(styles.th)}
            >
              Updated
            </th>
            <th style={{ width: 30 }} {...stylex.props(styles.th)} />
          </tr>
        </thead>
        <tbody>
          {rows.map((c) => (
            <ChangesetRow
              key={c.path}
              changeset={c}
              open={expanded.has(c.path)}
              checked={checked.has(c.path)}
              onToggleCheck={(on) => onToggleCheck(c.path, on)}
              onToggleExpand={() => onToggleExpand(c.path)}
            />
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * One changeset — the row toggles the inline diff; the trailing ↗ keeps the
 * `/browse` deep link one click away (same split as `CommitsScreen`'s file
 * rows). The action reveals on hover / focus-within.
 */
function ChangesetRow({
  changeset: c,
  open,
  checked,
  onToggleCheck,
  onToggleExpand,
}: {
  changeset: MockChangeset
  open: boolean
  checked: boolean
  onToggleCheck: (on: boolean) => void
  onToggleExpand: () => void
}) {
  const navigate = useNavigate()
  const [hover, setHover] = useState(false)
  return (
    <Fragment>
      <tr
        tabIndex={0}
        onClick={onToggleExpand}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onToggleExpand()
          }
        }}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        aria-expanded={open}
        {...stylex.props(styles.tr)}
      >
        <td
          onClick={(e) => e.stopPropagation()}
          {...stylex.props(styles.td, styles.tdCheck)}
        >
          <FormCheckbox
            checked={checked}
            onCheckedChange={onToggleCheck}
            label={<span {...stylex.props(styles.srOnly)}>select {c.path}</span>}
          />
        </td>
        <td {...stylex.props(styles.td)}>
          <span {...stylex.props(styles.pathCell)}>
            <span {...stylex.props(styles.caret, open && styles.caretOpen)}>
              <IconCaretDown size={10} />
            </span>
            <DisplayFilePath path={c.path} />
          </span>
        </td>
        <td {...stylex.props(styles.td)}>
          <DisplayBadge severity={STATE_SEVERITY[c.state]}>{c.state}</DisplayBadge>
        </td>
        <td style={{ textAlign: 'center' }} {...stylex.props(styles.td)}>
          <Check state={c.checkState} />
        </td>
        <td {...stylex.props(styles.td)}>
          {c.reviewState === 'none' ? (
            <span {...stylex.props(styles.dash)}>–</span>
          ) : (
            <DisplayBadge severity={REVIEW_SEVERITY[c.reviewState]}>
              {c.reviewState.replace('_', ' ')}
            </DisplayBadge>
          )}
        </td>
        <td style={{ textAlign: 'end' }} {...stylex.props(styles.td)}>
          <span {...stylex.props(styles.diff)}>
            <span {...stylex.props(styles.diffAdd)}>+{c.added}</span>
            <span {...stylex.props(styles.diffDel)}>−{c.removed}</span>
          </span>
        </td>
        <td style={{ textAlign: 'end' }} {...stylex.props(styles.td)}>
          <DisplayTimeAgo value={c.updatedAt} />
        </td>
        <td {...stylex.props(styles.td, styles.tdAction)}>
          <span
            {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}
            onClick={(e) => e.stopPropagation()}
          >
            <ActionIconButton
              compact
              icon={<IconArrowUpRight size={10} />}
              tooltip={`open ${c.path}`}
              label={`Open ${c.path}`}
              onClick={() => navigate(fileUrl(c.path))}
            />
          </span>
        </td>
      </tr>
      {open && (
        <tr>
          <td colSpan={8} {...stylex.props(styles.tdDetail)}>
            <DiffView diff={c.diff} />
          </td>
        </tr>
      )}
    </Fragment>
  )
}

/**
 * Mock unified diff — parses the fixture's ` `/`+`/`-`/`@@` prefixes into
 * hunk / added / removed / context lines.
 */
function DiffView({ diff }: { diff: string }) {
  return (
    <div {...stylex.props(styles.diffBox)}>
      {diff.split('\n').map((line, i) => {
        const kind = line.startsWith('@@')
          ? 'hunk'
          : line.startsWith('+')
            ? 'add'
            : line.startsWith('-')
              ? 'del'
              : 'ctx'
        const sigil = kind === 'add' ? '+' : kind === 'del' ? '−' : ' '
        const text = kind === 'hunk' ? line : line.slice(1)
        return (
          <div key={i} {...stylex.props(styles.dl, DIFF_LINE[kind])}>
            <span {...stylex.props(styles.sigil)}>{sigil}</span>
            <span {...stylex.props(styles.dlText)}>{text}</span>
          </div>
        )
      })}
    </div>
  )
}

/**
 * Burndown — cumulative merged changesets over sync time, hand-rolled SVG
 * like `InsightsScreen`'s LineChart. The dashed rail marks the changeset
 * total; the area + line ride the merged-state color (scaleHigh).
 */
function MergedChart({
  points,
  total,
  width,
  height,
}: {
  points: MockMergePoint[]
  total: number
  width: number
  height: number
}) {
  if (points.length === 0) {
    return <span {...stylex.props(styles.hint)}>no merge events recorded</span>
  }
  const tMin = Math.min(...points.map((p) => Date.parse(p.at)))
  const tMax = Math.max(...points.map((p) => Date.parse(p.at)))
  const tSpan = tMax - tMin || 1
  const vMax = Math.max(total, 1)
  const padX = 26
  const padT = 8
  const padB = 16
  const px = (t: number) => padX + ((t - tMin) / tSpan) * (width - padX - 8)
  const py = (v: number) => height - padB - (v / vMax) * (height - padB - padT)

  const coords = points
    .map((p) => `${px(Date.parse(p.at)).toFixed(1)},${py(p.count).toFixed(1)}`)
    .join(' ')
  const area = `${padX},${py(0)} ${coords} ${px(tMax)},${py(0)}`
  const first = points[0]
  const last = points[points.length - 1]

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      style={{ display: 'block', width: '100%', height: 'auto' }}
      role="img"
      aria-label="changesets merged over time"
      {...stylex.props(styles.chart)}
    >
      {[...new Set([0, total])].map((v) => (
        <g key={v}>
          <line
            x1={padX}
            x2={width - 8}
            y1={py(v)}
            y2={py(v)}
            stroke="currentColor"
            strokeWidth={1}
            strokeDasharray={v === total ? '3 3' : undefined}
            opacity={v === total ? 0.5 : 0.3}
          />
          <text
            x={padX - 6}
            y={py(v) + 3}
            textAnchor="end"
            fontSize={9}
            fontFamily={font.mono}
            fill="currentColor"
          >
            {v}
          </text>
        </g>
      ))}
      {first && (
        <text
          x={padX}
          y={height - 3}
          fontSize={9}
          fontFamily={font.mono}
          fill="currentColor"
        >
          {dayOf(first.at)}
        </text>
      )}
      {last && (
        <text
          x={width - 8}
          y={height - 3}
          textAnchor="end"
          fontSize={9}
          fontFamily={font.mono}
          fill="currentColor"
        >
          {dayOf(last.at)}
        </text>
      )}
      <g style={{ color: vars.scaleHigh }}>
        {points.length > 1 && (
          <>
            <polygon points={area} fill="currentColor" opacity={0.1} />
            <polyline
              points={coords}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          </>
        )}
        {last && (
          <circle
            cx={px(Date.parse(last.at))}
            cy={py(last.count)}
            r={2.5}
            fill="currentColor"
          />
        )}
      </g>
    </svg>
  )
}

/** Stacked open/merged/closed/draft/failed proportion bar with mono counts. */
function ChangesetBar({ changesets }: { changesets: MockChangeset[] }) {
  const counts = countStates(changesets)
  const total = changesets.length
  if (total === 0) {
    return <span {...stylex.props(styles.num)}>no changesets yet</span>
  }
  return (
    <div {...stylex.props(styles.barCell)}>
      <span {...stylex.props(styles.bar)}>
        {STATE_ORDER.map((s) => {
          const n = counts.get(s) ?? 0
          if (n === 0) return null
          return (
            <span
              key={s}
              {...stylex.props(styles.seg, STATE_SEG[s])}
              style={{ width: `${(n / total) * 100}%` }}
            />
          )
        })}
      </span>
      <span {...stylex.props(styles.num)}>
        {STATE_ORDER.filter((s) => counts.get(s))
          .map((s) => `${counts.get(s)}${s[0]}`)
          .join(' ')}
      </span>
    </div>
  )
}

/** CI rollup glyph — ✓ passed / ✗ failed / … pending / – none. */
function Check({ state }: { state: MockCheckState }) {
  const Icon = CHECK_ICON[state]
  return (
    <span title={CHECK_TITLE[state]} {...stylex.props(styles.check, CHECK_STYLE[state])}>
      <Icon size={13} />
    </span>
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
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorFaint,
  },
  spacer: {
    flex: 1,
  },
  search: {
    width: 200,
  },
  cell: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    minWidth: 0,
  },
  name: {
    fontSize: 12,
    fontWeight: 600,
  },
  desc: {
    margin: 0,
    fontSize: 11,
    color: vars.colorMuted,
  },
  barCell: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
  },
  bar: {
    display: 'flex',
    width: 84,
    height: 6,
    borderRadius: 3,
    overflow: 'hidden',
    backgroundColor: vars.borderBase,
  },
  barLg: {
    display: 'flex',
    width: '100%',
    height: 8,
    borderRadius: 4,
    overflow: 'hidden',
    backgroundColor: vars.borderBase,
  },
  seg: {
    display: 'block',
    height: '100%',
  },
  segOpen: {
    backgroundColor: vars.scaleLow,
  },
  segMerged: {
    backgroundColor: vars.scaleHigh,
  },
  segClosed: {
    backgroundColor: vars.scaleMedium,
  },
  segDraft: {
    backgroundColor: vars.colorFaint,
  },
  segFailed: {
    backgroundColor: vars.scaleCritical,
  },
  num: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
    whiteSpace: 'nowrap',
  },
  section: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
  },
  sectionHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
  },
  label: {
    fontSize: 10,
    fontWeight: 600,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    color: vars.colorFaint,
  },
  hint: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorFaint,
  },
  spec: {
    margin: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    lineHeight: '1.55',
    color: vars.colorMuted,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 8,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    overflowX: 'auto',
    whiteSpace: 'pre',
  },
  legend: {
    display: 'flex',
    alignItems: 'center',
    gap: 12,
    flexWrap: 'wrap',
  },
  legendItem: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
  },
  dot: {
    display: 'inline-block',
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },
  chips: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    flexWrap: 'wrap',
  },
  chip: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 4,
    backgroundColor: 'transparent',
    color: vars.colorMuted,
    fontFamily: 'inherit',
    fontSize: 11,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 6,
    cursor: 'pointer',
    whiteSpace: 'nowrap',
  },
  chipOn: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
    borderColor: 'transparent',
  },
  chipCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  check: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
  },
  checkPassed: {
    color: vars.scaleLow,
  },
  checkFailed: {
    color: vars.scaleCritical,
  },
  checkPending: {
    color: vars.scaleMedium,
  },
  checkNone: {
    color: vars.colorFaint,
  },
  dash: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  diff: {
    display: 'inline-flex',
    gap: 6,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    whiteSpace: 'nowrap',
  },
  diffAdd: {
    color: vars.scaleLow,
  },
  diffDel: {
    color: vars.scaleHigh,
  },
  log: {
    display: 'flex',
    flexDirection: 'column',
    fontFamily: font.mono,
    fontSize: 11,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 8,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 12,
    paddingRight: 12,
  },
  logLine: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
    paddingTop: 3,
    paddingBottom: 3,
  },
  logTs: {
    color: vars.colorFaint,
    fontVariantNumeric: 'tabular-nums',
    flexShrink: 0,
  },
  logAction: {
    color: vars.colorBase,
    fontWeight: 600,
    width: 64,
    flexShrink: 0,
  },
  logDetail: {
    color: vars.colorMuted,
    minWidth: 0,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  // ── List summary strip ────────────────────────────────────────────────
  summary: {
    margin: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorFaint,
  },
  // ── Bulk-operations bar ───────────────────────────────────────────────
  bulk: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    flexWrap: 'wrap',
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 8,
    backgroundColor: vars.bgSunken,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 10,
    paddingRight: 10,
  },
  bulkCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorBase,
  },
  // ── Changeset table (LayoutDataTable look + checkbox/expand columns) ──
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
  thCheck: {
    paddingRight: 0,
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
  tdCheck: {
    paddingRight: 0,
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
  tdAction: {
    paddingLeft: 0,
    paddingRight: 4,
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
  pathCell: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
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
  srOnly: {
    position: 'absolute',
    width: 1,
    height: 1,
    marginTop: -1,
    marginBottom: -1,
    marginLeft: -1,
    marginRight: -1,
    overflow: 'hidden',
    clipPath: 'inset(50%)',
    whiteSpace: 'nowrap',
    borderWidth: 0,
  },
  // ── Inline diff preview ───────────────────────────────────────────────
  diffBox: {
    display: 'flex',
    flexDirection: 'column',
    fontFamily: font.mono,
    fontSize: 11,
    lineHeight: '1.55',
    backgroundColor: vars.bgSunken,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    paddingTop: 4,
    paddingBottom: 4,
    overflowX: 'auto',
  },
  dl: {
    display: 'flex',
    width: 'max-content',
    minWidth: '100%',
    paddingRight: 10,
  },
  dlHunk: {
    color: vars.colorFaint,
    backgroundColor: vars.bgCode,
  },
  dlAdd: {
    color: vars.scaleLow,
    backgroundColor: vars.tipSuccessBg,
  },
  dlDel: {
    color: vars.scaleHigh,
    backgroundColor: vars.tipErrorBg,
  },
  dlCtx: {
    color: vars.colorMuted,
  },
  sigil: {
    flexShrink: 0,
    width: 22,
    paddingLeft: 10,
  },
  dlText: {
    whiteSpace: 'pre',
    minWidth: 0,
  },
  // ── Burndown chart (grid + ticks read currentColor) ──────────────────
  chart: {
    color: vars.colorFaint,
  },
})

/** Bar-segment and legend-dot color per changeset state. */
const STATE_SEG = {
  open: styles.segOpen,
  merged: styles.segMerged,
  closed: styles.segClosed,
  draft: styles.segDraft,
  failed: styles.segFailed,
}

/** Check-glyph color per CI rollup. */
const CHECK_STYLE = {
  passed: styles.checkPassed,
  failed: styles.checkFailed,
  pending: styles.checkPending,
  none: styles.checkNone,
}

/** Diff-line tint per unified-diff line kind. */
const DIFF_LINE = {
  hunk: styles.dlHunk,
  add: styles.dlAdd,
  del: styles.dlDel,
  ctx: styles.dlCtx,
}
