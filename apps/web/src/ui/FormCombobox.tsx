import * as stylex from '@stylexjs/stylex'
import { Combobox } from '@base-ui-components/react'
import { useMemo, useState, type ReactNode } from 'react'
import { IconCaretDown, IconCheckSmall, IconSearch } from './icons'
import { popup } from './recipes.stylex'
import { vars } from './tokens.stylex'

export interface ComboboxOption {
  value: string
  label?: ReactNode
  disabled?: boolean
}

/**
 * Port of `FormCombobox` on Base UI `Combobox` — a searchable, filterable
 * select: type to filter `options` by their visible label, Enter/click to
 * commit.
 */
export function FormCombobox({
  options,
  value,
  defaultValue,
  onValueChange,
  inputValue,
  onInputValueChange,
  placeholder = 'Search…',
  disabled,
}: {
  options: ComboboxOption[]
  /** The selected option's `value`. */
  value?: string
  defaultValue?: string
  onValueChange?: (value: string | null) => void
  inputValue?: string
  onInputValueChange?: (value: string) => void
  placeholder?: string
  disabled?: boolean
}) {
  const [internalQuery, setInternalQuery] = useState('')
  const query = inputValue ?? internalQuery

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return options
    return options.filter((o) => {
      const label = typeof o.label === 'string' ? o.label : o.value
      return label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q)
    })
  }, [options, query])

  return (
    <Combobox.Root
      items={filtered}
      value={value}
      defaultValue={defaultValue}
      onValueChange={(v) => onValueChange?.(v as string | null)}
      inputValue={inputValue}
      onInputValueChange={(v) => {
        setInternalQuery(v)
        onInputValueChange?.(v)
      }}
      itemToStringValue={(v) => {
        const opt = options.find(o => o.value === v)
        return typeof opt?.label === 'string' ? opt.label : String(v ?? '')
      }}
      disabled={disabled}
    >
      <div {...stylex.props(styles.anchor)}>
        <IconSearch size={12} />
        <Combobox.Input
          placeholder={placeholder}
          {...stylex.props(styles.input)}
        />
        <Combobox.Trigger {...stylex.props(styles.trigger)} aria-label="Toggle options">
          <IconCaretDown size={12} />
        </Combobox.Trigger>
      </div>
      <Combobox.Portal>
        <Combobox.Positioner sideOffset={6} align="start">
          <Combobox.Popup {...stylex.props(popup.surface)}>
            <Combobox.Empty {...stylex.props(styles.empty)}>No matches</Combobox.Empty>
            <Combobox.List>
              {(opt: ComboboxOption) => (
                <Combobox.Item
                  key={opt.value}
                  value={opt.value}
                  disabled={opt.disabled}
                  className={(state) =>
                    stylex.props(
                      popup.item,
                      state.highlighted && popup.itemHighlighted,
                      state.selected && popup.itemSelected,
                      state.disabled && popup.itemDisabled,
                    ).className
                  }
                >
                  <Combobox.ItemIndicator {...stylex.props(styles.indicator)}>
                    <IconCheckSmall size={12} />
                  </Combobox.ItemIndicator>
                  <span {...stylex.props(styles.itemText)}>{opt.label ?? opt.value}</span>
                </Combobox.Item>
              )}
            </Combobox.List>
          </Combobox.Popup>
        </Combobox.Positioner>
      </Combobox.Portal>
    </Combobox.Root>
  )
}

const styles = stylex.create({
  anchor: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    height: 32,
    minWidth: 160,
    paddingLeft: 8,
    paddingRight: 6,
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    color: vars.colorFaint,
    boxShadow: { ':focus-within': `0 0 0 2px ${vars.ringPrimary}` },
    transitionProperty: 'border-color, box-shadow',
    transitionDuration: '120ms',
  },
  input: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: {
      default: vars.colorBase,
      '::placeholder': vars.colorFaint,
    },
    fontFamily: 'inherit',
    fontSize: 13,
    lineHeight: '1.4',
    outline: 'none',
    padding: 0,
  },
  trigger: {
    display: 'inline-flex',
    alignItems: 'center',
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorFaint,
    cursor: 'pointer',
    padding: 2,
    outline: 'none',
  },
  indicator: {
    display: 'inline-flex',
    width: 14,
    flexShrink: 0,
    color: vars.colorActive,
  },
  itemText: {
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  empty: {
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 8,
    paddingRight: 8,
    fontSize: 12,
    color: vars.colorFaint,
  },
})
