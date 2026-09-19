import { useSyncExternalStore } from 'react'

/**
 * Client-side query telemetry — real measurements, not fixtures. Every
 * /v1/search call records its reported latency, hit count, and snapshot so
 * the dashboard can chart actual retrieval performance for this operator.
 * Persisted in localStorage, capped at the most recent entries.
 */

const KEY = 'cce:telemetry:queries'
const MAX_ENTRIES = 40

export interface QueryMeasurement {
  query: string
  latencyMs: number
  hits: number
  snapshotId: string
  at: string
}

// Cached snapshot so useSyncExternalStore's getSnapshot stays referentially
// stable between writes.
let cache: QueryMeasurement[] | undefined

function read(): QueryMeasurement[] {
  if (!cache) {
    try {
      const raw = localStorage.getItem(KEY)
      const parsed = raw ? (JSON.parse(raw) as unknown) : []
      cache = Array.isArray(parsed) ? (parsed as QueryMeasurement[]) : []
    } catch {
      cache = []
    }
  }
  return cache
}

const listeners = new Set<() => void>()
function subscribe(fn: () => void) {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

export function recordQuery(entry: QueryMeasurement) {
  const list = [entry, ...read()].slice(0, MAX_ENTRIES)
  cache = list
  try {
    localStorage.setItem(KEY, JSON.stringify(list))
  } catch {
    // storage full / private mode — telemetry is best-effort
  }
  listeners.forEach((fn) => fn())
}

export function getQueryTelemetry(): QueryMeasurement[] {
  return read()
}

export function useQueryTelemetry(): QueryMeasurement[] {
  return useSyncExternalStore(subscribe, getQueryTelemetry)
}
