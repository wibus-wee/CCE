import * as stylex from '@stylexjs/stylex'
import type { CSSProperties, ReactNode } from 'react'
import { buttons } from './recipes.stylex'

const styles = stylex.create({
  sm: { fontSize: 12 },
})

export type ActionButtonVariant = 'action' | 'primary' | 'text'

/**
 * Port of `ActionButton` — `variant` action/primary/text, polymorphic via
 * `href`/`as`, `icon`, `loading` swaps in a spinner glyph. Peers at one size;
 * `size="sm"` only changes type scale, never padding.
 */
export function ActionButton({
  variant = 'action',
  size,
  icon,
  loading,
  as,
  href,
  children,
  disabled,
  style,
  ...rest
}: {
  variant?: ActionButtonVariant
  size?: 'sm' | 'md'
  icon?: ReactNode
  loading?: boolean
  /** Render as another element/component (e.g. a router link). */
  as?: 'button' | 'a'
  href?: string
  children?: ReactNode
  disabled?: boolean
  style?: CSSProperties
} & Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'style'> &
  Omit<React.AnchorHTMLAttributes<HTMLAnchorElement>, 'style'>) {
  const isLink = href != null || as === 'a'
  const cls = stylex.props(buttons[variant], size === 'sm' && styles.sm).className
  const content = (
    <>
      {loading ? <SpinnerGlyph /> : icon}
      {children}
    </>
  )
  if (isLink) {
    return (
      <a href={href} className={cls} style={style} {...(rest as React.AnchorHTMLAttributes<HTMLAnchorElement>)}>
        {content}
      </a>
    )
  }
  return (
    <button
      type="button"
      disabled={disabled || loading}
      className={cls}
      style={style}
      {...(rest as React.ButtonHTMLAttributes<HTMLButtonElement>)}
    >
      {content}
    </button>
  )
}

export function SpinnerGlyph() {
  return (
    <svg
      width="13"
      height="13"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      aria-hidden
      {...stylex.props(spinStyles.svg)}
    >
      <path d="M8 1.5a6.5 6.5 0 1 1-6.13 8.7" />
    </svg>
  )
}

const spinStyles = stylex.create({
  svg: {
    animationName: stylex.keyframes({ to: { transform: 'rotate(360deg)' } }),
    animationDuration: '0.8s',
    animationTimingFunction: 'linear',
    animationIterationCount: 'infinite',
    flexShrink: 0,
  },
})
