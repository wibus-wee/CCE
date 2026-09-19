import * as stylex from '@stylexjs/stylex'
import { DisplayKbd } from '../ui/DisplayKbd'
import { OverlayModal } from '../ui/OverlayDialogs'
import { font, vars } from '../ui/tokens.stylex'

const SHORTCUTS: { group: string; keys: string; action: string }[] = [
  { group: 'Global', keys: 'mod+k', action: 'Command palette — search, navigate, act' },
  { group: 'Global', keys: 'mod+b', action: 'Toggle sidebar' },
  { group: 'Global', keys: '/', action: 'Focus search' },
  { group: 'Global', keys: '1', action: 'Go to Home' },
  { group: 'Global', keys: '2', action: 'Go to Query' },
  { group: 'Global', keys: '3', action: 'Go to Browse' },
  { group: 'Global', keys: '4', action: 'Go to Symbols' },
  { group: 'Global', keys: '5', action: 'Go to Context' },
  { group: 'Global', keys: '6', action: 'Go to Index' },
  { group: 'Global', keys: 'y', action: 'Copy canonical link (path + line anchor)' },
  { group: 'Global', keys: '?', action: 'This dialog' },
  { group: 'Global', keys: 'Escape', action: 'Close dialogs and menus' },
  { group: 'Results', keys: 'j', action: 'Select next hit' },
  { group: 'Results', keys: 'k', action: 'Select previous hit' },
  { group: 'Results', keys: 'h', action: 'Collapse the selected hit' },
  { group: 'Results', keys: 'l', action: 'Expand the selected hit' },
  { group: 'Results', keys: 'Enter', action: 'Expand the selected hit inline' },
  { group: 'Results', keys: 'o', action: 'Open the selected hit at its source line' },
  { group: 'Results', keys: 'mod+Enter', action: 'Open the selected hit in a new tab' },
  { group: 'Palette', keys: 'ArrowUp', action: 'Previous command' },
  { group: 'Palette', keys: 'ArrowDown', action: 'Next command' },
  { group: 'Palette', keys: 'Enter', action: 'Run the highlighted command' },
]

export function ShortcutsModal({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  return (
    <OverlayModal
      open={open}
      onOpenChange={onOpenChange}
      title="Keyboard shortcuts"
      width={380}
    >
      <div {...stylex.props(styles.list)}>
        {(['Global', 'Results', 'Palette'] as const).map((group) => (
          <div key={group}>
            <div {...stylex.props(styles.groupLabel)}>{group}</div>
            {SHORTCUTS.filter((s) => s.group === group).map((shortcut) => (
              <div key={shortcut.keys + shortcut.action} {...stylex.props(styles.row)}>
                <span {...stylex.props(styles.action)}>{shortcut.action}</span>
                <DisplayKbd keys={shortcut.keys} />
              </div>
            ))}
          </div>
        ))}
      </div>
    </OverlayModal>
  )
}

const styles = stylex.create({
  list: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  groupLabel: {
    fontSize: 9.5,
    color: vars.colorFaint,
    letterSpacing: '0.1em',
    textTransform: 'uppercase',
    paddingTop: 10,
    paddingBottom: 4,
  },
  row: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: 16,
    paddingTop: 5,
    paddingBottom: 5,
  },
  action: {
    fontSize: 12,
    color: vars.colorMuted,
    fontFamily: font.sans,
  },
})
