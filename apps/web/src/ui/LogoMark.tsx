import type { CSSProperties } from 'react'

/**
 * The CCE product mark — two nested "C" strata (Codebase · Context: the
 * independently materialized views) around the canonical source core, with
 * the provenance beam exiting the aligned gap to an external node (the
 * delivered context pack). Drawn on a 32 grid in `currentColor`, matching
 * the app's stroke-icon vocabulary; `public/favicon.svg` is the same glyph
 * on the primary tile.
 */
export function LogoMark({ size = 16 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      style={{ display: 'block', flexShrink: 0 } as CSSProperties}
    >
      <path d="M24.31 8.61A11.5 11.5 0 1 0 24.31 23.39" strokeWidth={2.8} />
      <path d="M21.25 11.18A7.5 7.5 0 1 0 21.25 20.82" strokeWidth={2.8} />
      <path d="M18.4 16H24.9" strokeWidth={2.4} />
      <circle cx="15.5" cy="16" r="3" fill="currentColor" stroke="none" />
      <circle cx="27.1" cy="16" r="2.5" fill="currentColor" stroke="none" />
    </svg>
  )
}
