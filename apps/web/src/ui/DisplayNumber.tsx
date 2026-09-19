import * as stylex from '@stylexjs/stylex'
import { useEffect, useState, type ReactNode } from 'react'
import {
  AGE_SCALE,
  BYTES_SCALE,
  DURATION_SCALE,
  formatBytes,
  formatDateTime,
  formatDuration,
  formatNumber,
  formatTimeAgo,
  mapSeverity,
} from './format'
import { OverlayTooltip } from './OverlayTooltip'
import { badges, severityColor } from './recipes.stylex'
import { font, vars, type Severity } from './tokens.stylex'

/**
 * `DisplayNumber` / `DisplayDuration` / `DisplayBytes` / `DisplayDate` —
 * technical values in mono + tabular numerals with `colorize` severity ramps.
 */
export function DisplayNumber({
  value,
  prefix,
  suffix,
  colorize,
  options,
}: {
  value: number
  prefix?: ReactNode
  suffix?: ReactNode
  colorize?: boolean
  options?: Intl.NumberFormatOptions
}) {
  return (
    <span {...stylex.props(styles.value)}>
      {prefix}
      <span>{formatNumber(value, options)}</span>
      {suffix}
    </span>
  )
}

export function DisplayDuration({
  value,
  colorize = false,
}: {
  /** Duration in milliseconds. */
  value: number | null | undefined
  colorize?: boolean
}) {
  const sev = value == null ? 'neutral' : mapSeverity(value, DURATION_SCALE)
  return (
    <span {...stylex.props(styles.value, colorize && severityColor[sev])}>
      {formatDuration(value)}
    </span>
  )
}

export function DisplayBytes({
  value,
  colorize = false,
  total,
  options,
}: {
  /** Byte count. */
  value: number
  colorize?: boolean
  /** Render as a percent of `total` alongside the size. */
  total?: number
  options?: { base?: 1000 | 1024; digits?: number }
}) {
  const [amount, unit] = formatBytes(value, options)
  const sev = mapSeverity(value, BYTES_SCALE)
  const percent = total ? ` (${((value / total) * 100).toFixed(1)}%)` : ''
  return (
    <span {...stylex.props(styles.value, colorize && severityColor[sev])}>
      {amount}
      <span {...stylex.props(styles.unit)}>{unit}</span>
      {percent}
    </span>
  )
}

/**
 * `DisplayDate` — relative time + exact-date tooltip, `colorize` by age,
 * `live` re-renders on a 30s tick.
 */
export function DisplayDate({
  value,
  colorize = false,
  live = false,
}: {
  value: Date | number | string
  colorize?: boolean
  live?: boolean
}) {
  useTick(live)
  const time = value instanceof Date ? value.getTime() : new Date(value).getTime()
  const age = Number.isFinite(time) ? Date.now() - time : 0
  const sev = mapSeverity(age, AGE_SCALE)
  const label = (
    <span {...stylex.props(styles.value, colorize && severityColor[sev])}>
      {formatTimeAgo(value)}
    </span>
  )
  const exact = formatDateTime(value, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
  return <OverlayTooltip content={exact}>{label}</OverlayTooltip>
}

/** 30s re-render tick while `live` is on. */
function useTick(enabled: boolean): void {
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!enabled) return
    const id = setInterval(() => setTick(t => t + 1), 30_000)
    return () => clearInterval(id)
  }, [enabled])
}

/** `DisplayTimeAgo` alias kept for callers already using the name. */
export const DisplayTimeAgo = DisplayDate

/**
 * `DisplayNumberBadge` — a `DisplayNumber` in a badge shell: the count pill
 * used for tab counts and list sizes.
 */
export function DisplayNumberBadge({
  value,
  severity,
  options,
}: {
  value: number
  severity?: Severity
  options?: Intl.NumberFormatOptions
}) {
  return (
    <span
      {...stylex.props(
        badges.base,
        severity ? severityColor[severity] : badges.muted,
        styles.badgeMono,
      )}
    >
      {formatNumber(value, options)}
    </span>
  )
}

/** `DisplayVersion` — `vX.Y.Z` prefix; specs/ranges pass through untouched. */
export function DisplayVersion({ version }: { version: string }) {
  const isBareSemver = /^\d+\.\d+\.\d+/.test(version.trim())
  return (
    <span {...stylex.props(styles.value)}>
      {isBareSemver ? `v${version.trim()}` : version}
    </span>
  )
}

const styles = stylex.create({
  value: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    display: 'inline-flex',
    alignItems: 'baseline',
    gap: 2,
    whiteSpace: 'nowrap',
  },
  unit: {
    color: vars.colorFaint,
    fontSize: '0.85em',
  },
  badgeMono: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
  },
})
