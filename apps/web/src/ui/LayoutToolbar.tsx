import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `LayoutToolbar` — a sticky, glass-surfaced action bar
 * (`bg-tooltip` + backdrop-blur) with start/end slots.
 */
export function LayoutToolbar({
  start,
  center,
  end,
}: {
  start?: ReactNode
  center?: ReactNode
  end?: ReactNode
}) {
  return (
    <header {...stylex.props(styles.bar)}>
      <div {...stylex.props(styles.side)}>{start}</div>
      {center && <div {...stylex.props(styles.center)}>{center}</div>}
      <div {...stylex.props(styles.side, styles.end)}>{end}</div>
    </header>
  )
}

const styles = stylex.create({
  bar: {
    position: 'sticky',
    top: 0,
    zIndex: 10, // z-nav — named layer owned by the app
    display: 'flex',
    alignItems: 'center',
    gap: 12,
    minHeight: 46,
    paddingLeft: 12,
    paddingRight: 12,
    backgroundColor: vars.bgTooltip,
    backdropFilter: 'blur(8px)',
    WebkitBackdropFilter: 'blur(8px)',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
  },
  side: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    minWidth: 0,
  },
  end: {
    marginLeft: 'auto',
  },
  center: {
    flex: 1,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    minWidth: 0,
  },
})
