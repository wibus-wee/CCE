import * as stylex from '@stylexjs/stylex'
import { useHotkey } from '@tanstack/react-hotkeys'
import { useEffect, useRef } from 'react'
import { IconClock, IconStar } from '../ui/icons'
import { popup, text } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Recent + saved searches — a dropdown anchored under the query field
 * (Sourcegraph's "recent searches"). Local state only; see
 * lib/searchHistory.
 */
export function HistoryPopover({
  recents,
  saved,
  open,
  onPick,
  onClose,
}: {
  recents: string[]
  saved: string[]
  open: boolean
  onPick: (query: string) => void
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)

  useHotkey('Escape', onClose, { enabled: open, conflictBehavior: 'allow' })

  useEffect(() => {
    if (!open) return
    function onPointerDown(event: PointerEvent) {
      if (ref.current && !ref.current.contains(event.target as Node)) onClose()
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown)
    }
  }, [open, onClose])

  if (!open || (recents.length === 0 && saved.length === 0)) return null

  return (
    <div ref={ref} role="menu" {...stylex.props(popup.surface, styles.menu)}>
      {saved.length > 0 && (
        <>
          <div {...stylex.props(popup.label)}>Saved</div>
          {saved.map((query) => (
            <Row key={`s:${query}`} query={query} icon="star" onPick={onPick} />
          ))}
        </>
      )}
      {recents.length > 0 && (
        <>
          <div {...stylex.props(popup.label)}>Recent</div>
          {recents.map((query) => (
            <Row key={`r:${query}`} query={query} icon="clock" onPick={onPick} />
          ))}
        </>
      )}
    </div>
  )
}

function Row({
  query,
  icon,
  onPick,
}: {
  query: string
  icon: 'star' | 'clock'
  onPick: (query: string) => void
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={() => onPick(query)}
      {...stylex.props(popup.item, styles.row)}
    >
      <span {...stylex.props(styles.icon, text.faint)}>
        {icon === 'star' ? <IconStar size={12} filled /> : <IconClock size={12} />}
      </span>
      <span {...stylex.props(styles.query)}>{query}</span>
    </button>
  )
}

const styles = stylex.create({
  menu: {
    position: 'absolute',
    top: 'calc(100% + 4px)',
    left: 0,
    right: 0,
    maxHeight: 280,
    overflowY: 'auto',
  },
  row: {
    width: '100%',
    borderWidth: 0,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    textAlign: 'left',
    fontFamily: 'inherit',
  },
  icon: {
    display: 'inline-flex',
    flexShrink: 0,
  },
  query: {
    fontFamily: font.mono,
    fontSize: 12,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
})
