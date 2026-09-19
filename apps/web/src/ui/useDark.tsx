import * as stylex from '@stylexjs/stylex'
import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react'
import { darkTheme } from './tokens.stylex'

const STORAGE_KEY = 'cce-color-scheme'
// defineVars defaults are the light values; the dark theme class overrides
// them on `<html>` for the whole tree. `props()` returns several class tokens,
// so each is toggled individually (DOMTokenList rejects space-joined tokens).
const darkClasses = (stylex.props(darkTheme).className ?? '')
  .split(' ')
  .filter(Boolean)

export type ColorScheme = 'light' | 'dark'

/**
 * Dark mode is the app's to own (per the design contract): preference in
 * localStorage, system default otherwise, applied as the `darkTheme` classes
 * on `<html>` plus `color-scheme` for native chrome.
 */
export function useDark() {
  const [dark, setDark] = useState<boolean>(() => {
    const stored = localStorage.getItem(STORAGE_KEY)
    if (stored === 'dark') return true
    if (stored === 'light') return false
    return matchMedia('(prefers-color-scheme: dark)').matches
  })

  useEffect(() => {
    for (const token of darkClasses) {
      document.documentElement.classList.toggle(token, dark)
    }
    document.documentElement.style.colorScheme = dark ? 'dark' : 'light'
    localStorage.setItem(STORAGE_KEY, dark ? 'dark' : 'light')
  }, [dark])

  const toggle = useCallback(() => setDark((value) => !value), [])
  return { dark, toggle }
}

// `provideColorScheme`/`useColorScheme` — components that tune hash colors for
// contrast (`DisplayBadge`, `DisplayLabel`, `DisplayPackageName`,
// `DisplayProportionBar`, `DisplayAvatar`) read this context; an explicit
// `colorScheme` prop always wins.
const ColorSchemeContext = createContext<ColorScheme>('light')

export function ColorSchemeProvider({
  value,
  children,
}: {
  value: ColorScheme
  children: ReactNode
}) {
  return <ColorSchemeContext.Provider value={value}>{children}</ColorSchemeContext.Provider>
}

export function useColorScheme(override?: ColorScheme): ColorScheme {
  const context = useContext(ColorSchemeContext)
  return override ?? context
}
