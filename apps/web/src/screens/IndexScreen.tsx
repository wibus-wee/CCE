import * as stylex from '@stylexjs/stylex'
import { Fragment, useEffect, useState, type ReactNode } from 'react'
import { useNavigate } from 'react-router'
import {
  api,
  describeError,
  ProviderReport,
  ProviderState,
  ViewKind,
  ViewManifest,
  ViewState,
  type ArchitectureDiff,
  type ChangedRelation,
  type DiffEntityRef,
  type DiffRelation,
  type ExportSummary,
} from '../api'
import { fileUrl, queryUrl } from '../lib/navigation'
import { useQueryTelemetry } from '../lib/telemetry'
import { MOCK_HIT_MIX, MOCK_QUERY_METRICS, MOCK_QUERY_VOLUME, MOCK_TIMELINE } from '../mock'
import { MOCK_INDEX_JOBS, type MockIndexJob, type MockIndexJobStage } from '../mock/indexJobs'
import { ActionButton } from '../ui/ActionButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayProportionBar, type ProportionSegment } from '../ui/DisplayCharts'
import { DisplayDuration, DisplayNumber, DisplayTimeAgo } from '../ui/DisplayNumber'
import { DisplayProgressBar } from '../ui/DisplayProgressBar'
import { FeedbackEmptyState, FeedbackSkeleton } from '../ui/FeedbackStates'
import { FeedbackTip } from '../ui/FeedbackTip'
import { getHashColorFromString } from '../ui/format'
import { IconCaretDown, IconDatabase, IconDownload, IconGitCommit, IconRefresh } from '../ui/icons'
import { LayoutCard, LayoutSeparator } from '../ui/LayoutPrimitives'
import { font, vars, type Severity } from '../ui/tokens.stylex'
import { useColorScheme } from '../ui/useDark'
import { useElementSize } from '../ui/useElementSize'

const VIEW_SEVERITY: Record<ViewState, Severity> = {
  ready: 'low',
  building: 'medium',
  partial: 'medium',
  stale: 'high',
  unavailable: 'critical',
  failed: 'critical',
}

const PROVIDER_SEVERITY: Record<ProviderState, Severity> = {
  ready: 'low',
  missing: 'medium',
  not_applicable: 'neutral',
  failed: 'critical',
}

const STATE_COLOR: Record<ViewState, string> = {
  ready: vars.scaleLow,
  building: vars.scaleMedium,
  partial: vars.scaleMedium,
  stale: vars.scaleHigh,
  unavailable: vars.scaleCritical,
  failed: vars.scaleCritical,
}

const STATE_RANK: Record<ViewState, number> = {
  failed: 5,
  unavailable: 4,
  stale: 3,
  partial: 2,
  building: 1,
  ready: 0,
}

const MATRIX_VIEWS: ViewKind[] = [
  'lexical',
  'dense',
  'graph',
  'symbols',
  'knowledge',
  'history',
  'dataflow',
]

// Retrieval routes and the views that back them — a capability map of what
// this snapshot can actually serve, derived honestly from view states.
const ROUTE_BACKENDS: { label: string; views: ViewKind[] }[] = [
  { label: 'exact symbol', views: ['symbols'] },
  { label: 'lexical', views: ['lexical'] },
  { label: 'dense raw', views: ['dense'] },
  { label: 'dense summary', views: ['dense'] },
  { label: 'structural', views: ['graph', 'symbols'] },
  { label: 'knowledge', views: ['knowledge'] },
  { label: 'history', views: ['history'] },
  { label: 'dataflow', views: ['dataflow'] },
]

/**
 * Index — the health surface: every materialized view with its state and
 * capabilities, then the provider toolchains that fed it.
 */
export function IndexScreen({
  manifest,
  providers,
  providersError,
  statusError,
  loading,
  onReindex,
}: {
  manifest?: ViewManifest
  providers?: ProviderReport[]
  providersError?: string
  statusError?: string
  loading: boolean
  onReindex: () => void
}) {
  const views = manifest ? Object.entries(manifest.views) : []

  const telemetry = useQueryTelemetry()
  const latencySeries =
    telemetry.length > 0
      ? telemetry
          .slice(0, 30)
          .reverse()
          .map((t) => ({ at: t.at, ms: t.latencyMs, label: t.query, hits: t.hits }))
      : MOCK_QUERY_METRICS.map((m) => ({ at: m.at, ms: m.latencyMs, label: m.query }))
  const latencyIsMock = telemetry.length === 0
  const avgLatency = latencySeries.length
    ? Math.round(latencySeries.reduce((n, t) => n + t.ms, 0) / latencySeries.length)
    : undefined

  return (
    <div {...stylex.props(styles.root)}>
      <div {...stylex.props(styles.header)}>
        <div {...stylex.props(styles.titleGroup)}>
          <h2 {...stylex.props(styles.title)}>Materialized views</h2>
          {manifest && (
            <code {...stylex.props(styles.snap)} title={`snapshot ${manifest.snapshotId}`}>
              {manifest.snapshotId.slice(0, 21)}
            </code>
          )}
        </div>
        <ActionButton variant="action" icon={<IconRefresh size={13} />} onClick={onReindex}>
          Refresh index
        </ActionButton>
      </div>

      {statusError && <FeedbackTip variant="error">{statusError}</FeedbackTip>}

      {manifest && views.length > 0 && <HealthStrip views={views} />}

      <LayoutSeparator
        label="Query telemetry"
        aside={latencyIsMock ? 'preview fixtures until real queries run' : `${latencySeries.length} queries measured`}
      />
      <div {...stylex.props(styles.teleRow)}>
        <div {...stylex.props(styles.teleCol)} style={{ flexBasis: '50%' }}>
          <BandSub label="query latency" aside={latencyIsMock ? 'preview' : undefined} />
          <AreaChart series={latencySeries} formatY={fmtMs} formatX={relTime} />
          <div {...stylex.props(styles.latencyStats)}>
            <LatencyStat label="last" value={latencySeries[latencySeries.length - 1]?.ms ?? 0} />
            <LatencyStat label="avg" value={avgLatency ?? 0} />
            <LatencyStat label="p95" value={percentile(latencySeries.map((t) => t.ms), 0.95)} />
          </div>
        </div>
        <span {...stylex.props(styles.teleSep)} />
        <div {...stylex.props(styles.teleCol)} style={{ flexBasis: '25%' }}>
          <BandSub label="hit mix by route" aside="preview" />
          <HitMixDonut shares={MOCK_HIT_MIX} />
        </div>
        <span {...stylex.props(styles.teleSep)} />
        <div {...stylex.props(styles.teleCol)} style={{ flexBasis: '25%' }}>
          <BandSub
            label="query volume"
            aside={`${MOCK_QUERY_VOLUME.reduce((n, d) => n + d.queries, 0)} · 14d`}
          />
          <VolumeBars days={MOCK_QUERY_VOLUME} />
        </div>
      </div>

      {telemetry.length > 0 && (
        <>
          <LayoutSeparator label="Query log" aside={`${telemetry.length} recorded locally`} />
          <QueryLog telemetry={telemetry} />
        </>
      )}

      {manifest && (
        <>
          <LayoutSeparator label="Capability matrix" aside="route × backing view" />
          <CapabilityMatrix manifest={manifest} />
        </>
      )}

      {loading && !manifest ? (
        <div {...stylex.props(styles.grid)}>
          {Array.from({ length: 6 }, (_, i) => (
            <FeedbackSkeleton key={i} height={120} />
          ))}
        </div>
      ) : manifest && views.length === 0 ? (
        <FeedbackEmptyState
          icon={<IconDatabase size={20} />}
          title="No views reported yet"
          description="Press Refresh index to build the first snapshot."
          action={
            <ActionButton variant="primary" icon={<IconRefresh size={13} />} onClick={onReindex}>
              Refresh index
            </ActionButton>
          }
        />
      ) : (
        <div {...stylex.props(styles.grid)}>
          {views.map(([name, view]) => (
            <LayoutCard key={name} pad>
              <div {...stylex.props(styles.viewHead)}>
                <h3 {...stylex.props(styles.viewName)}>{name}</h3>
                <DisplayBadge severity={VIEW_SEVERITY[view.state]} rounded="full">
                  {view.state.replaceAll('_', ' ')}
                </DisplayBadge>
              </div>
              <div {...stylex.props(styles.caps)}>
                {view.capabilities.map((capability) => (
                  <div key={capability.name} {...stylex.props(styles.capRow)} title={capability.reason}>
                    <span {...stylex.props(styles.capName)}>{capability.name}</span>
                    <span {...stylex.props(styles.capLevel)}>{capability.level}</span>
                  </div>
                ))}
              </div>
              {view.message && <p {...stylex.props(styles.viewMsg)}>{view.message}</p>}
              <div {...stylex.props(styles.viewFoot)}>
                <DisplayTimeAgo value={view.updatedAt} />
              </div>
            </LayoutCard>
          ))}
        </div>
      )}

      <LayoutSeparator label="Provider toolchains" />

      {providersError && <FeedbackTip variant="error">{providersError}</FeedbackTip>}
      {!providers && !providersError && (
        <div {...stylex.props(styles.grid)}>
          {Array.from({ length: 3 }, (_, i) => (
            <FeedbackSkeleton key={i} height={96} />
          ))}
        </div>
      )}
      {providers?.length === 0 && (
        <FeedbackEmptyState title="No providers registered" description="Toolchain probes run during indexing." />
      )}
      {providers && providers.length > 0 && (
        <div {...stylex.props(styles.grid)}>
          {providers.map((provider) => (
            <LayoutCard key={provider.providerId} pad>
              <div {...stylex.props(styles.viewHead)}>
                <h3 {...stylex.props(styles.viewName)}>{provider.providerId}</h3>
                <DisplayBadge severity={PROVIDER_SEVERITY[provider.state]} rounded="full">
                  {provider.state.replaceAll('_', ' ')}
                </DisplayBadge>
              </div>
              {provider.tool && (
                <p {...stylex.props(styles.providerTool)}>
                  <span {...stylex.props(styles.capName)}>tool</span>
                  <code {...stylex.props(styles.providerToolCode)}>{provider.tool}</code>
                </p>
              )}
              <div {...stylex.props(styles.providerStats)}>
                {provider.durationMs != null && (
                  <span {...stylex.props(styles.stat)}>
                    <DisplayDuration value={provider.durationMs} colorize />
                  </span>
                )}
                {provider.scipDocuments > 0 && (
                  <span {...stylex.props(styles.stat)}>
                    <DisplayNumber value={provider.scipDocuments} /> docs ·{' '}
                    <DisplayNumber value={provider.scipDefinitions} /> defs ·{' '}
                    <DisplayNumber value={provider.scipReferenceEdges} /> refs
                  </span>
                )}
              </div>
              {provider.message && <p {...stylex.props(styles.viewMsg)}>{provider.message}</p>}
            </LayoutCard>
          ))}
        </div>
      )}

      <SnapshotDelta />

      <ExportCard />

      <IndexJobs />

      <ActivityTimeline />
    </div>
  )
}

