import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

/** `FeedbackSpinner` — the spinner glyph (also used by `FeedbackLoading`). */
export function FeedbackSpinner({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      role="status"
      aria-label="Loading"
      {...stylex.props(styles.spinner)}
    >
      <path d="M8 1.5a6.5 6.5 0 1 1-6.13 8.7" />
    </svg>
  )
}

/**
 * `FeedbackEmptyState` — an icon + title + description placeholder over the
 * `bg-dots` pattern, with an `#action` slot.
 */
export function FeedbackEmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon?: ReactNode
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
}) {
  return (
    <div {...stylex.props(styles.empty)}>
      {icon && <span {...stylex.props(styles.emptyIcon)}>{icon}</span>}
      <div {...stylex.props(styles.emptyTitle)}>{title}</div>
      {description && <div {...stylex.props(styles.emptyDesc)}>{description}</div>}
      {action && <div {...stylex.props(styles.emptyAction)}>{action}</div>}
    </div>
  )
}

/** `FeedbackLoading` — a loading placeholder wrapping `FeedbackSpinner`. */
export function FeedbackLoading({ label }: { label?: ReactNode }) {
  return (
    <div {...stylex.props(styles.loading)}>
      <FeedbackSpinner />
      {label && <span>{label}</span>}
    </div>
  )
}

/** `FeedbackSkeleton` — a shimmer placeholder block for loading content. */
export function FeedbackSkeleton({
  height = 64,
  width,
  rounded = 8,
}: {
  height?: number | string
  width?: number | string
  rounded?: number
}) {
  return <div {...stylex.props(styles.skeleton)} style={{ height, width, borderRadius: rounded }} />
}

const pulse = stylex.keyframes({
  '0%': { opacity: 0.5 },
  '50%': { opacity: 0.25 },
  '100%': { opacity: 0.5 },
})

const styles = stylex.create({
  spinner: {
    animationName: stylex.keyframes({ to: { transform: 'rotate(360deg)' } }),
    animationDuration: '0.8s',
    animationTimingFunction: 'linear',
    animationIterationCount: 'infinite',
    flexShrink: 0,
  },
  empty: {
    display: 'flex',
    flexDirection: 'column',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 6,
    paddingTop: 28,
    paddingBottom: 28,
    paddingLeft: 16,
    paddingRight: 16,
    textAlign: 'center',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'dashed',
    borderColor: vars.borderBase,
    backgroundImage: `radial-gradient(circle, ${vars.borderBase} 1px, transparent 1px)`,
    backgroundSize: '16px 16px',
    backgroundPosition: 'center',
  },
  emptyIcon: {
    color: vars.colorFaint,
    display: 'inline-flex',
    marginBottom: 2,
  },
  emptyTitle: {
    fontSize: 13,
    fontWeight: 500,
    color: vars.colorBase,
  },
  emptyDesc: {
    fontSize: 12,
    color: vars.colorMuted,
    maxWidth: 420,
  },
  emptyAction: {
    marginTop: 6,
  },
  loading: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    gap: 8,
    padding: 24,
    color: vars.colorMuted,
    fontSize: 12,
  },
  skeleton: {
    borderRadius: 8,
    backgroundColor: vars.bgAmbient,
    animationName: pulse,
    animationDuration: '1.6s',
    animationTimingFunction: 'ease-in-out',
    animationIterationCount: 'infinite',
  },
})
