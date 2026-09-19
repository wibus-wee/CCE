/**
 * MOCK — fixture code monitors for the Code Monitoring surface
 * (Sourcegraph's diff/commit watchers). No `/v1/monitors` endpoint exists
 * yet, so every consumer must label the surface `preview` — these monitors
 * never run and their event logs are staged, not live index truth.
 *
 * The shape mirrors Sourcegraph's monitor model: a `trigger` (a `type:diff`
 * or `type:commit` query re-run over every new commit), `actions` (email /
 * slack / webhook / log deliveries fired per trigger event), and an
 * `events` log with one entry per run — the commit that produced new
 * results plus the lines that matched. Each action also carries a staged
 * `deliveries` history — sent/failed attempts tied back to their event.
 *
 * When the endpoint lands, delete this file and switch the screen to the
 * API. (The flat `MOCK_MONITORS` in `./index` keeps the legacy row shape;
 * this file carries the full feature surface.)
 */

export type MockMonitorTriggerKind = 'diff' | 'commit'

export interface MockMonitorTrigger {
  /**
   * Which `type:` the query runs as — `diff` events carry unified-diff
   * matched lines, `commit` events carry matched message lines.
   */
  kind: MockMonitorTriggerKind
  /** The search query re-run over every new commit. */
  query: string
}

export type MockMonitorActionType = 'email' | 'slack' | 'webhook' | 'log'

export type MockMonitorDeliveryStatus = 'sent' | 'failed'

export interface MockMonitorDelivery {
  /** ISO timestamp of the delivery attempt. */
  at: string
  status: MockMonitorDeliveryStatus
  /**
   * One-line receipt or failure reason — ties back to the event that
   * triggered it (`ev-*`), rendered in mono.
   */
  detail: string
}

export interface MockMonitorAction {
  type: MockMonitorActionType
  /** Delivery target — an address, a channel, a POST URL, or a stream. */
  target: string
  /** Newest-first staged delivery history for this action. */
  deliveries: MockMonitorDelivery[]
}

export interface MockMonitorEvent {
  id: string
  /** ISO timestamp of the trigger run. */
  at: string
  /** Abbreviated sha of the commit that produced new results. */
  commitSha: string
  /** Commit subject line. */
  message: string
  /**
   * Lines that matched — unified-diff lines (`+` / `-` / space prefixes)
   * for `type:diff` monitors, commit-message lines for `type:commit`.
   */
  matchedLines: string[]
  /** Commit author handle. */
  author: string
}

export interface MockMonitor {
  id: string
  /** Display name — travels in notification subjects, so it stays leak-safe. */
  name: string
  description: string
  /** Handle of the user who created the monitor. */
  owner: string
  /** Whether the trigger is re-run on each push. */
  enabled: boolean
  trigger: MockMonitorTrigger
  /** Deliveries fired per trigger event, in delivery order. */
  actions: MockMonitorAction[]
  /** Newest-first run history — `events[0]` is the last trigger. */
  events: MockMonitorEvent[]
}

const hoursAgo = (h: number) => new Date(Date.now() - h * 3600_000).toISOString()

/**
 * The watch set a repo like this one would actually carry: a contract guard
 * (`unsafe` is forbidden workspace-wide), a debt-intake tracker, a secrets
 * tripwire, a paused deprecation watch, and a `type:commit` release audit —
 * event shas reuse the timeline/branch fixtures where the commit lines up.
 */
