/**
 * MOCK — fixtures for the Insights surface (Sourcegraph Code Insights
 * analog): saved query series charted over time and grouped onto
 * dashboards. No `/v1/insights` endpoint exists, so every consumer must
 * label the surface `preview` — these are generated shapes, never
 * measured index truth.
 *
 * The shapes mirror the intended API: a dashboard references insight ids;
 * an insight owns its query, repo scope, sampling interval and one or
 * more named series of {at, value} points. When the endpoint lands,
 * delete this file and switch the screen to the API.
 */

export interface MockInsightPoint {
  /** ISO date of the sample — one per `intervalDays` step. */
  at: string
  value: number
}

export interface MockInsightSeries {
  /** Legend label — an insight may chart several query variants at once. */
  name: string
  points: MockInsightPoint[]
}

export interface MockInsight {
  id: string
  title: string
  /** The Sourcegraph-style query re-run once per interval. */
  query: string
  /** Repo scope filters resolved when the insight was created. */
  repos: string[]
  /** Value unit for legends, axes and the data table. */
  unit: string
  /** Days between samples — the insight's capture interval. */
  intervalDays: number
  series: MockInsightSeries[]
  /** One-line analyst note, like the insight's own description field. */
  note: string
}

export interface MockInsightDashboard {
  id: string
  title: string
  /** Insight ids pinned to this dashboard. */
  insights: string[]
}

const DAY_MS = 86400_000
const daysAgo = (d: number) => new Date(Date.now() - d * DAY_MS).toISOString().slice(0, 10)

/**
 * Deterministic point run — a base line plus linear drift and a fixed
 * sinusoidal wobble so each series has an honest-looking shape without
 * pulling in a random seed. `wobble` is the noise amplitude as a
 * fraction of `base`; pass 0 for a flat line.
 */
function mkPoints(
  intervalDays: number,
  count: number,
  base: number,
  drift: number,
  wobble = 0.12,
): MockInsightPoint[] {
  return Array.from({ length: count }, (_, i) => ({
    at: daysAgo((count - 1 - i) * intervalDays),
    value: Math.max(
      0,
      Math.round(base + drift * i + Math.sin(i * 1.7 + base * 0.37) * base * wobble),
    ),
  }))
}

type SeriesSpec = [name: string, base: number, drift: number, wobble?: number]

function mkInsight(
  id: string,
  title: string,
  query: string,
  unit: string,
  intervalDays: number,
  count: number,
  repos: string[],
  specs: SeriesSpec[],
  note: string,
): MockInsight {
  return {
    id,
    title,
    query,
    unit,
    intervalDays,
    repos,
    note,
    series: specs.map(([name, base, drift, wobble]) => ({
      name,
      points: mkPoints(intervalDays, count, base, drift, wobble),
    })),
  }
}

/**
 * Ten insights over three dashboards — code hygiene markers, retrieval
 * telemetry and index operations. Series counts (2–3), point counts
 * (10–14) and intervals vary so the window filter has something to bite.
 */
export const MOCK_INSIGHTS: MockInsight[] = [
  // --- code health ------------------------------------------------------
  mkInsight('ch-todo', 'TODO / FIXME density', 'TODO|FIXME|HACK', 'markers', 7, 13, ['cce'], [
    ['TODO', 34, -1.4],
    ['FIXME', 19, -0.7],
    ['HACK', 8, -0.3],
  ], 'declining — cleanup batch change working'),
  mkInsight('ch-unsafe', 'unsafe_code lint state', 'unsafe|allow(unsafe) lang:rust', 'lint hits', 7, 12, ['cce'], [
    ['unsafe blocks', 0, 0, 0],
    ['suppression attrs', 2, 0.08, 0.5],
  ], 'unsafe stays zero — the contract forbids unsafe_code'),
  mkInsight('ch-v0', 'Deprecated /v0 surface', 'path:api /v0/', 'hits', 7, 11, ['cce'], [
    ['handlers', 9, -0.6],
    ['call sites', 24, -1.6],
  ], 'removal batch change draining the old endpoints'),
  mkInsight('ch-skip', 'Skipped tests', '#[ignore]|.skip(', 'tests', 7, 12, ['cce'], [
    ['rust #[ignore]', 5, 0.18],
    ['web *.skip', 7, 0.32],
  ], 'creeping up — quarantine list needs triage'),

  // --- retrieval ----------------------------------------------------------
  mkInsight('rt-routes', 'Route share', 'type:query select:route', 'queries / interval', 2, 14, ['cce', 'gold-corpus'], [
    ['lexical', 34, -0.5],
    ['dense_summary', 13, 1.0],
    ['hybrid', 9, 0.7],
  ], 'dense share growing as embeddings warm up'),
  mkInsight('rt-rerank', 'Late-fusion coverage', 'rerank:(fused|raw)', '% of queries', 7, 11, ['cce', 'gold-corpus'], [
    ['fused', 36, 2.4],
    ['raw route', 64, -2.2],
  ], 'reranker now covers most multi-hit queries'),
  mkInsight('rt-zero', 'Zero-hit queries', 'hits:0', 'queries', 2, 12, ['cce'], [
    ['zero hits', 9, -0.4],
    ['reformulated', 4, 0.25],
  ], 'empty sets falling; more users retry than abandon'),

  // --- indexing -----------------------------------------------------------
  mkInsight('ix-fresh', 'View freshness lag', 'view:(stale|rebuilding)', 'views', 7, 12, ['cce'], [
    ['stale views', 6, -0.35],
    ['rebuilding', 2, 0.1],
  ], 'leases shorten rebuild windows after each push'),
  mkInsight('ix-lease', 'Lease contention', 'lease:(held|waiting)', 'tasks', 2, 13, ['cce'], [
    ['leases held', 3, 0.08],
    ['waiters', 5, -0.25],
  ], 'queue drains faster since the epoch fence landed'),
  mkInsight('ix-store', 'Artifact store growth', 'store:(artifact|vector)', 'k objects', 7, 11, ['cce'], [
    ['artifacts', 41, 1.9],
    ['vectors', 18, 1.3],
  ], 'steady growth — content-addressed, so dedup keeps it linear'),
]

export const MOCK_INSIGHT_DASHBOARDS: MockInsightDashboard[] = [
  { id: 'dash-health', title: 'code health', insights: ['ch-todo', 'ch-unsafe', 'ch-v0', 'ch-skip'] },
  { id: 'dash-retrieval', title: 'retrieval', insights: ['rt-routes', 'rt-rerank', 'rt-zero'] },
  { id: 'dash-indexing', title: 'indexing', insights: ['ix-fresh', 'ix-lease', 'ix-store'] },
]
