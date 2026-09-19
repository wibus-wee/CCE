import * as stylex from '@stylexjs/stylex'
import { Switch } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `FormSwitch` on Base UI `Switch` — a thumb sliding in a sunken
 * track that goes primary when on. `label` renders alongside.
 */
export function FormSwitch({
  checked,
  defaultChecked,
  onCheckedChange,
  disabled,
  label,
  name,
}: {
  checked?: boolean
  defaultChecked?: boolean
  onCheckedChange?: (checked: boolean) => void
  disabled?: boolean
  label?: ReactNode
  name?: string
}) {
  return (
    <label {...stylex.props(styles.wrap, disabled && styles.disabled)}>
      <Switch.Root
        checked={checked}
        defaultChecked={defaultChecked}
        onCheckedChange={(next) => onCheckedChange?.(next)}
        disabled={disabled}
        name={name}
        className={(state) => stylex.props(styles.track, state.checked && styles.on).className}
      >
        <Switch.Thumb
          className={(state) =>
            stylex.props(styles.thumb, state.checked && styles.thumbOn).className
          }
        />
      </Switch.Root>
      {label != null && <span {...stylex.props(styles.label)}>{label}</span>}
    </label>
  )
}

const styles = stylex.create({
  wrap: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 7,
    cursor: 'pointer',
    userSelect: 'none',
    fontSize: 13,
    color: vars.colorBase,
  },
  disabled: {
    cursor: 'default',
    opacity: vars.opMute,
  },
  track: {
    width: 30,
    height: 17,
    borderRadius: 999,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgSunken,
    display: 'inline-flex',
    alignItems: 'center',
    padding: 1,
    boxSizing: 'border-box',
    flexShrink: 0,
    cursor: 'inherit',
    outline: 'none',
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'background-color, border-color, box-shadow',
    transitionDuration: '150ms',
  },
  on: {
    backgroundColor: vars.primary500,
    borderColor: vars.primary500,
  },
  thumb: {
    width: 13,
    height: 13,
    borderRadius: '50%',
    backgroundColor: '#fff',
    boxShadow: '0 1px 2px rgba(0, 0, 0, 0.18)',
    transform: 'translateX(0)',
    transitionProperty: 'transform',
    transitionDuration: '150ms',
  },
  thumbOn: {
    transform: 'translateX(13px)',
  },
  label: {
    minWidth: 0,
  },
})
