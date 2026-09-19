import * as stylex from '@stylexjs/stylex'
import { useEffect, useState, type ReactNode } from 'react'

/**
 * Port of `DisplaySafeImage` — an `<img>` that swaps to `fallback` on a
 * missing/failed `src`, with an optional `preload` to avoid a flash.
 */
export function DisplaySafeImage({
  src,
  alt = '',
  fallback,
  preload = false,
  ...rest
}: {
  src?: string | null
  alt?: string
  /** Rendered when `src` is absent or fails — the `#fallback` slot. */
  fallback?: ReactNode
  /** Preload the image before showing it, avoiding a partial render. */
  preload?: boolean
} & Omit<React.ImgHTMLAttributes<HTMLImageElement>, 'src' | 'alt'>) {
  const [failed, setFailed] = useState(false)
  const [ready, setReady] = useState(!preload)

  useEffect(() => {
    setFailed(false)
    setReady(!preload)
    if (!src || !preload) return
    const img = new Image()
    img.onload = () => setReady(true)
    img.onerror = () => setFailed(true)
    img.src = src
    return () => {
      img.onload = null
      img.onerror = null
    }
  }, [src, preload])

  if (!src || failed) return <>{fallback ?? null}</>
  if (!ready) return null
  return (
    <img
      src={src}
      alt={alt}
      onError={() => setFailed(true)}
      {...stylex.props(styles.img)}
      {...rest}
    />
  )
}

const styles = stylex.create({
  img: {
    display: 'block',
    maxWidth: '100%',
  },
})
