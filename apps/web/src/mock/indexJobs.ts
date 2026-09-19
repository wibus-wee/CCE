/**
 * MOCK — fixture index jobs for the Index screen's job queue (the
 * Sourcegraph site-admin repo-updater queue idiom adapted to CCE's
 * indexing pipeline). No `/v1/jobs` endpoint exists and no worker
 * registry reports yet, so every consumer must label the surface
 * `preview` — job kinds, stages, workers, durations and errors are
 * fixtures, never live scheduler truth.
 *
 * When the endpoint lands, delete this file and switch the section to
 * the API; the shapes mirror the intended response. Timestamps
 * deliberately interleave with `MOCK_TIMELINE` pushes (src/mock/index.ts)
 * — each reindex trails the push that invalidated its views, so the
 * mocked surfaces tell one coherent story.
 */

/**
 * Work the scheduler can dispatch — a full `index` of a never-indexed
 * root, a delta `reindex` after a push, or the maintenance passes
 * (`pack` artifact writes, `embed` backfills, `compact` store sweeps).
 */
export type MockIndexJobKind = 'index' | 'reindex' | 'pack' | 'embed' | 'compact'

/** Queue lifecycle — `queued` → `running` → `done` | `failed`. */
export type MockIndexJobStage = 'queued' | 'running' | 'done' | 'failed'

export interface MockIndexJob {
  id: string
  kind: MockIndexJobKind
  /** Workspace root inside the indexed checkout the job targets. */
  repo: string
  stage: MockIndexJobStage
  /**
   * Enqueue time while queued — the queue surfaces it as "queued X
   * ago"; once a worker claims the job it becomes the start time.
   */
  startedAt?: string
  /** Terminal timestamp — absent while queued/running. */
  finishedAt?: string
  /** Wall-clock run time for terminal jobs. */
  durationMs?: number
  /** 0–1 progress reported by the worker — only while running. */
  progress?: number
  /** Terminal failure reason — names the pipeline stage it died in. */
  error?: string
  /** Owning worker — pre-assigned by the scheduler while queued. */
  worker: string
}

/**
 * The queue a small daemon actually carries: two workers mid-run, two
 * jobs waiting behind them, then the terminal history — newest first —
 * with one `pack` dead on a checksum mismatch and one `embed` dead on
 * the ONNX model fetch (the known cold-start failure mode).
 */
export const MOCK_INDEX_JOBS: MockIndexJob[] = [
  // ── Live queue ─────────────────────────────────────────────────────
  {
    id: 'idx-4821',
    kind: 'reindex',
    repo: 'apps/web',
    stage: 'running',
    startedAt: '2026-09-18T09:13:05Z',
    progress: 0.71,
    worker: 'worker-1',
  },
  {
    id: 'idx-4822',
    kind: 'embed',
    repo: 'crates/cce-engine',
    stage: 'running',
    startedAt: '2026-09-18T09:13:38Z',
    progress: 0.34,
    worker: 'worker-2',
  },
  {
    id: 'idx-4823',
    kind: 'index',
    repo: 'research/cce_research',
    stage: 'queued',
    startedAt: '2026-09-18T09:14:20Z',
    worker: 'worker-0',
  },
  {
    id: 'idx-4824',
    kind: 'compact',
    repo: 'apps/daemon',
    stage: 'queued',
    startedAt: '2026-09-18T09:15:02Z',
    worker: 'worker-1',
  },
  // ── Terminal history ───────────────────────────────────────────────
  {
    id: 'idx-4820',
    kind: 'pack',
    repo: 'crates/cce-engine',
    stage: 'failed',
    startedAt: '2026-09-18T09:02:11Z',
    finishedAt: '2026-09-18T09:04:37Z',
    durationMs: 146_000,
    error: 'artifact store: checksum mismatch on pack 7/12',
    worker: 'worker-0',
  },
  {
    id: 'idx-4819',
    kind: 'reindex',
    repo: 'apps/web',
    stage: 'done',
    startedAt: '2026-09-18T08:57:44Z',
    finishedAt: '2026-09-18T08:57:56Z',
    durationMs: 12_400,
    worker: 'worker-2',
  },
  {
    id: 'idx-4818',
    kind: 'reindex',
    repo: 'crates/cce-engine',
    stage: 'done',
    startedAt: '2026-09-17T22:43:10Z',
    finishedAt: '2026-09-17T22:46:55Z',
    durationMs: 225_000,
    worker: 'worker-1',
  },
  {
    id: 'idx-4817',
    kind: 'embed',
    repo: 'apps/daemon',
    stage: 'done',
    startedAt: '2026-09-17T18:06:32Z',
    finishedAt: '2026-09-17T18:08:44Z',
    durationMs: 132_000,
    worker: 'worker-2',
  },
  {
    id: 'idx-4816',
    kind: 'embed',
    repo: 'apps/web',
    stage: 'failed',
    startedAt: '2026-09-17T11:30:08Z',
    finishedAt: '2026-09-17T11:30:19Z',
    durationMs: 11_000,
    error: 'dense backend: ONNX model fetch timed out during embed',
    worker: 'worker-0',
  },
  {
    id: 'idx-4815',
    kind: 'pack',
    repo: 'apps/cli',
    stage: 'done',
    startedAt: '2026-09-16T15:31:47Z',
    finishedAt: '2026-09-16T15:36:02Z',
    durationMs: 255_000,
    worker: 'worker-1',
  },
]
