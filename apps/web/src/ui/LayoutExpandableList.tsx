import * as stylex from '@stylexjs/stylex'
import { useState, type ReactNode } from 'react'
import { buttons } from './recipes.stylex'
import { vars } from './tokens.stylex'

/**
 * Port of `LayoutExpandableList` — renders up to `limit` rows with a
 * "Show N more / Show less" toggle for the remainder.
 */
export function LayoutExpandableList<T>({
  items,
  limit = 5,
  renderItem,
  itemKey,
  moreLabel,
  lessLabel = 'Show less',
}: {
  items: T[]
  limit?: number
  renderItem: (item: T, index: number) => ReactNode
  itemKey: (item: T, index: number) => string | number
  moreLabel?: (hiddenCount: number) => ReactNode
  lessLabel?: ReactNode
}) {
  const [expanded, setExpanded] = useState(false)
  const hidden = items.length - limit
  const visible = expanded ? items : items.slice(0, limit)
  return (
    <div {...stylex.props(styles.list)}>
      {visible.map((item, i) => (
        <div key={itemKey(item, i)} {...stylex.props(styles.row)}>
          {renderItem(item, i)}
        </div>
      ))}
      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          {...stylex.props(buttons.text, styles.toggle)}
        >
          {expanded ? lessLabel : (moreLabel?.(hidden) ?? `Show ${hidden} more`)}
        </button>
      )}
    </div>
  )
}

const styles = stylex.create({
  list: {
    display: 'flex',
    flexDirection: 'column',
    minWidth: 0,
  },
  row: {
    minWidth: 0,
  },
  toggle: {
    alignSelf: 'flex-start',
    fontSize: 12,
    color: vars.colorMuted,
    marginTop: 2,
  },
})