/** Real view-state composition — a one-line freshness summary over the manifest. */
function HealthStrip({ views }: { views: [string, ViewManifest['views'][string]][] }) {
  const order: ViewState[] = ['ready', 'building', 'partial', 'stale', 'unavailable', 'failed']
  const colors: Record<ViewState, string> = {
    ready: vars.scaleLow,
    building: vars.scaleMedium,
    partial: vars.scaleMedium,
    stale: vars.scaleHigh,
    unavailable: vars.scaleCritical,
    failed: vars.scaleCritical,
  }
  const counts = new Map<ViewState, number>()
  for (const [, view] of views) counts.set(view.state, (counts.get(view.state) ?? 0) + 1)
  const segments: ProportionSegment[] = order
    .filter((state) => counts.has(state))
    .map((state) => ({
      value: counts.get(state)!,
      label: `${counts.get(state)} ${state.replaceAll('_', ' ')}`,
      color: colors[state],
    }))

  return <DisplayProportionBar segments={segments} height={5} showLegend />
}

/** Display order for the stage filter chips — queue order, not alphabetical. */
const JOB_STAGE_ORDER: MockIndexJobStage[] = ['running', 'queued', 'done', 'failed']

const JOB_STAGE_TITLE: Record<MockIndexJobStage, string> = {
  queued: 'queued — waiting for a worker',
  running: 'running',
  done: 'done',
  failed: 'failed',
}

/** The shared index pipeline every job expands into. */
const PIPELINE_STEPS = ['scan', 'parse', 'embed', 'pack'] as const

type PipelineStepState = 'done' | 'running' | 'pending' | 'failed'

/** Queue stage → the stepper node state it renders as. */
const GLYPH_STATE: Record<MockIndexJobStage, PipelineStepState> = {
  queued: 'pending',
  running: 'running',
  done: 'done',
  failed: 'failed',
}

/**
 * Per-step view of the pipeline. `done` jobs cleared every step; `queued`
 * jobs have not started; `running` jobs sit on the step their progress
 * fraction lands in; `failed` jobs completed everything before the step
 * their error names (fixture errors carry the stage word).
 */
function pipelineStepStates(job: MockIndexJob): PipelineStepState[] {
  if (job.stage === 'done') return PIPELINE_STEPS.map(() => 'done')
  if (job.stage === 'queued') return PIPELINE_STEPS.map(() => 'pending')
  if (job.stage === 'running') {
    const active = Math.min(
      PIPELINE_STEPS.length - 1,
      Math.floor((job.progress ?? 0) * PIPELINE_STEPS.length),
    )
    return PIPELINE_STEPS.map((_, i) => (i < active ? 'done' : i === active ? 'running' : 'pending'))
  }
  const named = PIPELINE_STEPS.findIndex((s) => job.error?.includes(s))
  const failedAt = named >= 0 ? named : PIPELINE_STEPS.length - 1
  return PIPELINE_STEPS.map((_, i) => (i < failedAt ? 'done' : i === failedAt ? 'failed' : 'pending'))
}

/**
 * Index jobs — the site-admin repo-updater queue idiom adapted to CCE's
 * indexing pipeline: what the workers are running, what is queued behind
 * them, and how the terminal history ended. MOCK: no `/v1/jobs` endpoint
 * exists; fixtures live in `src/mock/indexJobs.ts` and the section is
 * labeled `preview` so it is never mistaken for live scheduler truth.
 */
