import * as stylex from '@stylexjs/stylex'
import { ContextMenu, Menu } from '@base-ui-components/react'
import type { ReactElement, ReactNode } from 'react'
import { popup } from './recipes.stylex'
import { vars } from './tokens.stylex'

/**
 * Ports of the `OverlayDropdown*` family on Base UI `Menu` — a trigger +
 * floating popup of items, groups, checkboxes, radios, and submenus.
 */
export function OverlayDropdown({
  trigger,
  disabled = false,
  open,
  onOpenChange,
  side = 'bottom',
  align = 'start',
  children,
}: {
  /** The anchor element — usually an `ActionButton`. */
  trigger: ReactElement
  disabled?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
  side?: 'top' | 'bottom' | 'left' | 'right'
  align?: 'start' | 'center' | 'end'
  children: ReactNode
}) {
  return (
    <Menu.Root open={open} onOpenChange={onOpenChange} disabled={disabled}>
      <Menu.Trigger render={trigger as ReactElement<Record<string, unknown>>} />
      <Menu.Portal>
        <Menu.Positioner side={side} align={align} sideOffset={4}>
          <Menu.Popup {...stylex.props(popup.surface)}>{children}</Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  )
}

export function OverlayDropdownItem({
  icon,
  shortcut,
  disabled = false,
  closeOnClick = true,
  onClick,
  children,
}: {
  icon?: ReactNode
  /** Right-aligned hint, e.g. a `DisplayKbd`. */
  shortcut?: ReactNode
  disabled?: boolean
  closeOnClick?: boolean
  onClick?: () => void
  children: ReactNode
}) {
  return (
    <Menu.Item
      disabled={disabled}
      closeOnClick={closeOnClick}
      onClick={onClick}
      {...stylex.props(popup.item)}
    >
      {icon && <span {...stylex.props(styles.itemIcon)}>{icon}</span>}
      <span {...stylex.props(styles.itemLabel)}>{children}</span>
      {shortcut && <span {...stylex.props(styles.itemShortcut)}>{shortcut}</span>}
    </Menu.Item>
  )
}

export function OverlayDropdownCheckboxItem({
  checked,
  onCheckedChange,
  disabled = false,
  children,
}: {
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <Menu.CheckboxItem
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      {...stylex.props(popup.item)}
    >
      <Menu.CheckboxItemIndicator {...stylex.props(styles.indicator)}>✓</Menu.CheckboxItemIndicator>
      <span {...stylex.props(styles.itemLabel)}>{children}</span>
    </Menu.CheckboxItem>
  )
}

export function OverlayDropdownRadioGroup({
  value,
  onValueChange,
  children,
}: {
  value: string
  onValueChange: (value: string) => void
  children: ReactNode
}) {
  return (
    <Menu.RadioGroup value={value} onValueChange={(v) => onValueChange(v as string)}>
      {children}
    </Menu.RadioGroup>
  )
}

export function OverlayDropdownRadioItem({
  value,
  disabled = false,
  children,
}: {
  value: string
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <Menu.RadioItem value={value} disabled={disabled} {...stylex.props(popup.item)}>
      <Menu.RadioItemIndicator {...stylex.props(styles.indicator)}>●</Menu.RadioItemIndicator>
      <span {...stylex.props(styles.itemLabel)}>{children}</span>
    </Menu.RadioItem>
  )
}

export function OverlayDropdownGroup({ children }: { children: ReactNode }) {
  return <Menu.Group>{children}</Menu.Group>
}

export function OverlayDropdownLabel({ children }: { children: ReactNode }) {
  return <Menu.GroupLabel {...stylex.props(popup.label)}>{children}</Menu.GroupLabel>
}

export function OverlayDropdownSeparator() {
  return <Menu.Separator {...stylex.props(popup.separator)} />
}

export function OverlayDropdownSub({
  trigger,
  disabled = false,
  children,
}: {
  /** Content of the submenu trigger row. */
  trigger: ReactNode
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <Menu.SubmenuRoot disabled={disabled}>
      <Menu.SubmenuTrigger {...stylex.props(popup.item)}>
        <span {...stylex.props(styles.itemLabel)}>{trigger}</span>
        <span {...stylex.props(styles.itemShortcut)}>›</span>
      </Menu.SubmenuTrigger>
      <Menu.Portal>
        <Menu.Positioner align="start" sideOffset={-4}>
          <Menu.Popup {...stylex.props(popup.surface)}>{children}</Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.SubmenuRoot>
  )
}

/**
 * Ports of the `OverlayContextMenu*` family on Base UI `ContextMenu` —
 * right-click menu attached to `children` as the trigger area.
 */
export function OverlayContextMenu({
  trigger,
  children,
}: {
  /** The element that opens the menu on right-click. */
  trigger: ReactElement
  children: ReactNode
}) {
  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger render={trigger as ReactElement<Record<string, unknown>>} />
      <ContextMenu.Portal>
        <ContextMenu.Positioner>
          <ContextMenu.Popup {...stylex.props(popup.surface)}>{children}</ContextMenu.Popup>
        </ContextMenu.Positioner>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  )
}

export function OverlayContextMenuItem({
  icon,
  shortcut,
  disabled = false,
  onClick,
  children,
}: {
  icon?: ReactNode
  shortcut?: ReactNode
  disabled?: boolean
  onClick?: () => void
  children: ReactNode
}) {
  return (
    <ContextMenu.Item
      disabled={disabled}
      onClick={onClick}
      {...stylex.props(popup.item)}
    >
      {icon && <span {...stylex.props(styles.itemIcon)}>{icon}</span>}
      <span {...stylex.props(styles.itemLabel)}>{children}</span>
      {shortcut && <span {...stylex.props(styles.itemShortcut)}>{shortcut}</span>}
    </ContextMenu.Item>
  )
}

export function OverlayContextMenuLabel({ children }: { children: ReactNode }) {
  return <ContextMenu.GroupLabel {...stylex.props(popup.label)}>{children}</ContextMenu.GroupLabel>
}

export function OverlayContextMenuSeparator() {
  return <ContextMenu.Separator {...stylex.props(popup.separator)} />
}

const styles = stylex.create({
  itemIcon: {
    display: 'inline-flex',
    flexShrink: 0,
    color: vars.colorMuted,
  },
  itemLabel: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  itemShortcut: {
    marginLeft: 'auto',
    color: vars.colorFaint,
    fontSize: 11,
    flexShrink: 0,
  },
  indicator: {
    width: 14,
    display: 'inline-flex',
    justifyContent: 'center',
    color: vars.colorActive,
    fontSize: 11,
    flexShrink: 0,
  },
})
