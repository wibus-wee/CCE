import * as stylex from '@stylexjs/stylex'
import { useEffect, useState } from 'react'

const cache = new Map<string, string | null>()

/**
 * Port of `DisplayIconifyRemoteIcon` — renders an Iconify icon by name
 * (`set:name` or `set-name`) fetched from the Iconify API at runtime; no
 * bundled icon set. Cached per name. Note: this is the one component in the
 * port that needs network; it renders nothing until the SVG arrives.
 */
export function DisplayIconifyRemoteIcon({
  icon,
  size = 16,
  color,
}: {
  /** Iconify name — `ph:magnifying-glass` or `ph-magnifying-glass`. */
  icon: string
  size?: number
  color?: string
}) {
  const [svg, setSvg] = useState<string | null>(() => cache.get(icon) ?? null)

  useEffect(() => {
    if (cache.has(icon)) {
      setSvg(cache.get(icon) ?? null)
      return
    }
    const [set, name] = icon.includes(':') ? icon.split(':', 2) : icon.split(/-(.+)/, 2)
    if (!set || !name) return
    let live = true
    fetch(`https://api.iconify.design/${set}/${name}.svg`)
      .then(r => (r.ok ? r.text() : null))
      .then(text => {
        cache.set(icon, text)
        if (live) setSvg(text)
      })
      .catch(() => {
        cache.set(icon, null)
      })
    return () => {
      live = false
    }
  }, [icon])

  if (!svg) return <span {...stylex.props(styles.box)} style={{ width: size, height: size }} />
  return (
    <span
      {...stylex.props(styles.icon)}
      style={{ width: size, height: size, color }}
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  )
}

const styles = stylex.create({
  box: {
    display: 'inline-block',
    flexShrink: 0,
  },
  icon: {
    display: 'inline-flex',
    flexShrink: 0,
    lineHeight: 0,
  },
})