function IndexJobs() {
  const [stages, setStages] = useState<ReadonlySet<MockIndexJobStage>>(new Set())
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set())

  const counts = new Map<MockIndexJobStage, number>()
  for (const job of MOCK_INDEX_JOBS) counts.set(job.stage, (counts.get(job.stage) ?? 0) + 1)
  const running = counts.get('running') ?? 0
  const queued = counts.get('queued') ?? 0

  const toggleStage = (stage: MockIndexJobStage) =>
    setStages((prev) => {
      const next = new Set(prev)
      if (next.has(stage)) next.delete(stage)
      else next.add(stage)
      return next
    })
  const toggleRow = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const rows =
    stages.size === 0 ? MOCK_INDEX_JOBS : MOCK_INDEX_JOBS.filter((j) => stages.has(j.stage))

  return (
    <>
      <LayoutSeparator label="Index jobs" aside={`${running} running · ${queued} queued`} />
      <div {...stylex.props(styles.timelineHead)}>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.timelineNote)}>
          fixture data — the /v1/jobs endpoint is pending
        </span>
        <span {...stylex.props(styles.jobSpacer)} />
        <div {...stylex.props(styles.jobChips)} role="group" aria-label="Filter jobs by stage">
          <button
            type="button"
            aria-pressed={stages.size === 0}
            onClick={() => setStages(new Set())}
            {...stylex.props(styles.jobChip, stages.size === 0 && styles.jobChipOn)}
          >
            all
            <span {...stylex.props(styles.jobChipCount)}>{MOCK_INDEX_JOBS.length}</span>
          </button>
          {JOB_STAGE_ORDER.map((stage) => {
            const n = counts.get(stage) ?? 0
            if (n === 0) return null
            const on = stages.has(stage)
            return (
              <button
                key={stage}
                type="button"
                aria-pressed={on}
                onClick={() => toggleStage(stage)}
                {...stylex.props(styles.jobChip, on && styles.jobChipOn)}
              >
                <span {...stylex.props(styles.jobDot, JOB_DOT_STYLE[stage])} />
                {stage}
                <span {...stylex.props(styles.jobChipCount)}>{n}</span>
              </button>
            )
          })}
        </div>
      </div>
      {rows.length === 0 ? (
        <p {...stylex.props(styles.jobNote)}>No jobs match the selected stages.</p>
      ) : (
        <ol {...stylex.props(styles.timeline)}>
          {rows.map((job) => (
            <JobRow
              key={job.id}
              job={job}
              open={expanded.has(job.id)}
              onToggle={() => toggleRow(job.id)}
            />
          ))}
        </ol>
      )}
    </>
  )
}

/** One job — hairline row that expands to its pipeline-stage breakdown. */
function JobRow({
  job,
  open,
  onToggle,
}: {
  job: MockIndexJob
  open: boolean
  onToggle: () => void
}) {
  return (
    <li {...stylex.props(styles.jobItem)}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        {...stylex.props(styles.jobRow)}
      >
        <span {...stylex.props(styles.jobCaret, open && styles.jobCaretOpen)}>
          <IconCaretDown size={11} />
        </span>
        <JobGlyph stage={job.stage} />
        <DisplayBadge color={false}>{job.kind}</DisplayBadge>
        <code {...stylex.props(styles.jobRepo)}>{job.repo}</code>
        <span {...stylex.props(styles.jobWorker)}>{job.worker}</span>
        <span {...stylex.props(styles.jobStage, JOB_TEXT_STYLE[job.stage])}>{job.stage}</span>
        <span {...stylex.props(styles.jobDuration)}>
          <DisplayDuration value={job.durationMs} />
        </span>
        <span {...stylex.props(styles.jobWhen)}>
          <DisplayTimeAgo value={job.finishedAt ?? job.startedAt ?? ''} />
        </span>
      </button>
      {job.stage === 'running' && job.progress != null && (
        <div {...stylex.props(styles.jobProgress)}>
          <DisplayProgressBar value={job.progress} />
          <span {...stylex.props(styles.jobPct)}>{Math.round(job.progress * 100)}%</span>
        </div>
      )}
      {open && <JobDetail job={job} />}
    </li>
  )
}

/** Queue-stage glyph — the same animated node the stepper uses, at row size. */
function JobGlyph({ stage }: { stage: MockIndexJobStage }) {
  return (
    <span title={JOB_STAGE_TITLE[stage]} {...stylex.props(styles.jobCheck)}>
      <PipelineNode state={GLYPH_STATE[stage]} size={14} />
    </span>
  )
}

/**
 * Animated status node shared by the job-row glyph and the pipeline stepper.
 * `done` draws its check in, `running` spins a comet arc over a breathing
 * core, `pending` is a hollow ring, `failed` draws its cross. All motion is
 * gated behind `prefers-reduced-motion` — reduced users get the same states
 * rendered static. `delay` staggers the draw-in across the stepper.
 */
function PipelineNode({
  state,
  size = 20,
  delay = 0,
}: {
  state: PipelineStepState
  size?: number
  delay?: number
}) {
  const delayStyle = { animationDelay: `${delay}ms` }
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      aria-hidden="true"
      {...stylex.props(styles.nodeSvg)}
    >
      {state === 'done' && (
        <>
          <circle cx="10" cy="10" r="7.4" {...stylex.props(styles.nodeDoneRing)} />
          <path
            d="m6.7 10.3 2.2 2.3 4.4-4.9"
            style={delayStyle}
            {...stylex.props(styles.nodeCheck)}
          />
        </>
      )}
      {state === 'running' && (
        <>
          <circle cx="10" cy="10" r="7.4" {...stylex.props(styles.nodeTrack)} />
          <path d="M10 2.6a7.4 7.4 0 0 1 7.4 7.4" {...stylex.props(styles.nodeArc)} />
          <circle cx="10" cy="10" r="2" {...stylex.props(styles.nodeCore)} />
        </>
      )}
      {state === 'pending' && (
        <>
          <circle cx="10" cy="10" r="7.4" {...stylex.props(styles.nodePendingRing)} />
          <circle cx="10" cy="10" r="1.6" {...stylex.props(styles.nodePendingDot)} />
        </>
      )}
      {state === 'failed' && (
        <>
          <circle cx="10" cy="10" r="7.4" {...stylex.props(styles.nodeFailedRing)} />
          <path
            d="m7.3 7.3 5.4 5.4m0-5.4-5.4 5.4"
            style={delayStyle}
            {...stylex.props(styles.nodeCross)}
          />
        </>
      )}
    </svg>
  )
}

/**
 * Expanded job — the `scan → parse → embed → pack` stage breakdown as a
 * connected stepper, the terminal error for failed jobs, and the queue
 * actions (disabled — the jobs endpoint is pending).
 */
function JobDetail({ job }: { job: MockIndexJob }) {
  const steps = pipelineStepStates(job)
  return (
    <div {...stylex.props(styles.jobDetail)}>
      <div {...stylex.props(styles.jobSteps)}>
        {PIPELINE_STEPS.map((name, i) => {
          const state = steps[i]!
          return (
            <Fragment key={name}>
              {i > 0 && (
                <span {...stylex.props(styles.jobStepLink, STEP_LINK_STYLE[state])} />
              )}
              <span {...stylex.props(styles.jobStep)}>
                <PipelineNode state={state} delay={i * 70} />
                <span {...stylex.props(styles.jobStepName, STEP_NAME_STYLE[state])}>
                  {name}
                </span>
              </span>
            </Fragment>
          )
        })}
      </div>
      {job.error && <pre {...stylex.props(styles.jobError)}>{job.error}</pre>}
      <div {...stylex.props(styles.jobActions)}>
        <ActionButton size="sm" disabled title="endpoint pending — /v1/jobs">
          Retry
        </ActionButton>
        <ActionButton size="sm" disabled title="endpoint pending — /v1/jobs">
          Cancel
        </ActionButton>
      </div>
    </div>
  )
}

/**
 * Snapshot activity — pushes that invalidated views. MOCK: no `/v1/activity`
 * endpoint exists yet; fixtures live in `src/mock/` and the section is
 * labeled `preview` so it is never mistaken for live git truth.
 */
function ActivityTimeline() {
  return (
    <>
      <LayoutSeparator label="Snapshot activity" />
      <div {...stylex.props(styles.timelineHead)}>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.timelineNote)}>
          fixture data — endpoint pending — /v1/activity
        </span>
      </div>
      <ol {...stylex.props(styles.timeline)}>
        {MOCK_TIMELINE.map((entry) => (
          <li key={entry.sha} {...stylex.props(styles.timelineRow)}>
            <span {...stylex.props(styles.timelineIcon)}>
              <IconGitCommit size={13} />
            </span>
            <code {...stylex.props(styles.timelineSha)}>{entry.sha}</code>
            <span {...stylex.props(styles.timelineMsg)}>{entry.message}</span>
            <span {...stylex.props(styles.timelineViews)}>
              {entry.invalidated.map((view) => (
                <DisplayBadge key={view} color={false}>
                  {view}
                </DisplayBadge>
              ))}
            </span>
            <span {...stylex.props(styles.timelineWhen)}>
              <DisplayTimeAgo value={entry.at} />
            </span>
          </li>
        ))}
      </ol>
    </>
  )
}

