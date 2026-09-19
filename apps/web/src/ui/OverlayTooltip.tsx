import * as stylex from '@stylexjs/stylex'
import { Tooltip } from '@base-ui-components/react'
import type { ReactElement, ReactNode } from 'react'
import { vars } from './tokens.stylex'

type Placement = 'top' | 'bottom' | 'left' | 'right'

/**
 * Port of `OverlayTooltip` on Base UI `Tooltip` — text `content` or a rich
 * `tip` node, `placement`, `delay` (ms or `{show, hide}`), and a
 * virtual-anchor mode (`anchor: {x, y}` + `open`) for canvas overlays.
 */
export function OverlayTooltip({
  content,
  tip,
  placement = 'top',
  distance = 6,
  delay,
  triggers,
  open,
  anchor,
  disabled,
  children,
}: {
  /** Tooltip text (use `tip` for rich content). */
  content?: string
  /** Rich tooltip content — the `#content` slot. */
  tip?: ReactNode
  placement?: Placement
  distance?: number
  /** Show/hide delay in ms, or `{ show, hide }`. */
  delay?: number | { show?: number; hide?: number }
  /** Which triggers open it; `['click']` for click-only. */
  triggers?: Array<'hover' | 'focus' | 'click'>
  /** Programmatic open state (bypasses triggers when set). */
  open?: boolean
  /** Anchor to a viewport coordinate instead of the trigger element. */
  anchor?: { x: number; y: number }
  disabled?: boolean
  children: ReactElement
}) {
  const showDelay = typeof delay === 'number' ? delay : (delay?.show ?? 300)
  const hideDelay = typeof delay === 'number' ? 0 : (delay?.hide ?? 0)
  const clickOnly = triggers?.length === 1 && triggers[0] === 'click'

  const virtualAnchor =
    anchor != null
      ? {
          getBoundingClientRect: () =>
            ({
              x: anchor.x,
              y: anchor.y,
              top: anchor.y,
              left: anchor.x,
              right: anchor.x,
              bottom: anchor.y,
              width: 0,
              height: 0,
            }) as DOMRect,
        }
      : undefined

  if (disabled) return children

  return (
    <Tooltip.Provider delay={showDelay} closeDelay={hideDelay}>
      <Tooltip.Root open={open} disabled={disabled}>
        <Tooltip.Trigger
          render={children as ReactElement<Record<string, unknown>>}
          {...(clickOnly ? { nativeButton: true } : {})}
        />
        <Tooltip.Portal>
          <Tooltip.Positioner side={placement} sideOffset={distance} anchor={virtualAnchor}>
            <Tooltip.Popup
              className={(state) =>
                stylex.props(styles.popup, state.transitionStatus === 'starting' && styles.entering).className
              }
            >
              {tip ?? content}
            </Tooltip.Popup>
          </Tooltip.Positioner>
        </Tooltip.Portal>
      </Tooltip.Root>
    </Tooltip.Provider>
  )
}

const styles = stylex.create({
  popup: {
    zIndex: 60,
    maxWidth: 320,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgTooltip,
    backdropFilter: 'blur(8px)',
    color: vars.colorBase,
    fontSize: 11,
    lineHeight: '1.45',
    boxShadow: vars.shadowOverlay,
    opacity: 1,
    transitionProperty: 'opacity, transform',
    transitionDuration: '120ms',
  },
  entering: {
    opacity: 0,
  },
})
