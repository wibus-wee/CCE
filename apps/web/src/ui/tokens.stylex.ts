import * as stylex from '@stylexjs/stylex'

/**
 * StyleX port of the `@antfu/design` semantic token layer.
 *
 * The names mirror the UnoCSS shortcuts 1:1 (`bg-base` -> `bgBase`,
 * `color-muted` -> `colorMuted`, `color-scale-*` -> `scale*`) so the skill's
 * vocabulary applies verbatim. Light values live in `vars`; `darkTheme`
 * overrides the same keys, applied as a class on `<html>` — the app owns dark
 * mode, same contract as the package.
 */
export const vars = stylex.defineVars({
  // ── Text ──────────────────────────────────────────────────────────────
  colorBase: '#262626', // neutral-800
  colorMuted: '#525252', // neutral-600
  colorFaint: '#737373', // neutral-500
  colorActive: '#49833e', // primary-600

  // ── Surfaces ──────────────────────────────────────────────────────────
  bgBase: '#ffffff',
  bgSecondary: '#f6f6f6',
  // App chrome (topbar + sidebar) — sits one step below bgSecondary so the
  // floating content panel on bgBase reads as a distinct surface.
  bgShell: '#f4f3f0',
  bgRaised: 'rgba(255, 255, 255, 0.65)', // white/65
  bgSunken: 'rgba(0, 0, 0, 0.04)', // black/4
  bgActive: 'rgba(153, 153, 153, 0.19)', // #99999930
  bgAmbient: 'rgba(153, 153, 153, 0.15)', // #99999925
  bgHover: 'rgba(153, 153, 153, 0.13)', // #99999920
  bgCode: 'rgba(107, 114, 128, 0.05)', // gray-500/5
  bgTooltip: 'rgba(255, 255, 255, 0.75)', // + backdrop-blur at use site
  // Opaque surface for dialogs/drawers — `bgRaised` is a translucent tint
  // that lets underlying content bleed through without backdrop-blur.
  bgOverlay: '#ffffff',

  // ── Borders / rings ───────────────────────────────────────────────────
  borderBase: 'rgba(153, 153, 153, 0.13)', // #9992
  borderMute: 'rgba(153, 153, 153, 0.07)', // #9991
  borderActive: 'rgba(73, 131, 62, 0.25)', // primary-600/25
  borderCritical: 'rgba(185, 28, 28, 0.45)', // red-700/45 — invalid fields
  ringPrimary: 'rgba(91, 149, 68, 0.4)', // primary-500/40

  // ── Primary ramp (antfu green) ────────────────────────────────────────
  primary300: '#a8cf96',
  primary400: '#7eb267',
  primary500: '#5b9544',
  primary600: '#49833e',
  onPrimary: '#ffffff',

  // ── Severity ramp (gray -> lime -> amber -> orange -> red) ────────────
  scaleNeutral: '#374151', // gray-700
  scaleLow: '#4d7c0f', // lime-700
  scaleMedium: '#b45309', // amber-700
  scaleHigh: '#c2410c', // orange-700
  scaleCritical: '#b91c1c', // red-700

  // ── Semantic accents (tips, links) ────────────────────────────────────
  accentInfo: '#2563eb',
  accentSuccess: '#039855',
  accentWarning: '#b45309',
  accentError: '#b91c1c',
  tipInfoBg: 'rgba(37, 99, 235, 0.07)',
  tipSuccessBg: 'rgba(3, 152, 85, 0.07)',
  tipWarningBg: 'rgba(180, 83, 9, 0.08)',
  tipErrorBg: 'rgba(185, 28, 28, 0.07)',

  // ── Search match highlight (<mark> in snippets) ───────────────────────
  markBg: 'rgba(250, 204, 21, 0.4)', // yellow-400/40

  // ── Opacity shortcuts (op-fade / op-mute) ─────────────────────────────
  opFade: '0.65',
  opMute: '0.3',

  // ── Shadow for floating surfaces ──────────────────────────────────────
  shadowOverlay: '0 4px 24px rgba(0, 0, 0, 0.08), 0 1px 4px rgba(0, 0, 0, 0.06)',
})

export const darkTheme = stylex.createTheme(vars, {
  colorBase: '#e5e5e5', // neutral-200
  colorMuted: '#a3a3a3', // neutral-400
  colorFaint: '#737373', // neutral-500
  colorActive: '#a8cf96', // primary-300

  bgBase: '#161616',
  bgSecondary: '#101010',
  bgShell: '#0f0e0d',
  bgRaised: 'rgba(255, 255, 255, 0.06)', // white/6
  bgSunken: 'rgba(0, 0, 0, 0.2)', // black/20
  bgActive: 'rgba(153, 153, 153, 0.19)',
  bgAmbient: 'rgba(153, 153, 153, 0.15)',
  bgHover: 'rgba(153, 153, 153, 0.13)',
  bgCode: 'rgba(107, 114, 128, 0.05)',
  bgTooltip: 'rgba(22, 22, 22, 0.75)',
  bgOverlay: '#242424', // bgBase + white/6 composited — opaque, same tone

  borderBase: 'rgba(153, 153, 153, 0.13)',
  borderMute: 'rgba(153, 153, 153, 0.09)',
  borderActive: 'rgba(126, 178, 103, 0.25)', // primary-400/25
  borderCritical: 'rgba(252, 165, 165, 0.45)', // red-300/45
  ringPrimary: 'rgba(91, 149, 68, 0.4)',

  primary300: '#a8cf96',
  primary400: '#7eb267',
  primary500: '#5b9544',
  primary600: '#49833e',
  onPrimary: '#ffffff',

  // dark stops + the preset's saturate-75/90 nudges baked in
  scaleNeutral: '#d1d5db', // gray-300
  scaleLow: '#aad06a', // lime-300, desaturated ~75%
  scaleMedium: '#f7cd54', // amber-300, desaturated ~90%
  scaleHigh: '#fdba74', // orange-300
  scaleCritical: '#fca5a5', // red-300

  accentInfo: '#93c5fd',
  accentSuccess: '#32d583',
  accentWarning: '#fcd34d',
  accentError: '#fca5a5',
  tipInfoBg: 'rgba(147, 197, 253, 0.09)',
  tipSuccessBg: 'rgba(50, 213, 131, 0.09)',
  tipWarningBg: 'rgba(252, 211, 77, 0.09)',
  tipErrorBg: 'rgba(252, 165, 165, 0.09)',

  markBg: 'rgba(250, 204, 21, 0.22)',

  opFade: '0.55',
  opMute: '0.25',

  shadowOverlay: '0 4px 24px rgba(0, 0, 0, 0.45), 0 1px 4px rgba(0, 0, 0, 0.35)',
})

// Font stacks — platform faces keep the app fully offline; metrics chosen to
// sit close to the DM Sans / DM Mono pairing the source apps use.
// `defineConsts` (not plain exports) so the strings are usable inside
// stylex.create across files.
export const font = stylex.defineConsts({
  sans: 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Inter, Roboto, "Helvetica Neue", Arial, sans-serif',
  mono: 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace',
})

// Severity names shared by badges, durations and state pills.
export type Severity = 'neutral' | 'low' | 'medium' | 'high' | 'critical'