// ── Telemetry + capability visuals ──────────────────────────────────────────

function percentile(values: number[], p: number): number {
  if (values.length === 0) return 0
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0
}
function LatencyStat({ label, value }: { label: string; value: number }) {
  return (
    <span {...stylex.props(styles.latencyStat)}>
      <span {...stylex.props(styles.latencyStatLabel)}>{label}</span>
      {fmtMs(value)}
    </span>
  )
}

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

/** Small mono sub-header inside a telemetry column — label + right aside. */
function BandSub({ label, aside }: { label: string; aside?: string }) {
  return (
    <div {...stylex.props(styles.bandSub)}>
      <span {...stylex.props(styles.bandSubLabel)}>{label}</span>
      {aside && <span {...stylex.props(styles.bandSubAside)}>{aside}</span>}
    </div>
  )
}

/**
 * Real line/area chart rendered at measured pixel width — y gridlines +
 * labels, x time labels, smooth Catmull-Rom curve, flat translucent fill,
 * marker on the latest point. No viewBox scaling: text stays crisp.
 */
function AreaChart({
  series,
  formatY,
  formatX,
}: {
  series: { at: string; ms: number; label: string; hits?: number }[]
  formatY: (v: number) => string
  formatX: (iso: string) => string
}) {
  const { ref, width } = useElementSize()
  const h = 132
  const padL = 30
  const padR = 8
  const padT = 8
  const padB = 16
  const w = Math.max(width, 80)
  const iw = w - padL - padR
  const ih = h - padT - padB
  const max = Math.max(...series.map((s) => s.ms), 1)
  if (series.length === 0) return <div ref={ref} {...stylex.props(styles.chartFrame)} />
  const pts = series.map((s, i) => ({
    x: padL + (i / Math.max(series.length - 1, 1)) * iw,
    y: padT + ih - (s.ms / max) * ih,
    s,
  }))
  const line = pts
    .map((p, i) => {
      if (i === 0) return `M ${p.x} ${p.y}`
      const p0 = pts[i - 1]!
      const cx = (p0.x + p.x) / 2
      return `C ${cx} ${p0.y} ${cx} ${p.y} ${p.x} ${p.y}`
    })
    .join(' ')
  const area = `${line} L ${pts[pts.length - 1]!.x} ${padT + ih} L ${pts[0]!.x} ${padT + ih} Z`
  const last = pts[pts.length - 1]
  const yTicks = [0, max / 2, max]
  const p95 = percentile(series.map((s) => s.ms), 0.95)
  const p95y = padT + ih - (p95 / max) * ih
  const maxHits = Math.max(...series.map((s) => s.hits ?? 0), 1)

  return (
    <div ref={ref} {...stylex.props(styles.chartFrame)}>
      {width > 0 && (
        <svg
          width={w}
          height={h}
          viewBox={`0 0 ${w} ${h}`}
          {...stylex.props(styles.chart)}
          role="img"
          aria-label="Query latency over time"
        >
          {yTicks.map((t) => {
            const y = padT + ih - (t / max) * ih
            return (
              <g key={t}>
                <line x1={padL} y1={y} x2={w - padR} y2={y} stroke={vars.borderMute} strokeDasharray="2 4" />
                <text x={padL - 5} y={y + 3} textAnchor="end" fontSize={8} fill={vars.colorFaint} fontFamily={font.mono}>
                  {formatY(t)}
                </text>
              </g>
            )
          })}
          {p95 > 0 && (
            <g>
              <line
                x1={padL}
                y1={p95y}
                x2={w - padR}
                y2={p95y}
                stroke={vars.scaleHigh}
                strokeOpacity={0.55}
                strokeDasharray="3 3"
              />
              <text x={w - padR} y={p95y - 3} textAnchor="end" fontSize={7.5} fill={vars.scaleHigh} fontFamily={font.mono}>
                p95
              </text>
            </g>
          )}
          <path d={area} fill={vars.primary500} fillOpacity={0.08} />
          <path d={line} fill="none" stroke={vars.primary500} strokeWidth={1.5} strokeLinecap="round" />
          {pts.map((p, i) => (
            <circle
              key={i}
              cx={p.x}
              cy={p.y}
              r={p.s.hits != null ? 1.6 + (p.s.hits / maxHits) * 2.4 : 0}
              fill={vars.primary500}
              fillOpacity={0.85}
            >
              <title>{`${p.s.label} — ${formatY(p.s.ms)}${p.s.hits != null ? ` · ${p.s.hits} hits` : ''} · ${formatX(p.s.at)}`}</title>
            </circle>
          ))}
          {last && (
            <circle cx={last.x} cy={last.y} r={3} fill={vars.primary500} stroke={vars.bgRaised} strokeWidth={1.5}>
              <title>{`${last.s.label} — ${formatY(last.s.ms)}`}</title>
            </circle>
          )}
          {pts.map((p, i) => (
            <circle key={`h${i}`} cx={p.x} cy={p.y} r={8} fill="transparent">
              <title>{`${p.s.label} — ${formatY(p.s.ms)}${p.s.hits != null ? ` · ${p.s.hits} hits` : ''} · ${formatX(p.s.at)}`}</title>
            </circle>
          ))}
          {series.length > 0 && (
            <>
              <text x={padL} y={h - 3} fontSize={8} fill={vars.colorFaint} fontFamily={font.mono}>
                {formatX(series[0]!.at)}
              </text>
              <text x={w - padR} y={h - 3} textAnchor="end" fontSize={8} fill={vars.colorFaint} fontFamily={font.mono}>
                now
              </text>
            </>
          )}
        </svg>
      )}
    </div>
  )
}

/** Daily query volume — vertical bars at measured pixel width, sparse day labels. */
function VolumeBars({ days }: { days: { day: string; queries: number }[] }) {
  const { ref, width } = useElementSize()
  const h = 132
  const padT = 6
  const padB = 14
  const ih = h - padT - padB
  const w = Math.max(width, 80)
  const max = Math.max(...days.map((d) => d.queries), 1)
  const step = w / days.length
  // Label density adapts to real width — labels need ~34px each.
  const labelEvery = Math.max(1, Math.ceil(34 / step))
  return (
    <div ref={ref} {...stylex.props(styles.chartFrame)}>
      {width > 0 && (
        <svg
          width={w}
          height={h}
          viewBox={`0 0 ${w} ${h}`}
          {...stylex.props(styles.chart)}
          role="img"
          aria-label="Queries per day"
        >
          <line x1={0} y1={padT + ih} x2={w} y2={padT + ih} stroke={vars.borderMute} />
          {days.map((d, i) => {
            const bh = Math.max((d.queries / max) * ih, 2)
            return (
              <rect
                key={d.day}
                x={i * step + Math.min(3, step / 4)}
                y={padT + ih - bh}
                width={Math.max(step - Math.min(6, step / 2), 2)}
                height={bh}
                rx={2}
                fill={vars.primary500}
                fillOpacity={0.35 + 0.65 * (d.queries / max)}
              >
                <title>{`${d.day} — ${d.queries} queries`}</title>
              </rect>
            )
          })}
          {days.map((d, i) =>
            i % labelEvery === 0 || i === days.length - 1 ? (
              <text
                key={d.day}
                x={Math.min(i * step + step / 2, w - 14)}
                y={h - 3}
                textAnchor="middle"
                fontSize={7.5}
                fill={vars.colorFaint}
                fontFamily={font.mono}
              >
                {d.day.slice(5)}
              </text>
            ) : null,
          )}
        </svg>
      )}
    </div>
  )
}

