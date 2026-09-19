import * as stylex from '@stylexjs/stylex'
import { getHashColorFromString } from './format'
import { font } from './tokens.stylex'
import { useColorScheme, type ColorScheme } from './useDark'

/**
 * Port of `DisplayPackageName` — `@scope/name` with the scope hash-colored;
 * unscoped names render whole in the hash color.
 */
export function DisplayPackageName({
  name,
  colorScheme,
}: {
  name: string
  colorScheme?: ColorScheme
}) {
  const scheme = useColorScheme(colorScheme)
  const dark = scheme === 'dark'
  const scoped = name.startsWith('@') && name.includes('/')
  const scope = scoped ? name.slice(0, name.indexOf('/')) : undefined
  const rest = scoped ? name.slice(name.indexOf('/') + 1) : name

  return (
    <span {...stylex.props(styles.name)}>
      {scope != null && (
        <>
          <span style={{ color: getHashColorFromString(scope, 1, dark) }}>{scope}</span>
          <span {...stylex.props(styles.slash)}>/</span>
        </>
      )}
      <span style={scope == null ? { color: getHashColorFromString(rest, 1, dark) } : undefined}>
        {rest}
      </span>
    </span>
  )
}

const styles = stylex.create({
  name: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    whiteSpace: 'nowrap',
  },
  slash: {
    opacity: 0.5,
  },
})
