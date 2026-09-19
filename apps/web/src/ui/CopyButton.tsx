import { useEffect, useRef, useState } from 'react'
import { ActionIconButton } from './ActionIconButton'
import { IconCheckSmall, IconCopy } from './icons'

/**
 * Icon-only clipboard button — writes `text` and swaps the glyph to a
 * check for a moment. The shared copy idiom for row actions and header
 * chrome: icon copies get the in-place swap, labeled actions get a toast.
 */
export function CopyButton({
  text,
  title,
  label,
  size = 10,
  compact,
  onClick,
  ref,
}: {
  text: string
  /** Tooltip while idle — falls back to `copy`. */
  title?: string
  /** Accessible label while idle — falls back to `title`. */
  label?: string
  /** Glyph size — 10–11 in dense rows, 13 in header chrome. */
  size?: number
  /** Square (non-circular) chrome for dense toolbars. */
  compact?: boolean
  onClick?: React.MouseEventHandler
  /** Lets keyboard shortcuts fire the same click path (e.g. `y` permalink). */
  ref?: React.Ref<HTMLButtonElement>
}) {
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])
  return (
    <ActionIconButton
      ref={ref}
      compact={compact}
      tooltip={copied ? 'copied' : (title ?? 'copy')}
      label={copied ? 'copied' : (label ?? title ?? 'copy')}
      icon={
        copied ? <IconCheckSmall size={size} /> : <IconCopy size={size} />
      }
      onClick={(e) => {
        e.stopPropagation()
        onClick?.(e)
        void navigator.clipboard?.writeText(text)?.catch(() => {})
        setCopied(true)
        clearTimeout(timer.current)
        timer.current = setTimeout(() => setCopied(false), 1400)
      }}
    />
  )
}
