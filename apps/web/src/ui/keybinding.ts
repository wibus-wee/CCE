/**
 * Port of `@antfu/design/utils/keybinding` — chord parsing and platform glyph
 * display. `mod` resolves to `meta` on Apple platforms, `ctrl` elsewhere.
 */

export const isMac: boolean =
  typeof navigator !== 'undefined' &&
  /mac|iphone|ipad|ipod/i.test(navigator.platform || navigator.userAgent)

export interface ParsedChord {
  /** Modifiers, alphabetically ordered: `alt` | `ctrl` | `meta` | `shift`. */
  modifiers: string[]
  /** The final non-modifier key, e.g. `k`, `Enter`, `ArrowUp`, `/`. */
  key: string
}

const MOD_ALIASES: Record<string, string> = {
  mod: isMac ? 'meta' : 'ctrl',
  cmd: 'meta',
  command: 'meta',
  super: 'meta',
  win: 'meta',
  meta: 'meta',
  ctrl: 'ctrl',
  control: 'ctrl',
  alt: 'alt',
  option: 'alt',
  opt: 'alt',
  shift: 'shift',
}

const KEY_ALIASES: Record<string, string> = {
  esc: 'Escape',
  escape: 'Escape',
  enter: 'Enter',
  return: 'Enter',
  space: ' ',
  tab: 'Tab',
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
}

const MOD_ORDER = ['ctrl', 'alt', 'shift', 'meta']

export function parseChord(input: string): ParsedChord {
  const tokens = input.trim().split('+').map(t => t.trim()).filter(Boolean)
  const modifiers = new Set<string>()
  let key = ''
  for (const token of tokens) {
    const lower = token.toLowerCase()
    const mod = MOD_ALIASES[lower]
    if (mod != null) modifiers.add(mod)
    else key = KEY_ALIASES[lower] ?? (token.length === 1 ? token.toLowerCase() : token)
  }
  return { modifiers: MOD_ORDER.filter(m => modifiers.has(m)), key }
}

export function parseBinding(binding: string): ParsedChord[] {
  return binding.trim().split(/\s+/).filter(Boolean).map(parseChord)
}

export function chordToken(chord: ParsedChord): string {
  return [...chord.modifiers, chord.key.toLowerCase()].join('+')
}

export function eventToToken(event: KeyboardEvent | React.KeyboardEvent): string {
  const modifiers: string[] = []
  if (event.ctrlKey) modifiers.push('ctrl')
  if (event.altKey) modifiers.push('alt')
  if (event.shiftKey) modifiers.push('shift')
  if (event.metaKey) modifiers.push('meta')
  const key = KEY_ALIASES[event.key.toLowerCase()] ?? event.key
  return [...MOD_ORDER.filter(m => modifiers.includes(m)), key.toLowerCase()].join('+')
}

const GLYPHS_MAC: Record<string, string> = { meta: '⌘', ctrl: '⌃', alt: '⌥', shift: '⇧' }
const LABELS: Record<string, string> = { meta: 'Win', ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift' }
const KEY_GLYPHS: Record<string, string> = {
  Enter: '↵',
  Escape: 'Esc',
  ArrowUp: '↑',
  ArrowDown: '↓',
  ArrowLeft: '←',
  ArrowRight: '→',
  ' ': 'Space',
}

export function chordDisplay(chord: ParsedChord): string[] {
  const mods = chord.modifiers.map(m => (isMac ? GLYPHS_MAC[m] : LABELS[m]) ?? m)
  const key = KEY_GLYPHS[chord.key] ?? (chord.key.length === 1 ? chord.key.toUpperCase() : chord.key)
  return [...mods, key]
}

export function bindingDisplay(binding: string): string[] {
  return parseBinding(binding).flatMap(chordDisplay)
}