/** Route hit-mix — a multi-segment ring with hash-colored arcs + legend. */
function HitMixDonut({ shares }: { shares: { route: string; hits: number }[] }) {
  const dark = useColorScheme() === 'dark'
  const total = shares.reduce((n, s) => n + s.hits, 0) || 1
  const r = 44
  const c = 2 * Math.PI * r
  let offset = 0
  return (
    <div {...stylex.props(styles.mixWrap)}>
      <svg width={104} height={104} viewBox="0 0 104 104" role="img" aria-label="Hits by route">
        <circle cx={52} cy={52} r={r} fill="none" stroke={vars.bgSunken} strokeWidth={11} />
        {shares.map((s) => {
          const frac = s.hits / total
          const dash = `${frac * c} ${c}`
          const el = (
            <circle
              key={s.route}
              cx={52}
              cy={52}
              r={r}
              fill="none"
              stroke={getHashColorFromString(s.route, 1, dark)}
              strokeWidth={11}
              strokeDasharray={dash}
              strokeDashoffset={-offset * c}
              transform={`rotate(-90 52 52)`}
            >
              <title>{`${s.route} — ${s.hits} hits (${(frac * 100).toFixed(0)}%)`}</title>
            </circle>
          )
          offset += frac
          return el
        })}
        <text x={52} y={49} textAnchor="middle" fontSize={15} fontWeight={600} fill={vars.colorBase} fontFamily={font.mono}>
          {total}
        </text>
        <text x={52} y={62} textAnchor="middle" fontSize={7.5} fill={vars.colorFaint} fontFamily={font.mono}>
          hits
        </text>
      </svg>
      <div {...stylex.props(styles.mixLegend)}>
        {shares.map((s) => (
          <div key={s.route} {...stylex.props(styles.mixRow)}>
            <span
              {...stylex.props(styles.mixDot)}
              style={{ backgroundColor: getHashColorFromString(s.route, 1, dark) }}
            />
            <code {...stylex.props(styles.mixName)}>{s.route}</code>
            <span {...stylex.props(styles.mixPct)}>{((s.hits / total) * 100).toFixed(0)}%</span>
          </div>
        ))}
      </div>
    </div>
  )
}

/**
 * Query log — real localStorage telemetry: what was run, how long it
 * took, how many hits came back. Clicking reruns the query.
 */
function QueryLog({
  telemetry,
}: {
  telemetry: { query: string; latencyMs: number; hits: number; at: string }[]
}) {
  const navigate = useNavigate()
  return (
    <div {...stylex.props(styles.logList)}>
      {telemetry.slice(0, 10).map((t, i) => (
        <button
          key={`${t.at}:${i}`}
          type="button"
          {...stylex.props(styles.logRow)}
          onClick={() => navigate(queryUrl(t.query))}
          title={`${t.query} — ${t.hits} hits in ${fmtMs(t.latencyMs)}`}
        >
          <code {...stylex.props(styles.logQuery)}>{t.query}</code>
          <span {...stylex.props(styles.logMeta)}>
            <DisplayNumber value={t.hits} /> hits
          </span>
          <span {...stylex.props(styles.logMs)}>{fmtMs(t.latencyMs)}</span>
          <span {...stylex.props(styles.logWhen)}>
            <DisplayTimeAgo value={t.at} />
          </span>
        </button>
      ))}
    </div>
  )
}

function CapabilityMatrix({ manifest }: { manifest: ViewManifest }) {
  return (
    <div {...stylex.props(styles.matrix)}>
      <div {...stylex.props(styles.matrixHead)}>
        <span {...stylex.props(styles.matrixRouteHead)} />
        {MATRIX_VIEWS.map((v) => (
          <span key={v} {...stylex.props(styles.matrixColHead)}>
            {v}
          </span>
        ))}
        <span {...stylex.props(styles.matrixColHead, styles.matrixStateHead)}>state</span>
      </div>
      {ROUTE_BACKENDS.map(({ label, views: backing }) => {
        const worst = backing.reduce<ViewState | undefined>((acc, v) => {
          const state = manifest.views[v]?.state
          if (!state) return acc
          return !acc || STATE_RANK[state] > STATE_RANK[acc] ? state : acc
        }, undefined)
        return (
          <div key={label} {...stylex.props(styles.matrixRow)}>
            <code {...stylex.props(styles.matrixRoute)}>{label}</code>
            {MATRIX_VIEWS.map((v) => {
              const backed = backing.includes(v)
              const state = manifest.views[v]?.state
              return (
                <span key={v} {...stylex.props(styles.matrixCell)}>
                  {backed ? (
                    <span
                      {...stylex.props(styles.matrixDot)}
                      style={{ backgroundColor: state ? STATE_COLOR[state] : vars.colorFaint }}
                      title={`${label} ← ${v}: ${state?.replaceAll('_', ' ') ?? 'unknown'}`}
                    />
                  ) : (
                    <span {...stylex.props(styles.matrixEmpty)} />
                  )}
                </span>
              )
            })}
            <span
              {...stylex.props(styles.matrixState)}
              style={{ color: worst ? STATE_COLOR[worst] : vars.colorFaint }}
            >
              {worst?.replaceAll('_', ' ') ?? 'unknown'}
            </span>
          </div>
        )
      })}
    </div>
  )
}

const spin = stylex.keyframes({ to: { transform: 'rotate(360deg)' } })

/** Stroke draw-in for check/cross marks — dasharray 10 hides any path ≤10px. */
const drawIn = stylex.keyframes({
  from: { strokeDashoffset: 10 },
  to: { strokeDashoffset: 0 },
})

/** Breathing pulse for the running core and the running chip dot. */
const breathe = stylex.keyframes({
  from: { transform: 'scale(0.62)', opacity: 0.45 },
  to: { transform: 'scale(1)', opacity: 1 },
})

/** Marching dashes on the connector feeding a running step. */
const flow = stylex.keyframes({ to: { backgroundPositionX: '9px' } })

/** Mount animation for the expanded job detail. */
const riseIn = stylex.keyframes({
  from: { opacity: 0, transform: 'translateY(-3px)' },
  to: { opacity: 1, transform: 'none' },
})

/**
 * Snapshot delta — REAL `/v1/diff/architecture`: the entity/relation delta
 * between the previous committed snapshot and the current one. The daemon
 * answers 4XX/ViewUnavailable when no pair exists — that renders as a calm
 * note, not an error.
 */
