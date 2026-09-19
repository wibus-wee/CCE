import { useSyncExternalStore } from 'react'
import { api, describeError, type SearchHit } from '../api'

/**
 * Code monitors — the honest local version of Sourcegraph's diff watcher.
 * A monitor is a saved `/v1/search` query plus a BASELINE: the snapshot +
 * hit set captured when it was last checked. Checking re-runs the query;
 * when the daemon's snapshotId moved (reindex landed), hits absent from
 * the baseline become an EVENT ("N new matches") and the baseline rolls
 * forward.
 *
 * Honesty boundary: checks happen when the monitoring screen is open (and
 * once on app shell mount if monitors exist) — there is no daemon push or
 * background daemon-side scheduling. Events and baselines persist in
 * localStorage; delivery channels beyond the on-screen event log are
 * deliberately unclaimed.
 */

const KEY = 'cce:monitors:v1'
const MAX_EVENTS = 25
const MAX_EVENT_HITS = 12
const MAX_HIT_IDS = 500

export interface MonitorHit {
  /** entityId|path:line identity — stable across snapshots for the same entity. */
  id: string
  label: string
  path?: string
  line?: number
}

export interface MonitorEvent {
  id: string
  at: string // ISO
  snapshotId: string
  newHits: MonitorHit[]
  /** newHits may be truncated — this is the true count. */
  newCount: number
}

export interface Monitor {
  id: string
  query: string
  enabled: boolean
  createdAt: string
  baseline?: { snapshotId: string; hitIds: string[] }
  events: MonitorEvent[]
  lastCheckedAt?: string
  lastError?: string
}

export interface CheckResult {
  monitor: Monitor
  /** null → query failed (error recorded on monitor.lastError). */
  outcome: 'event' | 'checked' | 'primed' | 'error'
}

function hitId(h: SearchHit): string {
  const a = h.address
  return `${h.entityId}|${a?.path ?? ''}:${a?.startLine ?? 0}`
}

function toMonitorHit(h: SearchHit): MonitorHit {
  return {
    id: hitId(h),
    label: h.symbolName ?? h.address?.path ?? h.documentId,
    path: h.address?.path,
    line: h.address?.startLine,
  }
}

// --- persisted store --------------------------------------------------------

let monitors: Monitor[] | undefined

function read(): Monitor[] {
  if (monitors) return monitors
  try {
    const raw = localStorage.getItem(KEY)
    const parsed = raw ? (JSON.parse(raw) as unknown) : []
    monitors = Array.isArray(parsed)
      ? parsed.filter(
          (m): m is Monitor =>
            typeof m === 'object' && m !== null && typeof (m as Monitor).query === 'string',
        )
      : []
  } catch {
    monitors = []
  }
  return monitors
}

const listeners = new Set<() => void>()

function write(next: Monitor[]) {
  monitors = next
  try {
    localStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    // best-effort persistence
  }
  listeners.forEach((fn) => fn())
}

function subscribe(fn: () => void) {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

function getSnapshot(): Monitor[] {
  return read()
}

export function useMonitors(): Monitor[] {
  return useSyncExternalStore(subscribe, getSnapshot)
}

export function addMonitor(query: string): Monitor {
  const m: Monitor = {
    id: `mon_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 7)}`,
    query: query.trim(),
    enabled: true,
    createdAt: new Date().toISOString(),
    events: [],
  }
  write([m, ...read()])
  return m
}

export function removeMonitor(id: string) {
  write(read().filter((m) => m.id !== id))
}

export function setMonitorEnabled(id: string, enabled: boolean) {
  write(read().map((m) => (m.id === id ? { ...m, enabled } : m)))
}

function patch(id: string, f: (m: Monitor) => Monitor) {
  write(read().map((m) => (m.id === id ? f(m) : m)))
}

/** Total unacknowledged new-match count — drives the sidebar badge. */
export function unreadCount(list: Monitor[]): number {
  return list.reduce((n, m) => n + m.events.length, 0)
}

// --- checking ----------------------------------------------------------------

/**
 * Run the monitor's query against the daemon and reconcile with baseline.
 * - No baseline yet → PRIME: store the current hit set, no event.
 * - Snapshot unchanged → CHECKED: nothing fired, lastCheckedAt updated.
 * - Snapshot moved + hits differ → EVENT (possibly zero new hits — still
 *   rolls the baseline forward; zero-hit diffs record no event).
 */
export async function checkMonitor(m: Monitor): Promise<CheckResult> {
  const now = new Date().toISOString()
  try {
    const [status, result] = await Promise.all([
      api.status(),
      api.search({ query: m.query, limit: 50 }),
    ])
    const snap = status.snapshotId
    const ids = result.hits.map(hitId)
    if (!m.baseline) {
      const monitor = { ...m, baseline: { snapshotId: snap, hitIds: ids.slice(0, MAX_HIT_IDS) }, lastCheckedAt: now, lastError: undefined }
      patch(m.id, () => monitor)
      return { monitor, outcome: 'primed' }
    }
    if (m.baseline.snapshotId === snap) {
      const monitor = { ...m, lastCheckedAt: now, lastError: undefined }
      patch(m.id, () => monitor)
      return { monitor, outcome: 'checked' }
    }
    const base = new Set(m.baseline.hitIds)
    const fresh = result.hits.filter((h) => !base.has(hitId(h))).map(toMonitorHit)
    const events =
      fresh.length === 0
        ? m.events
        : [
            {
              id: `ev_${Date.now().toString(36)}`,
              at: now,
              snapshotId: snap,
              newHits: fresh.slice(0, MAX_EVENT_HITS),
              newCount: fresh.length,
            },
            ...m.events,
          ].slice(0, MAX_EVENTS)
    const monitor = {
      ...m,
      baseline: { snapshotId: snap, hitIds: ids.slice(0, MAX_HIT_IDS) },
      events,
      lastCheckedAt: now,
      lastError: undefined,
    }
    patch(m.id, () => monitor)
    return { monitor, outcome: fresh.length === 0 ? 'checked' : 'event' }
  } catch (e) {
    const monitor = { ...m, lastCheckedAt: now, lastError: describeError(e) }
    patch(m.id, () => monitor)
    return { monitor, outcome: 'error' }
  }
}

/** Dry run — current hits without touching the baseline (test preview). */
export async function probeMonitor(query: string): Promise<MonitorHit[]> {
  const r = await api.search({ query, limit: 50 })
  return r.hits.map(toMonitorHit)
}
