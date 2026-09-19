import * as stylex from '@stylexjs/stylex'
import { Field } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `FormField` on Base UI `Field` — wraps a control with `label`,
 * `description`, and `error` (role="alert"), `required` marker. The child
 * control is wired via `Field.Control` by the caller.
 */
export function FormField({
  label,
  description,
  error,
  required = false,
  disabled = false,
  name,
  children,
}: {
  label?: ReactNode
  description?: ReactNode
  /** Error text — renders `Field.Error` with `forceShow`. */
  error?: ReactNode
  /** Mark the label with the required asterisk. */
  required?: boolean
  disabled?: boolean
  name?: string
  children: ReactNode
}) {
  return (
    <Field.Root
      disabled={disabled}
      name={name}
      className={stylex.props(styles.root).className}
    >
      {label != null && (
        <Field.Label {...stylex.props(styles.label)}>
          {label}
          {required && <span {...stylex.props(styles.required)}>*</span>}
        </Field.Label>
      )}
      {children}
      {description != null && (
        <Field.Description {...stylex.props(styles.description)}>
          {description}
        </Field.Description>
      )}
      {error != null && (
        <Field.Error match {...stylex.props(styles.error)}>
          {error}
        </Field.Error>
      )}
    </Field.Root>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    minWidth: 0,
  },
  label: {
    fontSize: 12,
    fontWeight: 500,
    color: vars.colorMuted,
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
  },
  required: {
    color: vars.accentError,
  },
  description: {
    fontSize: 11,
    color: vars.colorFaint,
    lineHeight: '1.45',
  },
  error: {
    fontSize: 11,
    color: vars.accentError,
    lineHeight: '1.45',
  },
})