function SnapshotDelta() {
  const navigate = useNavigate()
  const [diff, setDiff] = useState<ArchitectureDiff | null>(null)
  const [err, setErr] = useState<string>()
  const [busy, setBusy] = useState(true)
  const [open, setOpen] = useState<string>()

  useEffect(() => {
    let dead = false
    api
      .archDiff()
      .then((d) => !dead && setDiff(d))
      .catch((e) => !dead && setErr(describeError(e)))
      .finally(() => !dead && setBusy(false))
    return () => {
      dead = true
    }
  }, [])

  const counts = diff?.counts
  const lists: { key: string; label: string; rows: ReactNode }[] = diff
    ? [
        {
          key: 'ae',
          label: `added entities (${counts!.addedEntities})`,
          rows: diff.addedEntities
            .slice(0, 15)
            .map((e) => (
              <DeltaEntity key={e.entityId} e={e} onOpen={(p) => navigate(fileUrl(p))} />
            )),
        },
        {
          key: 're',
          label: `removed entities (${counts!.removedEntities})`,
          rows: diff.removedEntities
            .slice(0, 15)
            .map((e) => (
              <DeltaEntity key={e.entityId} e={e} onOpen={(p) => navigate(fileUrl(p))} />
            )),
        },
        {
          key: 'ar',
          label: `added relations (${counts!.addedRelations})`,
          rows: diff.addedRelations.slice(0, 15).map((r) => (
            <DeltaRel key={r.relationId} r={r} />
          )),
        },
        {
          key: 'rr',
          label: `removed relations (${counts!.removedRelations})`,
          rows: diff.removedRelations.slice(0, 15).map((r) => (
            <DeltaRel key={r.relationId} r={r} />
          )),
        },
        {
          key: 'cr',
          label: `changed relations (${counts!.changedRelations})`,
          rows: diff.changedRelations.slice(0, 15).map((r, i) => (
            <DeltaChanged key={`${r.headRelationId}-${i}`} r={r} />
          )),
        },
      ].filter((l) => {
        const n = Number(l.label.match(/\d+/)?.[0] ?? 0)
        return n > 0
      })
    : []

  return (
    <>
      <LayoutSeparator
        label="Snapshot delta"
        aside={
          diff ? (
            <code {...stylex.props(styles.deltaPair)}>
              {diff.baseSnapshotId.slice(5, 13)}…{diff.headSnapshotId.slice(5, 13)}
            </code>
          ) : undefined
        }
      />
      <div {...stylex.props(styles.timelineHead)}>
        <DisplayBadge severity="low">live</DisplayBadge>
        {busy && <span {...stylex.props(styles.timelineNote)}>loading delta…</span>}
        {err && (
          <span {...stylex.props(styles.timelineNote)}>
            no snapshot pair to compare — index once more to get a delta ({err})
          </span>
        )}
        {diff && (
          <span {...stylex.props(styles.timelineNote)} title={diff.provenance}>
            {diff.truncated && 'lists capped at 500 — '}
            {diff.provenance}
          </span>
        )}
      </div>
      {diff && counts && (
        <div {...stylex.props(styles.deltaBox)}>
          <div {...stylex.props(styles.deltaCounts)}>
            <span {...stylex.props(styles.deltaAdd)}>+{counts.addedEntities} entities</span>
            <span {...stylex.props(styles.deltaDel)}>−{counts.removedEntities}</span>
            <span {...stylex.props(styles.deltaSep)}>·</span>
            <span {...stylex.props(styles.deltaAdd)}>+{counts.addedRelations} relations</span>
            <span {...stylex.props(styles.deltaDel)}>−{counts.removedRelations}</span>
            <span {...stylex.props(styles.deltaSep)}>·</span>
            <span {...stylex.props(styles.deltaChanged)}>{counts.changedRelations} changed</span>
          </div>
          {lists.length === 0 && (
            <p {...stylex.props(styles.deltaEmpty)}>
              no entity/relation delta between the two snapshots
            </p>
          )}
          {lists.map((l) => {
            const isOpen = open === l.key
            return (
              <div key={l.key}>
                <button
                  type="button"
                  aria-expanded={isOpen}
                  onClick={() => setOpen(isOpen ? undefined : l.key)}
                  {...stylex.props(styles.deltaListHead)}
                >
                  <span {...stylex.props(styles.deltaCaret, isOpen && styles.deltaCaretOpen)}>
                    <IconCaretDown size={10} />
                  </span>
                  {l.label}
                </button>
                {isOpen && <div {...stylex.props(styles.deltaList)}>{l.rows}</div>}
              </div>
            )
          })}
        </div>
      )}
    </>
  )
}

function DeltaEntity({ e, onOpen }: { e: DiffEntityRef; onOpen: (path: string) => void }) {
  return (
    <div {...stylex.props(styles.deltaRow)}>
      <DisplayBadge color={false}>{e.kind}</DisplayBadge>
      <span {...stylex.props(styles.deltaName)}>
        {e.qualifiedName ?? e.name}
      </span>
      {e.path && (
        <button
          type="button"
          title={`open ${e.path}`}
          onClick={() => onOpen(e.path!)}
          {...stylex.props(styles.deltaPath)}
        >
          {e.path}
        </button>
      )}
    </div>
  )
}

function DeltaRel({ r }: { r: DiffRelation }) {
  return (
    <div {...stylex.props(styles.deltaRow)} title={`${r.extractor} · ${r.origin}`}>
      <DisplayBadge color={false}>{r.kind}</DisplayBadge>
      <span {...stylex.props(styles.deltaName)}>
        {r.source.qualifiedName ?? r.source.name}
        <span {...stylex.props(styles.deltaArrow)}> → </span>
        {r.target.qualifiedName ?? r.target.name}
      </span>
      <span {...stylex.props(styles.deltaConf)}>{Math.round(r.confidence * 100)}%</span>
    </div>
  )
}

function DeltaChanged({ r }: { r: ChangedRelation }) {
  return (
    <div {...stylex.props(styles.deltaRow)}>
      <DisplayBadge color={false}>{r.kind}</DisplayBadge>
      <span {...stylex.props(styles.deltaName)}>
        {r.source.qualifiedName ?? r.source.name}
        <span {...stylex.props(styles.deltaArrow)}> → </span>
        {r.target.qualifiedName ?? r.target.name}
      </span>
      <span {...stylex.props(styles.deltaConf)}>
        {r.baseOrigin} {Math.round(r.baseConfidence * 100)}% → {r.headOrigin}{' '}
        {Math.round(r.headConfidence * 100)}%
      </span>
    </div>
  )
}

/**
 * Export — REAL `/v1/export*`: committed row counts plus first-page JSON
 * downloads of the entity/relation/region tables (cursor pagination
 * continues past 10 000 rows via the raw endpoints).
 */
