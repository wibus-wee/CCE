import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { font, vars } from './tokens.stylex'

/**
 * Port of `DisplayKeyValue` — a labeled stat: muted label + mono value,
 * inline or stacked.
 */
export function DisplayKeyValue({
  label,
  value,
  stacked,
  badge,
}: {
  label: ReactNode
  value: ReactNode
  stacked?: boolean
  badge?: ReactNode
}) {
  return (
    <div {...stylex.props(styles.row, stacked && styles.stacked)}>
      <span {...stylex.props(styles.label)}>{label}</span>
      <span {...stylex.props(styles.value)}>
        {value}
        {badge}
      </span>
    </div>
  )
}

const styles = stylex.create({
  row: {
    display: 'inline-flex',
    alignItems: 'baseline',
    gap: 6,
    minWidth: 0,
  },
  stacked: {
    flexDirection: 'column',
    gap: 2,
  },
  label: {
    color: vars.colorMuted,
    fontSize: 12,
    flexShrink: 0,
  },
  value: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 12,
    color: vars.colorBase,
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
  },
})
