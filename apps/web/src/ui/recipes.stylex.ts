import * as stylex from '@stylexjs/stylex'
import { font, vars } from './tokens.stylex'

/**
 * The composite shortcuts from the preset, ported verbatim:
 * `btn-action` / `btn-primary` / `btn-text` are peers at one size — same
 * padding (`px2 py1`), same 1px border box, same gap and focus ring — so a
 * mixed row aligns. `text-sm` drops to the compact size.
 */

const focusRing = {
  outline: 'none',
  boxShadow: {
    default: 'none',
    ':focus-visible': `0 0 0 2px ${vars.ringPrimary}`,
  },
} as const

const btnBase = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 8,
  paddingTop: 5,
  paddingBottom: 5,
  paddingLeft: 8,
  paddingRight: 8,
  borderRadius: 4,
  borderWidth: 1,
  borderStyle: 'solid',
  cursor: 'pointer',
  fontSize: 13,
  lineHeight: '1.4',
  fontFamily: 'inherit',
  textDecoration: 'none',
  whiteSpace: 'nowrap',
  opacity: {
    default: vars.opFade,
    ':hover': 1,
    ':disabled': vars.opMute,
  },
  transitionProperty: 'opacity, background-color, color, border-color, box-shadow',
  transitionDuration: '120ms',
  pointerEvents: { ':disabled': 'none' },
} as const

export const buttons = stylex.create({
  action: {
    ...btnBase,
    color: 'inherit',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    borderColor: vars.borderBase,
    ...focusRing,
  },
  primary: {
    ...btnBase,
    color: vars.onPrimary,
    backgroundColor: {
      default: vars.primary500,
      ':hover': vars.primary600,
    },
    borderColor: 'transparent',
    opacity: {
      default: 1,
      ':hover': 1,
      ':disabled': 0.5,
    },
    ...focusRing,
  },
  text: {
    ...btnBase,
    color: 'inherit',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    borderColor: 'transparent',
    ...focusRing,
  },
})

export const iconButtons = stylex.create({
  round: {
    width: 32,
    height: 32,
    padding: 0,
    borderRadius: '50%',
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: 'transparent',
    color: 'inherit',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    cursor: 'pointer',
    opacity: {
      default: vars.opFade,
      ':hover': 1,
      ':disabled': vars.opMute,
    },
    transitionProperty: 'opacity, background-color, color, box-shadow',
    transitionDuration: '120ms',
    pointerEvents: { ':disabled': 'none' },
    ...focusRing,
  },
  square: {
    width: 32,
    height: 32,
    padding: 0,
    borderRadius: 4,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    flexShrink: 0,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    color: 'inherit',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    cursor: 'pointer',
    opacity: {
      default: vars.opFade,
      ':hover': 1,
      ':disabled': vars.opMute,
    },
    transitionProperty: 'opacity, background-color, color, box-shadow',
    transitionDuration: '120ms',
    pointerEvents: { ':disabled': 'none' },
    ...focusRing,
  },
  active: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
    opacity: 1,
  },
  // 20px borderless ghost — the hover-revealed trailing action inside
  // rows (rerun/remove/set-default). Same bgHover tint as every other
  // icon button; visibility is the parent row's job, not this button's.
  mini: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 20,
    height: 20,
    padding: 0,
    borderWidth: 0,
    borderRadius: 4,
    color: vars.colorFaint,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    cursor: 'pointer',
    opacity: {
      default: 1,
      ':disabled': vars.opMute,
    },
    pointerEvents: { ':disabled': 'none' },
    ...focusRing,
  },
})

export const badges = stylex.create({
  base: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 6,
    borderRadius: 6,
    fontSize: 12,
    fontWeight: 500,
    lineHeight: 1,
    whiteSpace: 'nowrap',
  },
  active: {
    backgroundColor: vars.bgActive,
    color: vars.colorActive,
  },
  muted: {
    backgroundColor: 'rgba(136, 136, 136, 0.07)', // #8881
    color: vars.colorMuted,
  },
})

export const text = stylex.create({
  micro: { fontSize: 10, lineHeight: 1.4 },
  mini: { fontSize: 11, lineHeight: 1.45 },
  compact: { fontSize: 12, lineHeight: 1.5 },
  mono: { fontFamily: font.mono, fontVariantNumeric: 'tabular-nums' },
  muted: { color: vars.colorMuted },
  faint: { color: vars.colorFaint },
  fade: { opacity: vars.opFade },
})

// Field chrome shared by the Form* controls: sunken fill, base border, the
// primary ring on focus. A stylex-created map (not a plain object) so it
// composes via `stylex.props(field.base, styles.x)` — imported object
// spreads inside `stylex.create` are not resolved by the compiler.
export const field = stylex.create({
  base: {
    width: '100%',
    boxSizing: 'border-box',
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: {
      default: vars.borderBase,
      ':focus-visible': vars.borderActive,
    },
    backgroundColor: vars.bgRaised,
    color: vars.colorBase,
    fontFamily: 'inherit',
    fontSize: 13,
    lineHeight: '1.45',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    ...focusRing,
  },
})

export const severityColor = stylex.create({
  neutral: { color: vars.scaleNeutral },
  low: { color: vars.scaleLow },
  medium: { color: vars.scaleMedium },
  high: { color: vars.scaleHigh },
  critical: { color: vars.scaleCritical },
})

// Floating-surface chrome shared by Select/Combobox/Menu/Popover popups and
// their items — the `bg-glass` + hairline + shadow treatment from the preset.
export const popup = stylex.create({
  surface: {
    zIndex: 50,
    minWidth: 140,
    padding: 4,
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgTooltip,
    backdropFilter: 'blur(12px)',
    boxShadow: vars.shadowOverlay,
    outline: 'none',
  },
  item: {
    fontSize: 13,
    lineHeight: '1.4',
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 6,
    color: vars.colorBase,
    cursor: 'pointer',
    userSelect: 'none',
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    outline: 'none',
    // Base UI marks the active row with `data-highlighted` (hover and
    // keyboard alike) — fold it into the base item so menu/context/sub
    // items highlight without each call site wiring render-prop state.
    // `itemHighlighted` still exists for select/combobox's explicit state.
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
      '[data-highlighted]': vars.bgHover,
    },
    opacity: {
      default: 1,
      '[data-disabled]': vars.opMute,
    },
  },
  itemHighlighted: {
    backgroundColor: vars.bgHover,
  },
  itemSelected: {
    color: vars.colorActive,
  },
  itemDisabled: {
    opacity: vars.opMute,
    pointerEvents: 'none',
  },
  label: {
    fontSize: 10,
    fontWeight: 600,
    color: vars.colorFaint,
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 8,
    paddingRight: 8,
    textTransform: 'uppercase',
    letterSpacing: '0.04em',
  },
  separator: {
    height: 1,
    backgroundColor: vars.borderBase,
    marginTop: 4,
    marginBottom: 4,
    marginLeft: -4,
    marginRight: -4,
  },
})

// Subtle tinted wash behind severity text (badges, pills).
export const severityBg = stylex.create({
  neutral: { backgroundColor: 'rgba(107, 114, 128, 0.10)' },
  low: { backgroundColor: 'rgba(101, 163, 13, 0.12)' },
  medium: { backgroundColor: 'rgba(217, 119, 6, 0.12)' },
  high: { backgroundColor: 'rgba(234, 88, 12, 0.12)' },
  critical: { backgroundColor: 'rgba(220, 38, 38, 0.10)' },
})
