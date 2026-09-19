import type { Severity } from './tokens.stylex'

/**
 * Ports of the pure helpers in `@antfu/design/utils` — same behavior, no
 * colorjs/dompurify peer surface so they stay importable from React.
 */

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat('en-US', options).format(value)
}

const DURATION_FACTOR_MS = { ns: 1e-6, us: 1e-3, ms: 1, s: 1000 } as const

export function formatDuration(value: number | null | undefined): string {
  if (value == null) return '-'
  const ms = value
  if (ms < 1) return '<1 ms'
  if (ms < 1000) return `${ms.toFixed(0)} ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`
  if (ms < 3_600_000) return `${(ms / 60_000).toFixed(1)} min`
  if (ms < 86_400_000) return `${(ms / 3_600_000).toFixed(1)} h`
  return `${(ms / 86_400_000).toFixed(1)} d`
}

const TIME_UNITS: [limit: number, divisor: number, unit: string][] = [
  [60_000, 1000, 's'],
  [3_600_000, 60_000, 'min'],
  [86_400_000, 3_600_000, 'h'],
  [7 * 86_400_000, 86_400_000, 'd'],
  [30 * 86_400_000, 7 * 86_400_000, 'w'],
  [365 * 86_400_000, 30 * 86_400_000, 'mo'],
  [Number.POSITIVE_INFINITY, 365 * 86_400_000, 'y'],
]

export function formatTimeAgo(input: Date | number | string, now: number = Date.now()): string {
  const time = input instanceof Date ? input.getTime() : new Date(input).getTime()
  if (!Number.isFinite(time)) return ''
  const delta = now - time
  const abs = Math.abs(delta)
  if (abs < 1000) return 'just now'
  for (const [limit, divisor, unit] of TIME_UNITS) {
    if (abs < limit) {
      const value = Math.round(abs / divisor)
      return delta >= 0 ? `${value} ${unit} ago` : `in ${value} ${unit}`
    }
  }
  return ''
}

/**
 * Deterministic string -> hsla() tint, same algorithm as
 * `getHashColorFromString` (charCode fold -> hue 0..360).
 */
export function getHashColorFromString(name: string, opacity: number | string = 1, dark = false): string {
  let hash = 0
  for (let i = 0; i < name.length; i++) hash = name.charCodeAt(i) + ((hash << 5) - hash)
  const h = ((hash % 360) + 360) % 360
  const s = dark ? 50 : 65
  const l = dark ? 60 : 40
  return `hsla(${h}, ${s}%, ${l}%, ${opacity})`
}

/** `path:startLine–endLine`, the canonical source-address rendering. */
export function formatAddress(address: {
  path: string
  startLine: number
  endLine: number
}): string {
  const lines =
    address.endLine > address.startLine
      ? `${address.startLine}–${address.endLine}`
      : `${address.startLine}`
  return `${address.path}:${lines}`
}

/** Ascending `[max, severity]` thresholds; first bucket that fits wins. */
export function mapSeverity(value: number, scale: readonly (readonly [number, Severity])[]): Severity {
  for (const [max, level] of scale) {
    if (value <= max) return level
  }
  return scale.at(-1)?.[1] ?? 'neutral'
}

/** fast -> neutral, slow -> critical (ms). Same thresholds as DURATION_SCALE. */
export const DURATION_SCALE: readonly (readonly [number, Severity])[] = [
  [50, 'neutral'],
  [200, 'low'],
  [1000, 'medium'],
  [5000, 'high'],
  [Number.POSITIVE_INFINITY, 'critical'],
]

const BYTE_UNITS_1024 = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const
const BYTE_UNITS_1000 = ['B', 'kB', 'MB', 'GB', 'TB', 'PB'] as const

export interface FormatBytesOptions {
  /** `1024` (binary, default) or `1000` (decimal). */
  base?: 1000 | 1024
  /** Maximum decimal places before trimming. Defaults to `2`. */
  digits?: number
}

/** Humanize a byte count into a `[value, unit]` tuple, e.g. `['1.5', 'KB']`. */
export function formatBytes(bytes: number, options: FormatBytesOptions = {}): [string, string] {
  const { base = 1024, digits = 2 } = options
  if (!bytes || bytes < 0) return ['0', 'B']
  const units = base === 1000 ? BYTE_UNITS_1000 : BYTE_UNITS_1024
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(base)), units.length - 1)
  if (i === 0) return [String(bytes), 'B']
  const value = (bytes / base ** i).toFixed(digits).replace(/\.?0+$/, '')
  return [value, units[i] ?? 'B']
}

/** small -> neutral, huge -> critical (bytes). Same thresholds as BYTES_SCALE. */
export const BYTES_SCALE: readonly (readonly [number, Severity])[] = [
  [1024, 'neutral'],
  [10 * 1024, 'low'],
  [100 * 1024, 'medium'],
  [1024 * 1024, 'high'],
  [Number.POSITIVE_INFINITY, 'critical'],
]

/** fresh -> neutral, old -> critical (age in ms). Same thresholds as AGE_SCALE. */
export const AGE_SCALE: readonly (readonly [number, Severity])[] = [
  [60_000, 'neutral'],
  [3_600_000, 'low'],
  [86_400_000, 'medium'],
  [7 * 86_400_000, 'high'],
  [Number.POSITIVE_INFINITY, 'critical'],
]

export function formatPercent(value: number, digits = 1): string {
  return `${(value * 100).toFixed(digits)}%`
}

export function formatDateTime(input: Date | number | string, options?: Intl.DateTimeFormatOptions): string {
  const date = input instanceof Date ? input : new Date(input)
  if (!Number.isFinite(date.getTime())) return ''
  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    ...options,
  }).format(date)
}

/** Byte length of a string's UTF-8 encoding. */
export function getContentByteSize(str: string): number {
  return new TextEncoder().encode(str).length
}