function ExportCard() {
  const [sum, setSum] = useState<ExportSummary | null>(null)
  const [err, setErr] = useState<string>()
  const [busy, setBusy] = useState(true)
  const [dl, setDl] = useState<string>()
  const [dlErr, setDlErr] = useState<string>()

  useEffect(() => {
    let dead = false
    api
      .exportSummary()
      .then((s) => !dead && setSum(s))
      .catch((e) => !dead && setErr(describeError(e)))
      .finally(() => !dead && setBusy(false))
    return () => {
      dead = true
    }
  }, [])

  const download = async (kind: 'entities' | 'relations' | 'regions') => {
    setDl(kind)
    setDlErr(undefined)
    try {
      const res = await fetch(`/v1/export/${kind}?limit=10000`)
      if (!res.ok) throw new Error(`${res.status} ${res.statusText}`)
      const blob = await res.blob()
      const a = document.createElement('a')
      a.href = URL.createObjectURL(blob)
      a.download = `cce-${kind}-${sum?.snapshotId.slice(0, 13) ?? 'snapshot'}.json`
      a.click()
      URL.revokeObjectURL(a.href)
    } catch (e) {
      setDlErr(`${kind} export failed — ${describeError(e)}`)
    } finally {
      setDl(undefined)
    }
  }

  return (
    <>
      <LayoutSeparator
        label="Export"
        aside={
          sum ? (
            <code {...stylex.props(styles.deltaPair)} title={sum.snapshotId}>
              {sum.snapshotId.slice(0, 13)}
            </code>
          ) : undefined
        }
      />
      <div {...stylex.props(styles.timelineHead)}>
        <DisplayBadge severity="low">live</DisplayBadge>
        {busy && <span {...stylex.props(styles.timelineNote)}>loading counts…</span>}
        {err && <span {...stylex.props(styles.timelineNote)}>export failed — {err}</span>}
        {sum && (
          <span {...stylex.props(styles.timelineNote)}>
            first page (≤10 000 rows) — cursor pagination via /v1/export/{'<kind>'}?cursor=
          </span>
        )}
      </div>
      {sum && (
        <div {...stylex.props(styles.deltaBox)}>
          <div {...stylex.props(styles.deltaCounts)}>
            <span {...stylex.props(styles.exportCount)}>{sum.entityCount} entities</span>
            <span {...stylex.props(styles.deltaSep)}>·</span>
            <span {...stylex.props(styles.exportCount)}>{sum.relationCount} relations</span>
            <span {...stylex.props(styles.deltaSep)}>·</span>
            <span {...stylex.props(styles.exportCount)}>{sum.regionCount} regions</span>
            <span {...stylex.props(styles.deltaSep)} />
            {(['entities', 'relations', 'regions'] as const).map((kind) => (
              <ActionButton
                key={kind}
                size="sm"
                icon={<IconDownload size={11} />}
                disabled={dl !== undefined}
                title={`download ${kind} (first page, JSON)`}
                onClick={() => void download(kind)}
              >
                {dl === kind ? 'downloading…' : kind}
              </ActionButton>
            ))}
          </div>
          {dlErr && <p {...stylex.props(styles.deltaEmpty)}>{dlErr}</p>}
          <div {...stylex.props(styles.apiRow)}>
            <a
              href="/docs"
              target="_blank"
              rel="noreferrer"
              {...stylex.props(styles.apiLink)}
            >
              OpenAPI console ↗
            </a>
            <span {...stylex.props(styles.apiNote)}>
              — the daemon's generated API reference (Swagger UI at /docs)
            </span>
          </div>
        </div>
      )}
    </>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },
  header: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 12,
    flexWrap: 'wrap',
  },
  titleGroup: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 10,
    minWidth: 0,
  },
  title: {
    margin: 0,
    fontSize: 15,
    fontWeight: 600,
  },
  snap: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
  },
  grid: {
    display: 'grid',
    gridTemplateColumns: 'repeat(auto-fill, minmax(230px, 1fr))',
    gap: 10,
  },
  viewHead: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
    marginBottom: 8,
  },
  viewName: {
    margin: 0,
    fontSize: 13,
    fontWeight: 600,
    fontFamily: font.mono,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  caps: {
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
  },
  capRow: {
    display: 'flex',
    alignItems: 'baseline',
    justifyContent: 'space-between',
    gap: 8,
    fontSize: 11,
  },
  capName: {
    color: vars.colorMuted,
  },
  capLevel: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  viewMsg: {
    margin: '8px 0 0',
    fontSize: 11,
    color: vars.colorMuted,
    overflowWrap: 'anywhere',
  },
  viewFoot: {
    marginTop: 8,
    paddingTop: 8,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 10,
    fontFamily: font.mono,
  },
  providerTool: {
    margin: 0,
    display: 'flex',
    alignItems: 'baseline',
    gap: 6,
    fontSize: 11,
  },
  providerToolCode: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorBase,
    overflowWrap: 'anywhere',
  },
  providerStats: {
    marginTop: 8,
    display: 'flex',
    flexWrap: 'wrap',
    gap: 10,
  },
  stat: {
    fontSize: 11,
    color: vars.colorMuted,
  },
  timelineHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
  },
  timelineNote: {
    fontSize: 11,
    color: vars.colorFaint,
  },
  timeline: {
    margin: 0,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  timelineRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 12,
    paddingRight: 12,
    borderTopWidth: {
      default: 1,
      ':first-child': 0,
    },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 12,
  },
  timelineIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  timelineSha: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorActive,
    flexShrink: 0,
  },
  timelineMsg: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    color: vars.colorBase,
  },
  timelineViews: {
    display: 'inline-flex',
    gap: 4,
    flexShrink: 0,
  },
  timelineWhen: {
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    width: 76,
    textAlign: 'right',
  },

  // ── Telemetry band ──
  teleRow: {
    display: 'flex',
    alignItems: 'stretch',
    gap: 18,
    flexWrap: { default: 'nowrap', '@media (max-width: 980px)': 'wrap' },
  },
  teleCol: {
    flexGrow: 0,
    flexShrink: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
  },
  teleSep: {
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
  chartFrame: {
    width: '100%',
    height: 132,
  },
  chart: {
    display: 'block',
  },
  latencyStats: {
    display: 'flex',
    gap: 14,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  latencyStat: {
    display: 'inline-flex',
    alignItems: 'baseline',
    gap: 5,
  },
  latencyStatLabel: {
    fontSize: 9,
    textTransform: 'uppercase',
    letterSpacing: '0.05em',
    color: vars.colorFaint,
  },
  matrix: {
    display: 'flex',
    flexDirection: 'column',
  },
  matrixHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    paddingBottom: 6,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  matrixRouteHead: {
    width: 108,
    flexShrink: 0,
  },
  matrixColHead: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 8.5,
    color: vars.colorFaint,
    textAlign: 'center',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  matrixStateHead: {
    flex: '0 0 64px',
    textAlign: 'right',
  },
  matrixRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    paddingTop: 6,
    paddingBottom: 6,
    borderBottomWidth: { default: 1, ':last-child': 0 },
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  matrixRoute: {
    width: 108,
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  matrixCell: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    justifyContent: 'center',
  },
  matrixDot: {
    width: 9,
    height: 9,
    borderRadius: '50%',
    flexShrink: 0,
  },
  matrixEmpty: {
    width: 9,
    height: 2,
    borderRadius: 1,
    backgroundColor: vars.borderMute,
    flexShrink: 0,
  },
  matrixState: {
    flex: '0 0 64px',
    fontFamily: font.mono,
    fontSize: 9.5,
    textAlign: 'right',
    whiteSpace: 'nowrap',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
  },
  mixWrap: {
    display: 'flex',
    alignItems: 'center',
    gap: 14,
    minHeight: 132,
  },
  mixLegend: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
  },
  mixRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 7,
    fontSize: 11.5,
  },
  mixDot: {
    width: 7,
    height: 7,
    borderRadius: 2,
    flexShrink: 0,
  },
  mixName: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorBase,
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  mixPct: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    fontVariantNumeric: 'tabular-nums',
    flexShrink: 0,
  },

  // ── Query log ──
  logList: {
    display: 'flex',
    flexDirection: 'column',
  },
  logRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 5,
    paddingBottom: 5,
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
  logQuery: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  logMeta: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    flexShrink: 0,
    width: 64,
    textAlign: 'right',
  },
  logMs: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    flexShrink: 0,
    width: 52,
    textAlign: 'right',
    fontVariantNumeric: 'tabular-nums',
  },
  logWhen: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    flexShrink: 0,
    width: 44,
    textAlign: 'right',
  },

  // ── Index jobs ──
  jobSpacer: {
    flex: 1,
  },
  jobChips: {
    display: 'flex',
    alignItems: 'center',
    gap: 4,
    flexWrap: 'wrap',
  },
  jobChip: {
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
  jobChipOn: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
    borderColor: 'transparent',
  },
  jobChipCount: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  jobDot: {
    display: 'inline-block',
    width: 7,
    height: 7,
    borderRadius: '50%',
    flexShrink: 0,
  },
  jobDotQueued: {
    backgroundColor: vars.colorFaint,
  },
  jobDotRunning: {
    backgroundColor: vars.scaleMedium,
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': breathe,
    },
    animationDuration: '1.4s',
    animationTimingFunction: 'ease-in-out',
    animationIterationCount: 'infinite',
    animationDirection: 'alternate',
  },
  jobDotDone: {
    backgroundColor: vars.scaleLow,
  },
  jobDotFailed: {
    backgroundColor: vars.scaleCritical,
  },
  jobNote: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  jobItem: {
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  jobRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    width: '100%',
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 12,
    paddingRight: 12,
    borderWidth: 0,
    backgroundColor: { default: 'transparent', ':hover': vars.bgHover },
    fontFamily: 'inherit',
    fontSize: 12,
    color: vars.colorBase,
    textAlign: 'left',
    cursor: 'pointer',
  },
  jobCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '120ms',
  },
  jobCaretOpen: {
    transform: 'rotate(0deg)',
  },
  jobCheck: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
  },
  nodeSvg: {
    display: 'block',
    flexShrink: 0,
  },
  nodeDoneRing: {
    fill: vars.tipSuccessBg,
    stroke: vars.scaleLow,
    strokeWidth: 1.2,
  },
  nodeCheck: {
    fill: 'none',
    stroke: vars.scaleLow,
    strokeWidth: 1.6,
    strokeLinecap: 'round',
    strokeLinejoin: 'round',
    strokeDasharray: 10,
    strokeDashoffset: 0,
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': drawIn,
    },
    animationDuration: '0.35s',
    animationTimingFunction: 'ease-out',
    animationFillMode: 'backwards',
  },
  nodeTrack: {
    fill: 'none',
    stroke: vars.borderBase,
    strokeWidth: 1.3,
  },
  nodeArc: {
    fill: 'none',
    stroke: vars.scaleMedium,
    strokeWidth: 1.7,
    strokeLinecap: 'round',
    transformBox: 'view-box',
    transformOrigin: 'center',
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': spin,
    },
    animationDuration: '0.85s',
    animationTimingFunction: 'linear',
    animationIterationCount: 'infinite',
  },
  nodeCore: {
    fill: vars.scaleMedium,
    transformBox: 'fill-box',
    transformOrigin: 'center',
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': breathe,
    },
    animationDuration: '1.4s',
    animationTimingFunction: 'ease-in-out',
    animationIterationCount: 'infinite',
    animationDirection: 'alternate',
  },
  nodePendingRing: {
    fill: 'none',
    stroke: vars.borderBase,
    strokeWidth: 1.2,
  },
  nodePendingDot: {
    fill: vars.colorFaint,
  },
  nodeFailedRing: {
    fill: vars.tipErrorBg,
    stroke: vars.scaleCritical,
    strokeWidth: 1.2,
  },
  nodeCross: {
    fill: 'none',
    stroke: vars.scaleCritical,
    strokeWidth: 1.6,
    strokeLinecap: 'round',
    strokeDasharray: 10,
    strokeDashoffset: 0,
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': drawIn,
    },
    animationDuration: '0.3s',
    animationTimingFunction: 'ease-out',
    animationFillMode: 'backwards',
  },
  jobRepo: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  jobWorker: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    width: 64,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  jobStage: {
    fontFamily: font.mono,
    fontSize: 10,
    flexShrink: 0,
    width: 52,
    textAlign: 'right',
    whiteSpace: 'nowrap',
  },
  jobTextQueued: {
    color: vars.colorFaint,
  },
  jobTextRunning: {
    color: vars.scaleMedium,
  },
  jobTextDone: {
    color: vars.colorMuted,
  },
  jobTextFailed: {
    color: vars.scaleCritical,
  },
  jobDuration: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorMuted,
    flexShrink: 0,
    width: 56,
    textAlign: 'right',
  },
  jobWhen: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    width: 76,
    textAlign: 'right',
  },
  jobProgress: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingLeft: 34,
    paddingRight: 12,
    paddingBottom: 7,
  },
  jobPct: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 9.5,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  jobDetail: {
    marginLeft: 34,
    marginBottom: 8,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    paddingRight: 10,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderBase,
    backgroundColor: vars.bgSunken,
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': riseIn,
    },
    animationDuration: '0.18s',
    animationTimingFunction: 'ease-out',
  },
  jobSteps: {
    display: 'flex',
    alignItems: 'flex-start',
    paddingTop: 2,
    paddingBottom: 2,
  },
  jobStep: {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    gap: 5,
    flexShrink: 0,
  },
  jobStepLink: {
    width: 36,
    height: 1,
    marginTop: 10,
    flexShrink: 0,
  },
  jobStepLinkDone: {
    backgroundColor: vars.scaleLow,
    opacity: 0.4,
  },
  jobStepLinkFlow: {
    backgroundImage: `repeating-linear-gradient(90deg, ${vars.scaleMedium}, ${vars.scaleMedium} 4px, transparent 4px, transparent 9px)`,
    opacity: 0.8,
    animationName: {
      default: 'none',
      '@media (prefers-reduced-motion: no-preference)': flow,
    },
    animationDuration: '0.7s',
    animationTimingFunction: 'linear',
    animationIterationCount: 'infinite',
  },
  jobStepLinkFailed: {
    backgroundColor: vars.scaleCritical,
    opacity: 0.4,
  },
  jobStepLinkPending: {
    backgroundColor: vars.borderMute,
  },
  jobStepName: {
    fontFamily: font.mono,
    fontSize: 9.5,
    letterSpacing: '0.02em',
    lineHeight: '1',
  },
  jobStepNameDone: {
    color: vars.colorMuted,
  },
  jobStepNameRunning: {
    color: vars.scaleMedium,
  },
  jobStepNamePending: {
    color: vars.colorFaint,
  },
  jobStepNameFailed: {
    color: vars.scaleCritical,
  },
  jobError: {
    margin: 0,
    fontFamily: font.mono,
    fontSize: 10.5,
    lineHeight: '1.5',
    color: vars.accentError,
    backgroundColor: vars.tipErrorBg,
    borderRadius: 6,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 8,
    paddingRight: 8,
    whiteSpace: 'pre-wrap',
    overflowWrap: 'anywhere',
  },
  jobActions: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
  },
  deltaPair: {
    fontFamily: font.mono,
    fontSize: 10.5,
    color: vars.colorMuted,
  },
  deltaBox: {
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    overflow: 'hidden',
  },
  deltaCounts: {
    display: 'flex',
    alignItems: 'center',
    flexWrap: 'wrap',
    gap: 8,
    paddingTop: 9,
    paddingBottom: 9,
    paddingLeft: 12,
    paddingRight: 12,
    fontSize: 11.5,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
  },
  deltaAdd: {
    color: vars.scaleLow,
    fontFamily: font.mono,
  },
  deltaDel: {
    color: vars.scaleHigh,
    fontFamily: font.mono,
  },
  deltaChanged: {
    color: vars.colorMuted,
    fontFamily: font.mono,
  },
  deltaSep: {
    color: vars.colorFaint,
  },
  deltaEmpty: {
    margin: 0,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    fontSize: 11,
    color: vars.colorFaint,
  },
  apiRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 9,
    paddingBottom: 10,
    paddingLeft: 12,
    paddingRight: 12,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  apiLink: {
    fontSize: 11,
    fontFamily: font.mono,
    color: vars.accentInfo,
    textDecoration: 'none',
  },
  apiNote: {
    fontSize: 10.5,
    color: vars.colorFaint,
  },
  deltaListHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorMuted,
    fontSize: 11,
    fontFamily: font.mono,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 12,
    paddingRight: 12,
    cursor: 'pointer',
    textAlign: 'left',
  },
  deltaCaret: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transform: 'rotate(-90deg)',
    transitionProperty: 'transform',
    transitionDuration: '0.15s',
  },
  deltaCaretOpen: {
    transform: 'rotate(0deg)',
  },
  deltaList: {
    display: 'flex',
    flexDirection: 'column',
    paddingBottom: 6,
  },
  deltaRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 30,
    paddingRight: 12,
    fontSize: 11,
  },
  deltaName: {
    fontFamily: font.mono,
    color: vars.colorBase,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  deltaArrow: {
    color: vars.colorFaint,
  },
  deltaConf: {
    marginLeft: 'auto',
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  deltaPath: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    padding: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    cursor: 'pointer',
    textDecorationLine: {
      default: 'none',
      ':hover': 'underline',
    },
    flexShrink: 0,
  },
  exportCount: {
    fontFamily: font.mono,
    color: vars.colorBase,
    fontSize: 11.5,
  },
})

/** Chip-dot color per queue stage. */
const JOB_DOT_STYLE = {
  queued: styles.jobDotQueued,
  running: styles.jobDotRunning,
  done: styles.jobDotDone,
  failed: styles.jobDotFailed,
}

/** Row stage-text color per queue stage. */
const JOB_TEXT_STYLE = {
  queued: styles.jobTextQueued,
  running: styles.jobTextRunning,
  done: styles.jobTextDone,
  failed: styles.jobTextFailed,
}

/** Stepper connector per the state it leads into — trail / flow / dead-end. */
const STEP_LINK_STYLE = {
  done: styles.jobStepLinkDone,
  running: styles.jobStepLinkFlow,
  pending: styles.jobStepLinkPending,
  failed: styles.jobStepLinkFailed,
}

/** Stepper label color per derived step state — the node carries severity. */
const STEP_NAME_STYLE = {
  done: styles.jobStepNameDone,
  running: styles.jobStepNameRunning,
  pending: styles.jobStepNamePending,
  failed: styles.jobStepNameFailed,
}