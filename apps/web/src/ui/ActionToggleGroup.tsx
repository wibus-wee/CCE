import * as stylex from '@stylexjs/stylex'
import { Toggle, ToggleGroup } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

export interface ToggleGroupOption {
  value: string
  label?: ReactNode
  icon?: ReactNode
  disabled?: boolean
}

/**
 * Port of `ActionToggleGroup` on Base UI `ToggleGroup` — a segmented set of
 * toggles, single- or multi-select (`multiple`), optionally `iconOnly`.
 */
export function ActionToggleGroup({
  options,
  value,
  defaultValue,
  onValueChange,
  multiple = false,
  iconOnly = false,
  disabled,
  'aria-label': ariaLabel,
}: {
  options: ToggleGroupOption[]
  /** Controlled selection — `string` single, `string[]` when `multiple`. */
  value?: string | string[]
  defaultValue?: string | string[]
  onValueChange?: (value: string | string[]) => void
  /** Single selection (default) or multi-select. */
  multiple?: boolean
  /** Icon-only segments; pass `label` for the a11y name. */
  iconOnly?: boolean
  disabled?: boolean
  'aria-label'?: string
}) {
  const toArray = (v: string | string[] | undefined) =>
    v == null ? undefined : Array.isArray(v) ? v : [v]

  return (
    <ToggleGroup
      value={toArray(value) as string[] | undefined}
      defaultValue={toArray(defaultValue) as string[] | undefined}
      onValueChange={(groupValue) => {
        const arr = groupValue as string[]
        onValueChange?.(multiple ? arr : (arr[0] ?? ''))
      }}
      multiple={multiple}
      disabled={disabled}
      aria-label={ariaLabel}
      className={stylex.props(styles.group).className}
    >
      {options.map((opt) => (
        <Toggle
          key={opt.value}
          value={opt.value}
          disabled={opt.disabled}
          aria-label={typeof opt.label === 'string' ? opt.label : opt.value}
          className={(state) =>
            stylex.props(
              styles.item,
              iconOnly ? styles.iconOnly : styles.withLabel,
              state.pressed && styles.on,
            ).className
          }
        >
          {opt.icon}
          {!iconOnly && <span>{opt.label ?? opt.value}</span>}
        </Toggle>
      ))}
    </ToggleGroup>
  )
}

const styles = stylex.create({
  group: {
    padding: 3,
    borderRadius: 8,
    backgroundColor: vars.bgSunken,
    display: 'inline-flex',
    alignItems: 'stretch',
    gap: 3,
    width: 'max-content',
  },
  item: {
    fontSize: 13,
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorBase,
    },
    outline: 'none',
    borderRadius: 6,
    display: 'flex',
    gap: 6,
    alignItems: 'center',
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    cursor: 'pointer',
    transitionProperty: 'background-color, color, box-shadow, opacity',
    transitionDuration: '120ms',
    opacity: { ':disabled': 0.5 },
    pointerEvents: { ':disabled': 'none' },
  },
  withLabel: {
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 12,
    paddingRight: 12,
  },
  iconOnly: {
    padding: 6,
  },
  on: {
    color: vars.colorActive,
    backgroundColor: vars.bgRaised,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    boxShadow: `0 1px 2px rgba(0, 0, 0, 0.12)`,
    marginTop: -1,
    marginBottom: -1,
    marginLeft: -1,
    marginRight: -1,
  },
})
