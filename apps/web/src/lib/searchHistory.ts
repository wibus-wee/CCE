import { useSyncExternalStore } from 'react'

/**
 * Local search history — recents + starred saves, persisted in
 * localStorage. Client-side only (no endpoint needed): Sourcegraph's
 * "recent searches" + "saved searches" for a single-operator local tool.
 */

const RECENTS_KEY = 'cce:search:recents'
const SAVED_KEY = 'cce:search:saved'
const MAX_RECENTS = 8

// Parsed lists are cached per key so useSyncExternalStore's getSnapshot
// stays referentially stable between writes.
const cache = new Map<string, string[]>()

function read(key: string): string[] {
  let list = cache.get(key)
  if (!list) {
    try {
      const raw = localStorage.getItem(key)
      const parsed = raw ? (JSON.parse(raw) as unknown) : []
      list = Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === 'string') : []
    } catch {
      list = []
    }
    cache.set(key, list)
  }
  return list
}

function write(key: string, list: string[]) {
  cache.set(key, list)
  try {
    localStorage.setItem(key, JSON.stringify(list))
  } catch {
    // storage full / private mode — history is best-effort
  }
  listeners.forEach((fn) => fn())
}

const listeners = new Set<() => void>()
function subscribe(fn: () => void) {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

export function getRecents(): string[] {
  return read(RECENTS_KEY)
}

export function pushRecent(query: string) {
  const q = query.trim()
  if (!q) return
  write(RECENTS_KEY, [q, ...read(RECENTS_KEY).filter((x) => x !== q)].slice(0, MAX_RECENTS))
}

export function getSaved(): string[] {
  return read(SAVED_KEY)
}

export function toggleSaved(query: string) {
  const q = query.trim()
  if (!q) return
  const saved = read(SAVED_KEY)
  write(SAVED_KEY, saved.includes(q) ? saved.filter((x) => x !== q) : [q, ...saved])
}

export function isSaved(query: string): boolean {
  return read(SAVED_KEY).includes(query.trim())
}

export function useSearchHistory() {
  const recents = useSyncExternalStore(subscribe, getRecents)
  const saved = useSyncExternalStore(subscribe, getSaved)
  return { recents, saved }
}