export const MOCK_MONITORS: MockMonitor[] = [
  {
    id: 'mon-unsafe',
    name: 'Unsafe code watch',
    description:
      'Any `unsafe` landing in the engine — the workspace contract forbids `unsafe_code`, so hits should only ever appear on reverted drafts and docs.',
    owner: 'wibus',
    enabled: true,
    trigger: {
      kind: 'diff',
      query: 'type:diff repo:^cce$ select:commit.diff.added unsafe',
    },
    actions: [
      {
        type: 'slack',
        target: '#cce-alerts',
        deliveries: [
          { at: hoursAgo(9), status: 'sent', detail: 'ev-unsafe-3 · posted to channel' },
          { at: hoursAgo(33), status: 'sent', detail: 'ev-unsafe-2 · posted to channel' },
          {
            at: hoursAgo(120),
            status: 'failed',
            detail: 'ev-unsafe-1 · rate-limited — dropped after 3 retries',
          },
        ],
      },
      {
        type: 'email',
        target: 'wibus@localhost',
        deliveries: [
          { at: hoursAgo(9), status: 'sent', detail: 'ev-unsafe-3 · smtp 250 queued' },
          {
            at: hoursAgo(33),
            status: 'failed',
            detail: 'ev-unsafe-2 · smtp 451 relay busy — retry pending',
          },
          { at: hoursAgo(120), status: 'sent', detail: 'ev-unsafe-1 · smtp 250 queued' },
        ],
      },
    ],
    events: [
      {
        id: 'ev-unsafe-3',
        at: hoursAgo(9),
        commitSha: 'd91c6a4e',
        message: 'engine: LeaseCell — unsafe impl for Send (draft branch)',
        author: 'devin',
        matchedLines: [
          '+ unsafe impl Send for LeaseCell {}',
          '+ unsafe impl Sync for LeaseCell {}',
        ],
      },
      {
        id: 'ev-unsafe-2',
        at: hoursAgo(33),
        commitSha: '3f9c1d55',
        message: 'engine: mmap read path — unsafe slice over artifact pages',
        author: 'devin',
        matchedLines: [
          '+     let buf = unsafe { slice::from_raw_parts(page.as_ptr(), page.len()) };',
        ],
      },
      {
        id: 'ev-unsafe-1',
        at: hoursAgo(120),
        commitSha: '7a44e2c9',
        message: 'docs: safety contract — spell out the unsafe_code lint',
        author: 'wibus',
        matchedLines: [
          '+ Every Rust crate inherits `unsafe_code = "forbid"`; do not add `unsafe`',
          '+ blocks, declarations, or unsafe traits anywhere in the workspace.',
        ],
      },
    ],
  },
  {
    id: 'mon-todos',
    name: 'TODO / FIXME intake',
    description:
      'New TODO and FIXME markers added under crates/ — tracks debt intake while the cleanup batch change drains the backlog.',
    owner: 'wibus',
    enabled: true,
    trigger: {
      kind: 'diff',
      query: 'type:diff file:crates/ patternType:keyword (TODO|FIXME)',
    },
    actions: [
      {
        type: 'log',
        target: 'monitor-events',
        deliveries: [
          { at: hoursAgo(4), status: 'sent', detail: 'ev-todos-6 · appended' },
          { at: hoursAgo(14), status: 'sent', detail: 'ev-todos-5 · appended' },
          { at: hoursAgo(30), status: 'sent', detail: 'ev-todos-4 · appended' },
          { at: hoursAgo(52), status: 'sent', detail: 'ev-todos-3 · appended' },
          { at: hoursAgo(96), status: 'sent', detail: 'ev-todos-2 · appended' },
        ],
      },
    ],
    events: [
      {
        id: 'ev-todos-6',
        at: hoursAgo(4),
        commitSha: 'c5e99258',
        message: 'engine: late-fusion rerank over dense+lexical',
        author: 'wibus',
        matchedLines: [
          '+ // TODO(wibus): calibrate fusion weights once the gold corpus is relabeled',
          '+ // FIXME: dense leg drops short queries — floor the score instead',
        ],
      },
      {
        id: 'ev-todos-5',
        at: hoursAgo(14),
        commitSha: '9d1b42ea',
        message: 'daemon: lease-guard concurrent index writes',
        author: 'wibus',
        matchedLines: ['+ // TODO: wake waiters in epoch order, not registration order'],
      },
      {
        id: 'ev-todos-4',
        at: hoursAgo(30),
        commitSha: '77ac0d21',
        message: 'packing: deterministic budget split for context packs',
        author: 'wibus',
        matchedLines: ['+ // TODO: charge provenance headers against the pack budget'],
      },
      {
        id: 'ev-todos-3',
        at: hoursAgo(52),
        commitSha: 'e3f08b90',
        message: 'scip: emit reference edges for trait impls',
        author: 'wibus',
        matchedLines: [
          '+ // FIXME: dedupe edges when an impl is re-indexed under a new snapshot',
          '+ // TODO: emit enclosing_range once scip-rs publishes it',
        ],
      },
      {
        id: 'ev-todos-2',
        at: hoursAgo(96),
        commitSha: '6f3d8b2c',
        message: 'dataflow: propagate taint through call edges',
        author: 'devin',
        matchedLines: ['+ // TODO(devin): sink allowlist should live in policy, not the pass'],
      },
      {
        id: 'ev-todos-1',
        at: hoursAgo(150),
        commitSha: '42b6e1f7',
        message: 'web: redesign — StyleX token layer, dark parity',
        author: 'wibus',
        matchedLines: ['+ // TODO: drop the legacy .css files once Browse is ported'],
      },
    ],
  },
  {
    id: 'mon-secrets',
    name: 'Secret material guard',
    description:
      'Keys, tokens and private material in any diff — a tripwire that fires before a leak can reach the canonical snapshot.',
    owner: 'wibus',
    enabled: true,
    trigger: {
      kind: 'diff',
      query: 'type:diff patternType:regexp (api[_-]?key|secret|private[_-]?key|token)\\s*[:=]',
    },
    actions: [
      {
        type: 'slack',
        target: '#cce-security',
        deliveries: [
          { at: hoursAgo(21), status: 'sent', detail: 'ev-secrets-4 · posted to channel' },
          { at: hoursAgo(48), status: 'sent', detail: 'ev-secrets-3 · posted to channel' },
          {
            at: hoursAgo(110),
            status: 'failed',
            detail: 'ev-secrets-2 · socket hangup — retried 3×',
          },
          { at: hoursAgo(170), status: 'sent', detail: 'ev-secrets-1 · posted to channel' },
        ],
      },
      {
        type: 'webhook',
        target: 'https://hooks.local/monitors/secrets',
        deliveries: [
          { at: hoursAgo(21), status: 'sent', detail: 'ev-secrets-4 · 200 OK · 84 ms' },
          { at: hoursAgo(48), status: 'sent', detail: 'ev-secrets-3 · 200 OK · 121 ms' },
          { at: hoursAgo(110), status: 'sent', detail: 'ev-secrets-2 · 200 OK · 97 ms' },
          {
            at: hoursAgo(170),
            status: 'failed',
            detail: 'ev-secrets-1 · 502 after 3 retries — dropped',
          },
        ],
      },
    ],
    events: [
      {
        id: 'ev-secrets-4',
        at: hoursAgo(21),
        commitSha: 'd2f7a913',
        message: 'research: eval harness — local provider keys for offline runs',
        author: 'devin',
        matchedLines: [
          '+ OPENAI_API_KEY = "sk-test-4f2c…"',
          '+ os.environ.setdefault("CCE_API_TOKEN", "dev-token")',
        ],
      },
      {
        id: 'ev-secrets-3',
        at: hoursAgo(48),
        commitSha: '5b8d0e34',
        message: 'daemon: accept bearer token for the admin socket',
        author: 'wibus',
        matchedLines: ['+ let token: &str = "cce-admin-local"; // replaced by config on boot'],
      },
      {
        id: 'ev-secrets-2',
        at: hoursAgo(110),
        commitSha: 'f9e2b6a0',
        message: 'web: dev proxy — embed staging api key for previews',
        author: 'devin',
        matchedLines: ["+ const api_key = 'staging-9a3b-x7' // dev proxy only"],
      },
      {
        id: 'ev-secrets-1',
        at: hoursAgo(170),
        commitSha: '1c6d4f88',
        message: 'docs: example .env for the webhook receiver',
        author: 'wibus',
        matchedLines: [
          '+ SLACK_WEBHOOK_URL=https://hooks.slack.com/services/T000/B000/XXXX',
        ],
      },
    ],
  },
  {
    id: 'mon-v0',
    name: 'Deprecated /v0 consumers',
    description:
      'New fetch() callers hitting the deprecated /v0 routes — paused while the endpoint-removal batch change finishes the sweep.',
    owner: 'wibus',
    enabled: false,
    trigger: {
      kind: 'diff',
      query: "type:diff file:\\.tsx?$ fetch\\(['\"`]/v0/",
    },
    actions: [
      {
        type: 'webhook',
        target: 'https://hooks.local/monitors/v0-watch',
        deliveries: [
          { at: hoursAgo(140), status: 'sent', detail: 'ev-v0-5 · 200 OK · 132 ms' },
          { at: hoursAgo(190), status: 'sent', detail: 'ev-v0-4 · 200 OK · 88 ms' },
          {
            at: hoursAgo(240),
            status: 'failed',
            detail: 'ev-v0-3 · timeout after 10 s — retried once',
          },
          { at: hoursAgo(300), status: 'sent', detail: 'ev-v0-2 · 200 OK · 104 ms' },
        ],
      },
    ],
    events: [
      {
        id: 'ev-v0-5',
        at: hoursAgo(140),
        commitSha: 'f0a317c0',
        message: 'web: atomic components — Base UI + StyleX port',
        author: 'wibus',
        matchedLines: ['+ const res = await fetch(`/v0/symbols?qualified=${name}`)'],
      },
      {
        id: 'ev-v0-4',
        at: hoursAgo(190),
        commitSha: 'c9a3e1b7',
        message: 'web: notebooks — fetch block bodies over /v0',
        author: 'devin',
        matchedLines: ["+   return fetch('/v0/notebooks/' + id).then(r => r.json())"],
      },
      {
        id: 'ev-v0-3',
        at: hoursAgo(240),
        commitSha: '88b1d5f2',
        message: 'web: legacy command palette — history search',
        author: 'wibus',
        matchedLines: ['+ fetch(`/v0/history?q=${encodeURIComponent(q)}`)'],
      },
      {
        id: 'ev-v0-2',
        at: hoursAgo(300),
        commitSha: '4e7b2c96',
        message: 'daemon: metrics shim — poll /v0/status for compat',
        author: 'wibus',
        matchedLines: ['+     let body = get("http://127.0.0.1:7180/v0/status").send()?.text()?;'],
      },
      {
        id: 'ev-v0-1',
        at: hoursAgo(360),
        commitSha: '60f2d8a4',
        message: 'web: insights preview — chart series over /v0/insights',
        author: 'devin',
        matchedLines: ["+ fetch('/v0/insights?dashboard=code-health')"],
      },
    ],
  },
  {
    id: 'mon-releases',
    name: 'Release commits',
    description:
      'Commit-message watch on `release:` subjects — the audit trail for cut versions, paired with the snapshot manifest freeze.',
    owner: 'wibus',
    enabled: true,
    trigger: {
      kind: 'commit',
      query: 'type:commit message:^release:',
    },
    actions: [
      {
        type: 'email',
        target: 'cce-team@localhost',
        deliveries: [
          { at: hoursAgo(190), status: 'sent', detail: 'ev-rel-4 · smtp 250 queued' },
          {
            at: hoursAgo(280),
            status: 'failed',
            detail: 'ev-rel-3 · smtp 550 — alias bounced',
          },
          { at: hoursAgo(360), status: 'sent', detail: 'ev-rel-2 · smtp 250 queued' },
          { at: hoursAgo(430), status: 'sent', detail: 'ev-rel-1 · smtp 250 queued' },
        ],
      },
    ],
    events: [
      {
        id: 'ev-rel-4',
        at: hoursAgo(190),
        commitSha: '0f9d4c8b',
        message: 'release: cut 0.4 — snapshot manifest freeze',
        author: 'wibus',
        matchedLines: [
          'release: cut 0.4 — snapshot manifest freeze',
          '',
          'Freeze ViewManifest at v3; the artifact store stays content-addressed.',
        ],
      },
      {
        id: 'ev-rel-3',
        at: hoursAgo(280),
        commitSha: '3a8f0d2e',
        message: 'release: 0.3.2 — hotfix lease epoch overflow',
        author: 'wibus',
        matchedLines: [
          'release: 0.3.2 — hotfix lease epoch overflow',
          '',
          'Backport the lease-epoch widening to the 0.3 line.',
        ],
      },
      {
        id: 'ev-rel-2',
        at: hoursAgo(360),
        commitSha: 'e5b21f7a',
        message: 'release: 0.3.1 — packing budget backport',
        author: 'cce-ci',
        matchedLines: [
          'release: 0.3.1 — packing budget backport',
          '',
          'Deterministic budget split, cherry-picked from main.',
        ],
      },
      {
        id: 'ev-rel-1',
        at: hoursAgo(430),
        commitSha: '71c4b9d3',
        message: 'release: cut 0.3 — scip trait edges',
        author: 'wibus',
        matchedLines: [
          'release: cut 0.3 — scip trait edges',
          '',
          'Graph route gains trait-impl reference edges.',
        ],
      },
    ],
  },
]
