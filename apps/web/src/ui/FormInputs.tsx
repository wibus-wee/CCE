import * as stylex from '@stylexjs/stylex'
import { Input } from '@base-ui-components/react'
import type { InputHTMLAttributes, ReactNode, TextareaHTMLAttributes } from 'react'
import { field } from './recipes.stylex'
import { font, vars } from './tokens.stylex'

/**
 * `FormTextInput` / `FormTextarea` on Base UI `Input` — the single-line and
 * multi-line controls sharing `fieldBase` chrome (raised fill, base border,
 * primary focus ring). `invalid` flips the border to critical.
 */
export function FormTextInput({
  icon,
  invalid,
  mono = false,
  ...rest
}: {
  icon?: ReactNode
  invalid?: boolean
  /** Monospace value (paths, ids). */
  mono?: boolean
} & InputHTMLAttributes<HTMLInputElement>) {
  return (
    <span {...stylex.props(styles.wrap, icon != null && styles.withIcon)}>
      {icon != null && <span {...stylex.props(styles.icon)}>{icon}</span>}
      <Input
        {...stylex.props(
          field.base,
          mono && styles.mono,
          icon != null && styles.fieldIcon,
          invalid && styles.invalid,
        )}
        {...rest}
      />
    </span>
  )
}

export function FormTextarea({
  invalid,
  mono = false,
  resize = true,
  rows = 3,
  ...rest
}: {
  invalid?: boolean
  mono?: boolean
  /** Allow manual resize (default `true`). */
  resize?: boolean
} & TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      rows={rows}
      {...stylex.props(
        field.base,
        styles.textarea,
        mono && styles.mono,
        !resize && styles.noResize,
        invalid && styles.invalid,
      )}
      {...rest}
    />
  )
}

const styles = stylex.create({
  wrap: {
    position: 'relative',
    display: 'flex',
    alignItems: 'center',
    width: '100%',
  },
  withIcon: {
    width: '100%',
  },
  icon: {
    position: 'absolute',
    left: 8,
    display: 'inline-flex',
    alignItems: 'center',
    opacity: 0.55,
    pointerEvents: 'none',
  },
  fieldIcon: {
    paddingLeft: 26,
  },
  mono: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
  },
  textarea: {
    minHeight: 60,
    lineHeight: '1.5',
  },
  noResize: {
    resize: 'none',
  },
  invalid: {
    borderColor: vars.borderCritical,
  },
})
