import * as stylex from '@stylexjs/stylex'
import { Toast } from '@base-ui-components/react'
import { createContext, useContext, type ReactNode } from 'react'
import { IconCheck, IconError, IconInfo, IconWarning, IconX } from './icons'
import { vars } from './tokens.stylex'

export type ToastType = 'info' | 'success' | 'warning' | 'error'

const TYPE_ICON: Record<ToastType, ReactNode> = {
  info: <IconInfo size={13} />,
  success: <IconCheck size={13} />,
  warning: <IconWarning size={13} />,
  error: <IconError size={13} />,
}

/**
 * Port of `FeedbackToasts` on Base UI `Toast` — a positioned toast stack.
 * The app owns the list through `useToast()` (mirroring the package's
 * contract: the app owns ids and dismissal via its own composable).
 */
export function FeedbackToasts({
  position = 'bottom-right',
  limit = 4,
  children,
}: {
  position?: 'top-right' | 'bottom-right' | 'top-left' | 'bottom-left'
  limit?: number
  children?: ReactNode
}) {
  return (
    <Toast.Provider limit={limit}>
      {children}
      <Toast.Viewport
        className={stylex.props(
          styles.viewport,
          styles[
            position === 'top-right'
              ? 'topRight'
              : position === 'top-left'
                ? 'topLeft'
                : position === 'bottom-left'
                  ? 'bottomLeft'
                  : 'bottomRight'
          ],
        ).className}
      >
        <ToastStack />
      </Toast.Viewport>
    </Toast.Provider>
  )
}

function ToastStack() {
  const { toasts } = Toast.useToastManager()
  return (
    <>
      {toasts.map((toast) => {
        const type = ((toast.type as ToastType | undefined) ?? 'info') as ToastType
        return (
          <Toast.Root key={toast.id} toast={toast} {...stylex.props(styles.toast)}>
            <Toast.Content {...stylex.props(styles.content)}>
              <span {...stylex.props(styles.icon, styles[`icon_${type}`])}>
                {TYPE_ICON[type]}
              </span>
              <span {...stylex.props(styles.body)}>
                <Toast.Title {...stylex.props(styles.title)} />
                {toast.description != null && (
                  <Toast.Description {...stylex.props(styles.description)} />
                )}
              </span>
              <Toast.Close {...stylex.props(styles.close)} aria-label="Dismiss">
                <IconX size={11} />
              </Toast.Close>
            </Toast.Content>
          </Toast.Root>
        )
      })}
    </>
  )
}

/** `useToast` — the app's own toast store (port of `useNotification`). */
export function useToast() {
  const manager = Toast.useToastManager()
  return {
    push: (
      title: string,
      options?: { description?: string; type?: ToastType; timeout?: number },
    ) =>
      manager.add({
        title,
        description: options?.description,
        type: options?.type ?? 'info',
        timeout: options?.timeout ?? 5000,
      }),
    dismiss: manager.close,
    toasts: manager.toasts,
  }
}

// `provideNotification`/`useNotification` — optional context so deep children
// can post toasts without prop drilling.
const ToastContext = createContext<ReturnType<typeof useToast> | null>(null)

export function NotificationProvider({ children }: { children: ReactNode }) {
  return (
    <FeedbackToasts>
      <ToastBridge>{children}</ToastBridge>
    </FeedbackToasts>
  )
}

function ToastBridge({ children }: { children: ReactNode }) {
  const toast = useToast()
  return <ToastContext.Provider value={toast}>{children}</ToastContext.Provider>
}

export function useNotification() {
  const ctx = useContext(ToastContext)
  if (!ctx) throw new Error('useNotification must be used inside NotificationProvider')
  return ctx
}

const styles = stylex.create({
  viewport: {
    position: 'fixed',
    zIndex: 80,
    display: 'flex',
    flexDirection: 'column',
    gap: 8,
    width: 320,
    maxWidth: '90vw',
    padding: 8,
  },
  topRight: { top: 12, right: 12 },
  bottomRight: { bottom: 12, right: 12 },
  topLeft: { top: 12, left: 12 },
  bottomLeft: { bottom: 12, left: 12 },
  toast: {
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgTooltip,
    backdropFilter: 'blur(12px)',
    boxShadow: vars.shadowOverlay,
  },
  content: {
    display: 'flex',
    alignItems: 'flex-start',
    gap: 8,
    padding: 10,
  },
  icon: {
    display: 'inline-flex',
    flexShrink: 0,
    marginTop: 1,
  },
  icon_info: { color: vars.accentInfo },
  icon_success: { color: vars.accentSuccess },
  icon_warning: { color: vars.accentWarning },
  icon_error: { color: vars.accentError },
  body: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  title: {
    fontSize: 12,
    fontWeight: 600,
    color: vars.colorBase,
    lineHeight: '1.4',
  },
  description: {
    fontSize: 11,
    color: vars.colorMuted,
    lineHeight: '1.45',
  },
  close: {
    display: 'inline-flex',
    alignItems: 'center',
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: {
      default: vars.colorFaint,
      ':hover': vars.colorBase,
    },
    cursor: 'pointer',
    padding: 2,
    borderRadius: 4,
    flexShrink: 0,
  },
})
