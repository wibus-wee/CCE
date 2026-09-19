import * as stylex from '@stylexjs/stylex'
import { Avatar } from '@base-ui-components/react'
import { getHashColorFromString } from './format'
import { useColorScheme, type ColorScheme } from './useDark'

/**
 * Port of `DisplayAvatar` on Base UI `Avatar` — image with hash-colored
 * initials fallback from `name` when `src` is absent or fails to load.
 */
export function DisplayAvatar({
  src,
  name = '',
  size = 32,
  square = false,
  colorScheme,
}: {
  /** Image source. Falls back to initials when absent or failed. */
  src?: string
  /** Name driving the initials and the hash-tinted fallback color. */
  name?: string
  /** Size in pixels (width and height). */
  size?: number
  /** Rounded-corner square instead of a circle. */
  square?: boolean
  /** The app's current color scheme — tunes the fallback for contrast. */
  colorScheme?: ColorScheme
}) {
  const scheme = useColorScheme(colorScheme)
  const dark = scheme === 'dark'
  const initials = name
    .split(/[\s/-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map(part => part[0]!.toUpperCase())
    .join('')

  return (
    <Avatar.Root
      {...stylex.props(styles.root, square ? styles.square : styles.round)}
      style={{ width: size, height: size, fontSize: size * 0.4 }}
    >
      {src != null && <Avatar.Image src={src} alt={name} {...stylex.props(styles.image)} />}
      <Avatar.Fallback
        style={{
          background: getHashColorFromString(name, 0.18, dark),
          color: getHashColorFromString(name, 1, dark),
        }}
        {...stylex.props(styles.fallback)}
      >
        {initials || '?'}
      </Avatar.Fallback>
    </Avatar.Root>
  )
}

const styles = stylex.create({
  root: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    overflow: 'hidden',
    flexShrink: 0,
    verticalAlign: 'middle',
    userSelect: 'none',
  },
  round: { borderRadius: '50%' },
  square: { borderRadius: 6 },
  image: {
    width: '100%',
    height: '100%',
    objectFit: 'cover',
  },
  fallback: {
    width: '100%',
    height: '100%',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    fontWeight: 600,
    lineHeight: 1,
  },
})
