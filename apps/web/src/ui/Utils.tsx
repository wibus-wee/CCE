import * as stylex from '@stylexjs/stylex'
import { useCallback, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { vars } from './tokens.stylex'

/**
 * Port of `ScrollFade` — fades overflowing content edges (top/bottom by
 * default) using mask-image; pass `direction="x"` for horizontal lists.
 */
export function ScrollFade({
  direction = 'y',
  size = 24,
  children,
}: {
  direction?: 'x' | 'y'
  size?: number
  children: ReactNode
}) {
  const style: CSSProperties =
    direction === 'y'
      ? {
          maskImage: `linear-gradient(to bottom, transparent 0, black ${size}px, black calc(100% - ${size}px), transparent 100%)`,
          WebkitMaskImage: `linear-gradient(to bottom, transparent 0, black ${size}px, black calc(100% - ${size}px), transparent 100%)`,
        }
      : {
          maskImage: `linear-gradient(to right, transparent 0, black ${size}px, black calc(100% - ${size}px), transparent 100%)`,
          WebkitMaskImage: `linear-gradient(to right, transparent 0, black ${size}px, black calc(100% - ${size}px), transparent 100%)`,
        }
  return (
    <div {...stylex.props(styles.fade)} style={style}>
      {children}
    </div>
  )
}

/**
 * Port of `Shimmer` — an animated loading stripe overlay for `rounded`
 * containers while `loading` is true.
 */
export function Shimmer({
  loading = false,
  rounded = 8,
  children,
}: {
  loading?: boolean
  rounded?: number
  children?: ReactNode
}) {
  return (
    <div {...stylex.props(styles.shimmerWrap)} style={{ borderRadius: rounded }}>
      {children}
      {loading && <div {...stylex.props(styles.shimmer)} style={{ borderRadius: rounded }} />}
    </div>
  )
}

/**
 * `useInputFocus` — port of the composable: exposes `focused` state plus
 * `onFocus`/`onBlur` handlers and an imperative `focus()` for a bound ref.
 */
export function useInputFocus<T extends HTMLElement = HTMLInputElement>() {
  const ref = useRef<T>(null)
  const [focused, setFocused] = useState(false)
  const focus = useCallback(() => ref.current?.focus(), [])
  const onFocus = useCallback(() => setFocused(true), [])
  const onBlur = useCallback(() => setFocused(false), [])
  return { ref, focused, focus, onFocus, onBlur }
}

const shimmerMove = stylex.keyframes({
  '0%': { transform: 'translateX(-100%)' },
  '100%': { transform: 'translateX(100%)' },
})

const styles = stylex.create({
  fade: {
    minWidth: 0,
    minHeight: 0,
    display: 'flex',
    flexDirection: 'column',
  },
  shimmerWrap: {
    position: 'relative',
    overflow: 'hidden',
  },
  shimmer: {
    position: 'absolute',
    inset: 0,
    overflow: 'hidden',
    backgroundImage: `linear-gradient(90deg, transparent 0%, ${vars.bgAmbient} 50%, transparent 100%)`,
    animationName: shimmerMove,
    animationDuration: '1.4s',
    animationTimingFunction: 'ease-in-out',
    animationIterationCount: 'infinite',
    pointerEvents: 'none',
  },
})
