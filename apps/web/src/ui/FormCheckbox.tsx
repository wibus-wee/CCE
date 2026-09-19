import * as stylex from '@stylexjs/stylex'
import { Checkbox } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { IconCheckSmall, IconMinus } from './icons'
import { vars } from './tokens.stylex'

/**
 * Port of `FormCheckbox` on Base UI `Checkbox` — a bordered box that fills
 * primary when checked; `indeterminate` shows the minus bar. `label` renders
 * the text alongside.
 */
export function FormCheckbox({
  checked,
  defaultChecked,
  onCheckedChange,
  indeterminate = false,
  disabled,
  label,
  name,
  value,
  required,
}: {
  checked?: boolean
  defaultChecked?: boolean
  onCheckedChange?: (checked: boolean) => void
  /** Show the indeterminate dash (overrides `checked` display). */
  indeterminate?: boolean
  disabled?: boolean
  label?: ReactNode
  name?: string
  value?: string
  required?: boolean
}) {
  return (
    <label {...stylex.props(styles.wrap, disabled && styles.disabled)}>
      <Checkbox.Root
        checked={checked}
        defaultChecked={defaultChecked}
        onCheckedChange={(next) => onCheckedChange?.(next === true)}
        indeterminate={indeterminate}
        disabled={disabled}
        name={name}
        value={value}
        required={required}
        className={(state) =>
          stylex.props(
            styles.box,
            (state.checked || state.indeterminate) && styles.filled,
          ).className
        }
      >
        <Checkbox.Indicator {...stylex.props(styles.indicator)}>
          {indeterminate ? <IconMinus size={11} /> : <IconCheckSmall size={11} />}
        </Checkbox.Indicator>
      </Checkbox.Root>
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
  box: {
    width: 15,
    height: 15,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
    cursor: 'inherit',
    outline: 'none',
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'background-color, border-color, box-shadow',
    transitionDuration: '120ms',
    padding: 0,
  },
  filled: {
    backgroundColor: vars.primary500,
    borderColor: vars.primary500,
    color: vars.onPrimary,
  },
  indicator: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    color: vars.onPrimary,
    lineHeight: 0,
  },
  label: {
    minWidth: 0,
  },
})
