import * as stylex from '@stylexjs/stylex'
import { Separator } from '@base-ui-components/react'
import type { HTMLAttributes, ReactNode } from 'react'
import { font, vars } from './tokens.stylex'

/**
 * `LayoutCard` — bordered content surface over `bg-raised` so it nests
 * visibly on any parent (the alpha-surface rule from the token docs).
 */
export function LayoutCard({
  children,
  pad = true,
  ...rest
}: { pad?: boolean } & HTMLAttributes<HTMLDivElement>) {
  return (
    <div {...stylex.props(styles.card, pad && styles.pad)} {...rest}>
      {children}
    </div>
  )
}

/**
 * `LayoutSeparator` — a hairline divider, optionally with a centered label.
 */
export function LayoutSeparator({
  label,
  aside,
  vertical,
}: {
  label?: ReactNode
  aside?: ReactNode
  vertical?: boolean
}) {
  if (vertical)
    return <Separator orientation="vertical" {...stylex.props(styles.vsep)} />
  if (!label) return <Separator {...stylex.props(styles.sep)} />
  return (
    <div {...stylex.props(styles.labeled)} role="separator">
      <span {...stylex.props(styles.sepLine)} />
      <span {...stylex.props(styles.sepLabel)}>{label}</span>
      <span {...stylex.props(styles.sepLine)} />
      {aside && <span {...stylex.props(styles.sepAside)}>{aside}</span>}
    </div>
  )
}

/**
 * `LayoutPanelGrids` — the `bg-dots` radial dot-grid, for empty states and
 * canvases.
 */
export function LayoutPanelGrids({
  cell = 16,
  children,
}: {
  cell?: number
  children?: ReactNode
}) {
  return (
    <div
      {...stylex.props(styles.dots)}
      style={{ backgroundSize: `${cell}px ${cell}px` }}
    >
      {children}
    </div>
  )
}

const styles = stylex.create({
  card: {
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    minWidth: 0,
  },
  pad: {
    padding: 12,
  },
  sep: {
    borderWidth: 0,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderBase,
    margin: 0,
    width: '100%',
  },
  vsep: {
    width: 1,
    alignSelf: 'stretch',
    backgroundColor: vars.borderBase,
    flexShrink: 0,
  },
  labeled: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
  },
  sepLine: {
    flex: 1,
    height: 1,
    backgroundColor: vars.borderBase,
  },
  sepLabel: {
    fontSize: 11,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
  },
  sepAside: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    whiteSpace: 'nowrap',
    flexShrink: 0,
  },
  dots: {
    backgroundImage: `radial-gradient(circle, ${vars.borderBase} 1px, transparent 1px)`,
    backgroundPosition: 'center',
  },
})
