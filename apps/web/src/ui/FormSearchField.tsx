import * as stylex from '@stylexjs/stylex'
import { forwardRef, type InputHTMLAttributes } from 'react'
import { DisplayKbd } from './DisplayKbd'
import { IconSearch, IconX } from './icons'
import { vars } from './tokens.stylex'

/**
 * Port of `FormSearchField` — search input with a leading icon, a `kbd`
 * shortcut hint, and a clear button that appears when there's text.
 */
export const FormSearchField = forwardRef<
  HTMLInputElement,
  {
    shortcut?: string
    onClear?: () => void
  } & InputHTMLAttributes<HTMLInputElement>
>(function FormSearchField({ shortcut, onClear, value, ...rest }, ref) {
  const hasText = value != null && String(value).length > 0
  return (
    <div {...stylex.props(styles.box)}>
      <span {...stylex.props(styles.icon)}>
        <IconSearch size={14} />
      </span>
      <input ref={ref} value={value} {...stylex.props(styles.input)} {...rest} />
      {hasText && onClear && (
        <button type="button" aria-label="Clear" {...stylex.props(styles.clear)} onClick={onClear}>
          <IconX size={11} />
        </button>
      )}
      {shortcut && (
        <span {...stylex.props(styles.kbd)}>
          <DisplayKbd chord={shortcut} />
        </span>
      )}
    </div>
  )
})

const styles = stylex.create({
  box: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    height: 32,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: {
      default: vars.borderBase,
      ':focus-within': vars.borderActive,
    },
    backgroundColor: vars.bgRaised,
    paddingLeft: 8,
    paddingRight: 6,
    transitionProperty: 'border-color, box-shadow',
    transitionDuration: '120ms',
    boxShadow: {
      default: 'none',
      ':focus-within': `0 0 0 2px ${vars.ringPrimary}`,
    },
  },
  icon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  input: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    outline: 'none',
    backgroundColor: 'transparent',
    color: vars.colorBase,
    fontFamily: 'inherit',
    fontSize: 13,
    lineHeight: '1.45',
    paddingTop: 6,
    paddingBottom: 6,
  },
  clear: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 16,
    height: 16,
    padding: 0,
    borderWidth: 0,
    borderRadius: '50%',
    backgroundColor: vars.bgAmbient,
    color: vars.colorMuted,
    cursor: 'pointer',
    flexShrink: 0,
  },
  kbd: {
    flexShrink: 0,
    opacity: vars.opFade,
  },
})
