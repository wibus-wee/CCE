import { useSyncExternalStore } from 'react'

/**
 * Local pack history — recent context packs, persisted in localStorage.
 * Client-side only (no endpoint needed): the pack builder's equivalent of
 * Sourcegraph's "recent searches" for a single-operator local tool.
 */

const PACKS_KEY = 'cce:pack:recents'
const MAX_PACKS = 8

export interface PackHistoryEntry {
  query: string
  budgetTokens: number
  snapshotId: string
  intent: string
  itemCount: number
  usedTokens: number
  at: string
}

// Parsed lists are cached per key so useSyncExternalStore's getSnapshot
// stays referentially stable between writes.
const cache = new Map<string, PackHistoryEntry[]>()

function read(key: string): PackHistoryEntry[] {
  let list = cache.get(key)
  if (!list) {
    try {
      const raw = localStorage.getItem(key)
      const parsed = raw ? (JSON.parse(raw) as unknown) : []
      list = Array.isArray(parsed)
        ? parsed.filter(
            (x): x is PackHistoryEntry =>
              typeof x === 'object' &&
              x !== null &&
              typeof (x as PackHistoryEntry).query === 'string' &&
              typeof (x as PackHistoryEntry).budgetTokens === 'number' &&
              typeof (x as PackHistoryEntry).snapshotId === 'string' &&
              typeof (x as PackHistoryEntry).intent === 'string' &&
              typeof (x as PackHistoryEntry).itemCount === 'number' &&
              typeof (x as PackHistoryEntry).usedTokens === 'number' &&
              typeof (x as PackHistoryEntry).at === 'string',
          )
        : []
    } catch {
      list = []
    }
    cache.set(key, list)
  }
  return list
}

function write(key: string, list: PackHistoryEntry[]) {
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

export function getPacks(): PackHistoryEntry[] {
  return read(PACKS_KEY)
}

// Recency + identity both live in (query, budget): a repeat build of the
// same ask moves to the top instead of duplicating.
export function pushPack(entry: PackHistoryEntry) {
  const query = entry.query.trim()
  if (!query) return
  const normalized = { ...entry, query }
  write(
    PACKS_KEY,
    [
      normalized,
      ...read(PACKS_KEY).filter(
        (x) => !(x.query === query && x.budgetTokens === entry.budgetTokens),
      ),
    ].slice(0, MAX_PACKS),
  )
}

export function removePack(query: string, budgetTokens: number) {
  write(
    PACKS_KEY,
    read(PACKS_KEY).filter(
      (x) => !(x.query === query && x.budgetTokens === budgetTokens),
    ),
  )
}

export function clearPacks() {
  write(PACKS_KEY, [])
}

export function usePackHistory() {
  const packs = useSyncExternalStore(subscribe, getPacks)
  return { packs, push: pushPack, remove: removePack, clear: clearPacks }
}
