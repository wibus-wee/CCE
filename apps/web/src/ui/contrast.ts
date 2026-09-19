/**
 * Port of `@antfu/design/utils/contrast` — WCAG relative luminance and
 * contrast-ratio checks (the `a11y` scan's math, runtime-usable).
 */

export interface RGB {
  r: number
  g: number
  b: number
}

function channel(hex: string, start: number): number {
  return parseInt(hex.slice(start, start + 2), 16) / 255
}

export function parseColor(input: string | RGB): RGB {
  if (typeof input !== 'string') return input
  let hex = input.trim().replace(/^#/, '')
  if (hex.length === 3)
    hex = hex.split('').map(c => c + c).join('')
  if (!/^[0-9a-f]{6}$/i.test(hex))
    throw new Error(`parseColor: unsupported color "${input}"`)
  return { r: channel(hex, 0) * 255, g: channel(hex, 2) * 255, b: channel(hex, 4) * 255 }
}

function linearize(v: number): number {
  const c = v / 255
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
}

export function relativeLuminance(color: string | RGB): number {
  const { r, g, b } = parseColor(color)
  return 0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b)
}

export function contrastRatio(a: string | RGB, b: string | RGB): number {
  const la = relativeLuminance(a)
  const lb = relativeLuminance(b)
  const [light, dark] = la > lb ? [la, lb] : [lb, la]
  return (light + 0.05) / (dark + 0.05)
}

export type ContrastLevel = 'AA' | 'AAA'

export function meetsContrast(ratio: number, level: ContrastLevel = 'AA', large = false): boolean {
  const threshold = level === 'AAA' ? (large ? 4.5 : 7) : (large ? 3 : 4.5)
  return ratio >= threshold
}

export interface ContrastResult {
  ratio: number
  aa: boolean
  aaLarge: boolean
  aaa: boolean
  aaaLarge: boolean
}

export function checkContrast(foreground: string | RGB, background: string | RGB): ContrastResult {
  const ratio = contrastRatio(foreground, background)
  return {
    ratio,
    aa: meetsContrast(ratio, 'AA'),
    aaLarge: meetsContrast(ratio, 'AA', true),
    aaa: meetsContrast(ratio, 'AAA'),
    aaaLarge: meetsContrast(ratio, 'AAA', true),
  }
}
