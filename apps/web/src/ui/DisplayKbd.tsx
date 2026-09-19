import * as stylex from '@stylexjs/stylex'
import { bindingDisplay } from './keybinding'
import { font, vars } from './tokens.stylex'

/**
 * Port of `DisplayKbd` — renders a chord string (`'mod+k'`, `'ctrl+shift+p'`,
 * multi-chord `'g g'`) as platform keycaps via `bindingDisplay`, over a
 * `bg-sunken` well. `keys` is the canonical prop; `chord` is kept as an
 * alias for existing callers.
 */
export function DisplayKbd({
  keys,
  chord,
  size = 'md',
}: {
  /** The binding string, e.g. `'mod+shift+k'` or `'g g'`. */
  keys?: string
  /** @deprecated alias for `keys`. */
  chord?: string
  size?: 'sm' | 'md'
}) {
  const tokens = bindingDisplay(keys ?? chord ?? '')
  return (
    <kbd {...stylex.props(styles.group)}>
      {tokens.map((token, i) => (
        <span key={`${token}-${i}`} {...stylex.props(styles.cap, size === 'sm' && styles.sm)}>
          {token}
        </span>
      ))}
    </kbd>
  )
}

const styles = stylex.create({
  group: {
    display: 'inline-flex',
    gap: 2,
    fontFamily: font.mono,
  },
  cap: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    minWidth: 16,
    height: 16,
    paddingLeft: 4,
    paddingRight: 4,
    borderRadius: 3,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    boxShadow: '0 1px 0 ' + vars.borderMute,
    color: vars.colorMuted,
    fontSize: 10,
    lineHeight: 1,
  },
  sm: {
    minWidth: 13,
    height: 13,
    fontSize: 9,
  },
})
