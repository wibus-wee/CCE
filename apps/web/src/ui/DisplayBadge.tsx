import * as stylex from '@stylexjs/stylex'
import type { CSSProperties, ReactNode } from 'react'
import { getHashColorFromString } from './format'
import { badges, severityBg, severityColor } from './recipes.stylex'
import type { Severity } from './tokens.stylex'
import { useColorScheme, type ColorScheme } from './useDark'

/**
 * Port of `DisplayBadge` — status/type/tag chip: hash-colored from `text`, a
 * `Severity` name, an explicit hue/color, or muted. `variant` subtle/solid,
 * `icon`, `rounded` (`md`/`full`/em number), `paddingX`/`paddingY`; sized by
 * its own font-size.
 */
export function DisplayBadge({
  text,
  color = true,
  severity,
  variant = 'subtle',
  icon,
  rounded = 'md',
  paddingX,
  paddingY,
  colorScheme,
  children,
  title,
}: {
  /** Chip text (also the hash seed when `color` is `true`). */
  text?: string
  /**
   * - `true` (default): deterministic color hashed from `text`
   * - `false`: neutral/muted
   * - `number`: explicit hue (0–360)
   * - `'#...'`/`'hsl(...)'`: explicit CSS color
   */
  color?: boolean | number | string
  /** Named severity — maps onto the scale ramp. */
  severity?: Severity
  /** `subtle` = tinted background, `solid` = filled. */
  variant?: 'subtle' | 'solid'
  icon?: ReactNode
  /** `md` (default) token radius, `full` pill, or an explicit radius in `em`. */
  rounded?: 'md' | 'full' | number
  /** Horizontal padding, in `em`. */
  paddingX?: number
  /** Vertical padding, in `em`. */
  paddingY?: number
  colorScheme?: ColorScheme
  children?: ReactNode
  title?: string
}) {
  const scheme = useColorScheme(colorScheme)
  const dark = scheme === 'dark'

  const dynamic: CSSProperties = {}
  let staticClass = ''
  const seed = text ?? ''

  if (severity) {
    staticClass = stylex.props(
      badges.base,
      severityColor[severity],
      severityBg[severity],
    ).className ?? ''
  } else if (color === false) {
    staticClass = stylex.props(badges.base, badges.muted).className ?? ''
  } else if (typeof color === 'string' && color !== 'true') {
    // Explicit CSS color or palette name — tint border+text, faint wash.
    const explicit = color
    dynamic.color = explicit
    dynamic.borderColor = explicit
    dynamic.backgroundColor = `color-mix(in srgb, ${explicit} 12%, transparent)`
    if (variant === 'solid') {
      dynamic.backgroundColor = explicit
      dynamic.color = '#fff'
      dynamic.borderColor = 'transparent'
    }
    staticClass = stylex.props(badges.base, styles.outlined).className ?? ''
  } else {
    // `true` or a numeric hue — hashed tint.
    const hue =
      typeof color === 'number'
        ? `hsla(${((color % 360) + 360) % 360}, ${dark ? 50 : 65}%, ${dark ? 60 : 40}%`
        : getHashColorFromString(seed, 1, dark).replace(/,\s*[\d.]+\)$/, '')
    const fg = `${hue}, 1)`
    const bg = `${hue}, ${variant === 'solid' ? '0.9' : '0.12'})`
    dynamic.color = variant === 'solid' ? '#fff' : fg
    dynamic.backgroundColor = bg
    staticClass = stylex.props(badges.base).className ?? ''
  }

  if (rounded === 'full') dynamic.borderRadius = 999
  else if (typeof rounded === 'number') dynamic.borderRadius = `${rounded}em`
  if (paddingX != null) {
    dynamic.paddingLeft = `${paddingX}em`
    dynamic.paddingRight = `${paddingX}em`
  }
  if (paddingY != null) {
    dynamic.paddingTop = `${paddingY}em`
    dynamic.paddingBottom = `${paddingY}em`
  }

  return (
    <span className={staticClass} style={dynamic} title={title}>
      {icon}
      {children ?? text}
    </span>
  )
}

/** `DisplayLabel` — a fully-rounded badge with tighter padding (GH tags). */
export function DisplayLabel({
  text,
  colorScheme,
  icon,
  children,
  title,
}: {
  text?: string
  colorScheme?: ColorScheme
  icon?: ReactNode
  children?: ReactNode
  title?: string
}) {
  return (
    <DisplayBadge
      text={text}
      colorScheme={colorScheme}
      rounded="full"
      paddingX={0.6}
      paddingY={0.15}
      icon={icon}
      title={title}
    >
      {children}
    </DisplayBadge>
  )
}

const styles = stylex.create({
  outlined: {
    borderWidth: 1,
    borderStyle: 'solid',
  },
})
