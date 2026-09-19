import * as stylex from '@stylexjs/stylex'
import { AlertDialog, Dialog, PreviewCard } from '@base-ui-components/react'
import type { ReactElement, ReactNode } from 'react'
import { buttons, popup } from './recipes.stylex'
import { vars } from './tokens.stylex'

/**
 * Port of `OverlayModal` on Base UI `Dialog` — a centered modal with a
 * dimmed backdrop, `title`/`description`, and `#actions`.
 */
export function OverlayModal({
  trigger,
  open,
  onOpenChange,
  title,
  description,
  actions,
  width = 420,
  children,
}: {
  trigger?: ReactElement
  open?: boolean
  onOpenChange?: (open: boolean) => void
  title?: ReactNode
  description?: ReactNode
  /** Footer action row. */
  actions?: ReactNode
  width?: number
  children: ReactNode
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger && <Dialog.Trigger render={trigger as ReactElement<Record<string, unknown>>} />}
      <Dialog.Portal>
        <Dialog.Backdrop {...stylex.props(styles.backdrop)} />
        <Dialog.Viewport {...stylex.props(styles.viewport)}>
          <Dialog.Popup {...stylex.props(styles.modal)} style={{ maxWidth: width }}>
            {title != null && <Dialog.Title {...stylex.props(styles.title)}>{title}</Dialog.Title>}
            {description != null && (
              <Dialog.Description {...stylex.props(styles.description)}>
                {description}
              </Dialog.Description>
            )}
            <div {...stylex.props(styles.body)}>{children}</div>
            {actions != null && <div {...stylex.props(styles.actions)}>{actions}</div>}
          </Dialog.Popup>
        </Dialog.Viewport>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

/**
 * Port of `OverlayConfirm` on Base UI `AlertDialog` — a small destructive-
 * style confirm dialog with confirm/cancel actions.
 */
export function OverlayConfirm({
  trigger,
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  danger = false,
  onConfirm,
}: {
  trigger?: ReactElement
  open?: boolean
  onOpenChange?: (open: boolean) => void
  title: ReactNode
  description?: ReactNode
  confirmLabel?: ReactNode
  cancelLabel?: ReactNode
  danger?: boolean
  onConfirm?: () => void
}) {
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger && <AlertDialog.Trigger render={trigger as ReactElement<Record<string, unknown>>} />}
      <AlertDialog.Portal>
        <AlertDialog.Backdrop {...stylex.props(styles.backdrop)} />
        <AlertDialog.Viewport {...stylex.props(styles.viewport)}>
          <AlertDialog.Popup {...stylex.props(styles.confirm)}>
            <AlertDialog.Title {...stylex.props(styles.title)}>{title}</AlertDialog.Title>
            {description != null && (
              <AlertDialog.Description {...stylex.props(styles.description)}>
                {description}
              </AlertDialog.Description>
            )}
            <div {...stylex.props(styles.actions)}>
              <AlertDialog.Close {...stylex.props(buttons.action)}>{cancelLabel}</AlertDialog.Close>
              <AlertDialog.Close
                onClick={onConfirm}
                {...stylex.props(buttons.primary, danger && styles.dangerBtn)}
              >
                {confirmLabel}
              </AlertDialog.Close>
            </div>
          </AlertDialog.Popup>
        </AlertDialog.Viewport>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  )
}

/**
 * Port of `OverlayDrawer` on Base UI `Dialog` — a panel anchored to an
 * edge of the viewport.
 */
export function OverlayDrawer({
  trigger,
  open,
  onOpenChange,
  side = 'right',
  width = 360,
  title,
  children,
}: {
  trigger?: ReactElement
  open?: boolean
  onOpenChange?: (open: boolean) => void
  side?: 'left' | 'right'
  width?: number
  title?: ReactNode
  children: ReactNode
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger && <Dialog.Trigger render={trigger as ReactElement<Record<string, unknown>>} />}
      <Dialog.Portal>
        <Dialog.Backdrop {...stylex.props(styles.backdrop)} />
        <Dialog.Popup
          {...stylex.props(styles.drawer, side === 'left' ? styles.drawerLeft : styles.drawerRight)}
          style={{ width }}
        >
          {title != null && (
            <Dialog.Title {...stylex.props(styles.drawerTitle)}>{title}</Dialog.Title>
          )}
          <div {...stylex.props(styles.drawerBody)}>{children}</div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

/**
 * Port of `OverlayHoverCard` on Base UI `PreviewCard` — a rich preview
 * popup shown on hover over the `trigger`.
 */
export function OverlayHoverCard({
  trigger,
  side = 'top',
  align = 'center',
  children,
}: {
  trigger: ReactElement
  side?: 'top' | 'bottom' | 'left' | 'right'
  align?: 'start' | 'center' | 'end'
  children: ReactNode
}) {
  return (
    <PreviewCard.Root>
      <PreviewCard.Trigger render={trigger as ReactElement<Record<string, unknown>>} />
      <PreviewCard.Portal>
        <PreviewCard.Positioner side={side} align={align} sideOffset={6}>
          <PreviewCard.Popup {...stylex.props(popup.surface, styles.hoverCard)}>{children}</PreviewCard.Popup>
        </PreviewCard.Positioner>
      </PreviewCard.Portal>
    </PreviewCard.Root>
  )
}

const styles = stylex.create({
  backdrop: {
    position: 'fixed',
    inset: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.45)',
    zIndex: 60,
  },
  viewport: {
    position: 'fixed',
    inset: 0,
    zIndex: 70,
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 24,
  },
  modal: {
    width: '100%',
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgOverlay,
    boxShadow: vars.shadowOverlay,
    padding: 16,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    outline: 'none',
  },
  confirm: {
    width: '100%',
    maxWidth: 360,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgOverlay,
    boxShadow: vars.shadowOverlay,
    padding: 16,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    outline: 'none',
  },
  title: {
    fontSize: 14,
    fontWeight: 600,
    color: vars.colorBase,
    margin: 0,
  },
  description: {
    fontSize: 12,
    color: vars.colorMuted,
    margin: 0,
    lineHeight: '1.5',
  },
  body: {
    fontSize: 13,
    color: vars.colorBase,
    minWidth: 0,
  },
  actions: {
    display: 'flex',
    justifyContent: 'flex-end',
    gap: 8,
    marginTop: 8,
  },
  dangerBtn: {
    backgroundColor: {
      default: vars.accentError,
      ':hover': vars.accentError,
    },
  },
  drawer: {
    position: 'fixed',
    top: 0,
    bottom: 0,
    zIndex: 70,
    backgroundColor: vars.bgOverlay,
    display: 'flex',
    flexDirection: 'column',
    outline: 'none',
    maxWidth: '90vw',
  },
  drawerRight: {
    right: 0,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderBase,
    boxShadow: '-8px 0 24px rgba(0, 0, 0, 0.12)',
  },
  drawerLeft: {
    left: 0,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderBase,
    boxShadow: '8px 0 24px rgba(0, 0, 0, 0.12)',
  },
  drawerTitle: {
    fontSize: 13,
    fontWeight: 600,
    color: vars.colorBase,
    padding: '12px 14px',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
    margin: 0,
  },
  drawerBody: {
    flex: 1,
    overflowY: 'auto',
    padding: 14,
    minHeight: 0,
  },
  hoverCard: {
    padding: 10,
    maxWidth: 320,
    fontSize: 12,
    color: vars.colorBase,
  },
})
