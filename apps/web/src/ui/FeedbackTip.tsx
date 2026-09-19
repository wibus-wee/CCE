import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { IconCheck, IconError, IconInfo, IconWarning } from './icons'
import { vars } from './tokens.stylex'

/**
 * Port of `FeedbackTip` — an inline callout, info/success/warning/error.
 * Caveats (stale views, missing capabilities) ride this, never hidden.
 */
export function FeedbackTip({
  variant = 'info',
  title,
  children,
}: {
  variant?: 'info' | 'success' | 'warning' | 'error'
  title?: ReactNode
  children: ReactNode
}) {
  const Icon = { info: IconInfo, success: IconCheck, warning: IconWarning, error: IconError }[variant]
  return (
    <div {...stylex.props(styles.tip, styles[variant])} role={variant === 'error' ? 'alert' : 'status'}>
      <span {...stylex.props(styles.icon)}>
        <Icon size={14} />
      </span>
      <div {...stylex.props(styles.content)}>
        {title && <div {...stylex.props(styles.title)}>{title}</div>}
        {children}
      </div>
    </div>
  )
}

const styles = stylex.create({
  tip: {
    display: 'flex',
    gap: 8,
    padding: 10,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    fontSize: 12,
    lineHeight: '1.5',
    alignItems: 'flex-start',
  },
  icon: {
    display: 'inline-flex',
    flexShrink: 0,
    marginTop: 1,
  },
  content: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    minWidth: 0,
  },
  title: {
    fontWeight: 600,
  },
  info: {
    color: vars.accentInfo,
    backgroundColor: vars.tipInfoBg,
    borderColor: vars.tipInfoBg,
  },
  success: {
    color: vars.accentSuccess,
    backgroundColor: vars.tipSuccessBg,
    borderColor: vars.tipSuccessBg,
  },
  warning: {
    color: vars.accentWarning,
    backgroundColor: vars.tipWarningBg,
    borderColor: vars.tipWarningBg,
  },
  error: {
    color: vars.accentError,
    backgroundColor: vars.tipErrorBg,
    borderColor: vars.tipErrorBg,
  },
})
