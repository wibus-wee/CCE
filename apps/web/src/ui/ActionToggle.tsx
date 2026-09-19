import * as stylex from '@stylexjs/stylex'
import { Toggle } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `ActionToggle` on Base UI `Toggle` — a single pressed/unpressed
 * button with optional `icon` + `label`.
 */
export function ActionToggle({
  pressed,
  defaultPressed,
  onPressedChange,
  icon,
  label,
  disabled,
}: {
  pressed?: boolean
  defaultPressed?: boolean
  onPressedChange?: (pressed: boolean) => void
  icon?: ReactNode
  label?: ReactNode
  disabled?: boolean
}) {
  return (
    <Toggle
      pressed={pressed}
      defaultPressed={defaultPressed}
      onPressedChange={(next) => onPressedChange?.(next)}
      disabled={disabled}
      className={(state) =>
        stylex.props(styles.toggle, state.pressed && styles.on).className
      }
    >
      {icon}
      {label != null && <span>{label}</span>}
    </Toggle>
  )
}

const styles = stylex.create({
  toggle: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorMuted,
    fontSize: 13,
    lineHeight: '1.4',
    fontFamily: 'inherit',
    cursor: 'pointer',
    opacity: { ':disabled': vars.opMute },
    outline: 'none',
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'background-color, color, opacity, box-shadow',
    transitionDuration: '120ms',
    pointerEvents: { ':disabled': 'none' },
  },
  on: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
    borderColor: vars.borderActive,
  },
})
