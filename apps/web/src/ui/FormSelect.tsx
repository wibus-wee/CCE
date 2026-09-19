import * as stylex from '@stylexjs/stylex'
import { Select } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { IconCaretDown, IconCheckSmall } from './icons'
import { popup } from './recipes.stylex'
import { vars } from './tokens.stylex'

export interface SelectOption {
  value: string
  label?: ReactNode
  icon?: ReactNode
  disabled?: boolean
}

/**
 * Port of `FormSelect` on Base UI `Select` — trigger + popup list over an
 * `options` list, item indicator check, `fieldBase`-sized trigger.
 */
export function FormSelect({
  options,
  value,
  defaultValue,
  onValueChange,
  placeholder = 'Select…',
  disabled,
  align = 'start',
}: {
  options: SelectOption[]
  value?: string
  defaultValue?: string
  onValueChange?: (value: string) => void
  placeholder?: string
  disabled?: boolean
  align?: 'start' | 'center' | 'end'
}) {
  return (
    <Select.Root
      value={value}
      defaultValue={defaultValue}
      onValueChange={(v) => onValueChange?.(v as string)}
      disabled={disabled}
    >
      <Select.Trigger {...stylex.props(styles.trigger)}>
        <Select.Value
          {...stylex.props(styles.value)}
        >
          {(selected) => {
            const opt = options.find(o => o.value === selected)
            return opt ? (
              <span {...stylex.props(styles.valueInner)}>
                {opt.icon}
                {opt.label ?? opt.value}
              </span>
            ) : (
              <span {...stylex.props(styles.placeholder)}>{placeholder}</span>
            )
          }}
        </Select.Value>
        <Select.Icon {...stylex.props(styles.icon)}>
          <IconCaretDown size={12} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner sideOffset={6} align={align} alignItemWithTrigger={false}>
          <Select.Popup {...stylex.props(popup.surface)}>
            <Select.List>
              {options.map((opt) => (
                <Select.Item
                  key={opt.value}
                  value={opt.value}
                  disabled={opt.disabled}
                  label={typeof opt.label === 'string' ? opt.label : opt.value}
                  className={(state) =>
                    stylex.props(
                      popup.item,
                      state.highlighted && popup.itemHighlighted,
                      state.selected && popup.itemSelected,
                      state.disabled && popup.itemDisabled,
                    ).className
                  }
                >
                  <Select.ItemIndicator {...stylex.props(styles.indicator)}>
                    <IconCheckSmall size={12} />
                  </Select.ItemIndicator>
                  <Select.ItemText {...stylex.props(styles.itemText)}>
                    {opt.icon}
                    {opt.label ?? opt.value}
                  </Select.ItemText>
                </Select.Item>
              ))}
            </Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  )
}

const styles = stylex.create({
  trigger: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 8,
    height: 32,
    minWidth: 120,
    paddingLeft: 10,
    paddingRight: 8,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    color: vars.colorBase,
    fontFamily: 'inherit',
    fontSize: 13,
    lineHeight: '1.4',
    cursor: 'pointer',
    outline: 'none',
    boxShadow: { ':focus-visible': `0 0 0 2px ${vars.ringPrimary}` },
    opacity: { ':disabled': vars.opMute },
    pointerEvents: { ':disabled': 'none' },
    transitionProperty: 'border-color, box-shadow, opacity',
    transitionDuration: '120ms',
  },
  value: {
    display: 'inline-flex',
    alignItems: 'center',
    minWidth: 0,
    overflow: 'hidden',
  },
  valueInner: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  placeholder: {
    color: vars.colorFaint,
  },
  icon: {
    display: 'inline-flex',
    opacity: 0.55,
    flexShrink: 0,
  },
  indicator: {
    display: 'inline-flex',
    width: 14,
    flexShrink: 0,
    color: vars.colorActive,
  },
  itemText: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
})
