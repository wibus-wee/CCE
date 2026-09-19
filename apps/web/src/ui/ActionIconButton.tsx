import * as stylex from '@stylexjs/stylex'
import type { ReactNode } from 'react'
import { iconButtons } from './recipes.stylex'
import { OverlayTooltip } from './OverlayTooltip'

/**
 * Port of `ActionIconButton` — icon-only button, `tooltip` via `OverlayTooltip`,
 * `active` tint, `compact` square for dense toolbars, `badge` adornment.
 * `href`/`as` render a link.
 */
export function ActionIconButton({
  icon,
  tooltip,
  active,
  disabled,
  label,
  compact,
  badge,
  as,
  href,
  children,
  onClick,
  ref,
  ...rest
}: {
  icon?: ReactNode
  tooltip?: string
  active?: boolean
  disabled?: boolean
  /** Accessible label when the button has no visible text. */
  label?: string
  /** Compact, square (non-circular) icon button for dense toolbars. */
  compact?: boolean
  /** Corner adornment — the `#badge` slot. */
  badge?: ReactNode
  as?: 'button' | 'a'
  href?: string
  children?: ReactNode
  onClick?: React.MouseEventHandler
  ref?: React.Ref<HTMLButtonElement>
} & Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'children'>) {
  const cls = stylex.props(
    compact ? iconButtons.square : iconButtons.round,
    active && iconButtons.active,
    styles.wrap,
  ).className

  const body = (
    <>
      {icon}
      {children}
      {badge != null && <span {...stylex.props(styles.badge)}>{badge}</span>}
    </>
  )

  const el =
    href != null || as === 'a' ? (
      <a href={href} aria-label={label} className={cls} onClick={onClick} {...(rest as object)}>
        {body}
      </a>
    ) : (
      <button
        ref={ref}
        type="button"
        aria-label={label ?? tooltip}
        disabled={disabled}
        className={cls}
        onClick={onClick}
        {...rest}
      >
        {body}
      </button>
    )

  return tooltip ? (
    <OverlayTooltip content={tooltip}>{el}</OverlayTooltip>
  ) : (
    el
  )
}

const styles = stylex.create({
  wrap: {
    position: 'relative',
  },
  badge: {
    position: 'absolute',
    top: -2,
    right: -2,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    minWidth: 12,
    height: 12,
    paddingLeft: 3,
    paddingRight: 3,
    borderRadius: 6,
    fontSize: 9,
    lineHeight: 1,
  },
})
