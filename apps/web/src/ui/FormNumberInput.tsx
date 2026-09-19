import * as stylex from '@stylexjs/stylex'
import { NumberField } from '@base-ui-components/react'
import { IconMinus, IconPlus } from './icons'
import { field } from './recipes.stylex'
import { font, vars } from './tokens.stylex'

/**
 * Port of `FormNumberInput` on Base UI `NumberField` — numeric input with
 * `min`/`max`/`step` clamping and optional −/+ stepper `controls`.
 */
export function FormNumberInput({
  value,
  defaultValue,
  onValueChange,
  min,
  max,
  step = 1,
  controls = false,
  disabled,
  invalid,
  placeholder,
  mono = true,
  name,
  id,
  'aria-label': ariaLabel,
  title,
}: {
  value?: number | null
  defaultValue?: number | null
  onValueChange?: (value: number | null) => void
  min?: number
  max?: number
  step?: number
  /** Show the −/+ stepper buttons. */
  controls?: boolean
  disabled?: boolean
  invalid?: boolean
  placeholder?: string
  /** Monospace numerals (default `true` — technical values are mono). */
  mono?: boolean
  name?: string
  id?: string
  'aria-label'?: string
  title?: string
}) {
  return (
    <NumberField.Root
      value={value ?? undefined}
      defaultValue={defaultValue ?? undefined}
      onValueChange={(v) => onValueChange?.(v)}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      name={name}
      id={id}
      className={stylex.props(styles.root).className}
    >
      <NumberField.Group {...stylex.props(styles.group)}>
        {controls && (
          <NumberField.Decrement {...stylex.props(styles.stepper, styles.stepperLeft)} aria-label="Decrement">
            <IconMinus size={11} />
          </NumberField.Decrement>
        )}
        <NumberField.Input
          placeholder={placeholder}
          aria-label={ariaLabel}
          title={title}
          {...stylex.props(
            field.base,
            mono && styles.mono,
            invalid && styles.invalid,
            controls && styles.inputControls,
          )}
        />
        {controls && (
          <NumberField.Increment {...stylex.props(styles.stepper, styles.stepperRight)} aria-label="Increment">
            <IconPlus size={11} />
          </NumberField.Increment>
        )}
      </NumberField.Group>
    </NumberField.Root>
  )
}

const styles = stylex.create({
  root: {
    display: 'inline-flex',
    width: '100%',
    height: '100%',
  },
  group: {
    display: 'flex',
    alignItems: 'stretch',
    width: '100%',
    height: '100%',
  },
  inputControls: {
    textAlign: 'center',
    borderRadius: 0,
  },
  mono: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
  },
  invalid: {
    borderColor: vars.borderCritical,
  },
  stepper: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 26,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgSunken,
    color: vars.colorMuted,
    cursor: 'pointer',
    padding: 0,
    outline: 'none',
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'background-color, color',
    transitionDuration: '120ms',
  },
  stepperLeft: {
    borderTopLeftRadius: 4,
    borderBottomLeftRadius: 4,
    borderRightWidth: 0,
  },
  stepperRight: {
    borderTopRightRadius: 4,
    borderBottomRightRadius: 4,
    borderLeftWidth: 0,
  },
})
