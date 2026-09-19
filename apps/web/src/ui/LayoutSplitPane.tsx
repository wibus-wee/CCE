import * as stylex from '@stylexjs/stylex'
import { useCallback, useRef, useState, type ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `LayoutSplitPane` — a two-pane container with a draggable
 * divider. Base UI has no split-pane primitive; this is a small
 * pointer-event splitter (keyboard: ArrowLeft/Right on the handle).
 */
export function LayoutSplitPane({
  direction = 'horizontal',
  defaultSplit = 0.5,
  min = 0.15,
  max = 0.85,
  first,
  second,
}: {
  direction?: 'horizontal' | 'vertical'
  /** Initial fraction for the first pane (0-1). */
  defaultSplit?: number
  min?: number
  max?: number
  first: ReactNode
  second: ReactNode
}) {
  const [split, setSplit] = useState(defaultSplit)
  const rootRef = useRef<HTMLDivElement>(null)
  const horizontal = direction === 'horizontal'

  const updateFromPointer = useCallback(
    (clientX: number, clientY: number) => {
      const rect = rootRef.current?.getBoundingClientRect()
      if (!rect) return
      const pos = horizontal
        ? (clientX - rect.left) / rect.width
        : (clientY - rect.top) / rect.height
      setSplit(Math.min(max, Math.max(min, pos)))
    },
    [horizontal, min, max],
  )

  const onPointerDown = (e: React.PointerEvent) => {
    e.preventDefault()
    const move = (ev: PointerEvent) => updateFromPointer(ev.clientX, ev.clientY)
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <div
      ref={rootRef}
      {...stylex.props(styles.root)}
      style={{ flexDirection: horizontal ? 'row' : 'column' }}
    >
      <div
        {...stylex.props(styles.pane)}
        style={horizontal ? { width: `${split * 100}%` } : { height: `${split * 100}%` }}
      >
        {first}
      </div>
      <div
        role="separator"
        aria-orientation={horizontal ? 'vertical' : 'horizontal'}
        aria-valuenow={Math.round(split * 100)}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onKeyDown={(e) => {
          const step = 0.05
          if (e.key === 'ArrowLeft' || e.key === 'ArrowUp')
            setSplit((s) => Math.max(min, s - step))
          if (e.key === 'ArrowRight' || e.key === 'ArrowDown')
            setSplit((s) => Math.min(max, s + step))
        }}
        {...stylex.props(
          styles.divider,
          horizontal ? styles.dividerX : styles.dividerY,
        )}
      />
      <div {...stylex.props(styles.pane, styles.paneSecond)}>{second}</div>
    </div>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    width: '100%',
    height: '100%',
    minWidth: 0,
    minHeight: 0,
  },
  pane: {
    minWidth: 0,
    minHeight: 0,
    overflow: 'hidden',
    flexShrink: 0,
  },
  paneSecond: {
    flex: 1,
    flexShrink: 1,
  },
  divider: {
    flexShrink: 0,
    backgroundColor: {
      default: vars.borderBase,
      ':hover': vars.borderActive,
      ':focus-visible': vars.borderActive,
    },
    outline: 'none',
    transitionProperty: 'background-color',
    transitionDuration: '120ms',
  },
  dividerX: {
    width: 5,
    marginLeft: -2,
    marginRight: -2,
    cursor: 'col-resize',
    zIndex: 5,
  },
  dividerY: {
    height: 5,
    marginTop: -2,
    marginBottom: -2,
    cursor: 'row-resize',
    zIndex: 5,
  },
})
