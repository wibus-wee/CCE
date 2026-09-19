import * as stylex from '@stylexjs/stylex'
import { Radio, RadioGroup } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { font, vars } from './tokens.stylex'

export interface RadioOption {
  value: string
  label?: ReactNode
  description?: ReactNode
  disabled?: boolean
}

/**
 * Port of `FormRadioGroup` on Base UI `RadioGroup` + `Radio` — a vertical
 * list of circle-dot radios; `row` lays them out horizontally.
 */
export function FormRadioGroup({
  options,
  value,
  defaultValue,
  onValueChange,
  disabled,
  row = false,
  name,
}: {
  options: RadioOption[]
  value?: string
  defaultValue?: string
  onValueChange?: (value: string) => void
  disabled?: boolean
  /** Lay options out in a row instead of a column. */
  row?: boolean
  name?: string
}) {
  return (
    <RadioGroup
      value={value}
      defaultValue={defaultValue}
      onValueChange={(v) => onValueChange?.(String(v))}
      disabled={disabled}
      name={name}
      className={stylex.props(styles.group, row && styles.row).className}
    >
      {options.map((opt) => (
        <label key={opt.value} {...stylex.props(styles.option, (disabled || opt.disabled) && styles.disabled)}>
          <Radio.Root
            value={opt.value}
            disabled={opt.disabled}
            className={(state) =>
              stylex.props(styles.circle, state.checked && styles.circleOn).className
            }
          >
            <Radio.Indicator {...stylex.props(styles.dot)} />
          </Radio.Root>
          <span {...stylex.props(styles.text)}>
            {opt.label ?? opt.value}
            {opt.description != null && (
              <span {...stylex.props(styles.description)}>{opt.description}</span>
            )}
          </span>
        </label>
      ))}
    </RadioGroup>
  )
}

/**
 * Port of `FormSegmentedControl` — a single-select segmented control over an
 * `options` list, including a valid `null` segment. Also a `RadioGroup`,
 * styled as an inline border-split control.
 */
export interface SegmentedOption {
  value: string | null
  label?: ReactNode
  /** Trailing mono count — the chip+count filter idiom on a segment. */
  count?: number
  disabled?: boolean
}

const NULL_VALUE = '__null__'

export function FormSegmentedControl({
  options,
  value,
  defaultValue,
  onValueChange,
  disabled,
}: {
  options: SegmentedOption[]
  value?: string | null
  defaultValue?: string | null
  onValueChange?: (value: string | null) => void
  disabled?: boolean
}) {
  const encode = (v: string | null | undefined) => (v == null ? NULL_VALUE : v)
  const decode = (v: unknown) => (v === NULL_VALUE ? null : String(v))
  return (
    <RadioGroup
      value={encode(value)}
      defaultValue={encode(defaultValue)}
      onValueChange={(v) => onValueChange?.(decode(v))}
      disabled={disabled}
      className={stylex.props(styles.segments).className}
    >
      {options.map((opt, i) => (
        <Radio.Root
          key={opt.value ?? `null-${i}`}
          value={encode(opt.value)}
          disabled={opt.disabled}
          className={(state) =>
            stylex.props(
              styles.segment,
              i > 0 && styles.segmentBorder,
              state.checked && styles.segmentOn,
              state.disabled && styles.segmentDisabled,
            ).className
          }
        >
          {opt.label ?? (opt.value == null ? 'none' : opt.value)}
          {opt.count != null && (
            <span {...stylex.props(styles.segmentCount)}>{opt.count}</span>
          )}
        </Radio.Root>
      ))}
    </RadioGroup>
  )
}

const styles = stylex.create({
  group: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
  },
  row: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 14,
  },
  option: {
    display: 'inline-flex',
    alignItems: 'flex-start',
    gap: 7,
    cursor: 'pointer',
    userSelect: 'none',
    fontSize: 13,
    color: vars.colorBase,
    minWidth: 0,
  },
  disabled: {
    cursor: 'default',
    opacity: vars.opMute,
  },
  circle: {
    width: 15,
    height: 15,
    borderRadius: '50%',
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
    marginTop: 2,
    cursor: 'inherit',
    outline: 'none',
    padding: 0,
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'border-color, box-shadow',
    transitionDuration: '120ms',
  },
  circleOn: {
    borderColor: vars.primary500,
  },
  dot: {
    width: 7,
    height: 7,
    borderRadius: '50%',
    backgroundColor: vars.primary500,
  },
  text: {
    display: 'inline-flex',
    flexDirection: 'column',
    minWidth: 0,
  },
  description: {
    fontSize: 11,
    color: vars.colorFaint,
  },
  segments: {
    fontSize: 11,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    borderRadius: 4,
    display: 'inline-flex',
    width: 'max-content',
    overflow: 'hidden',
  },
  segment: {
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    borderWidth: 0,
    fontFamily: 'inherit',
    fontSize: 'inherit',
    color: vars.colorMuted,
    cursor: 'pointer',
    outline: 'none',
    textTransform: 'capitalize',
    transitionProperty: 'background-color, color, opacity',
    transitionDuration: '120ms',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  segmentBorder: {
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderBase,
  },
  segmentCount: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    marginLeft: 5,
  },
  segmentOn: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
    opacity: 1,
  },
  segmentDisabled: {
    opacity: vars.opMute,
    pointerEvents: 'none',
  },
})
