import { useEffect, useRef, useState } from 'react'

/**
 * ResizeObserver-based element measurement — for hand-rolled SVG charts that
 * must compute geometry in real pixels (viewBox scaling distorts text).
 */
export function useElementSize<T extends HTMLElement = HTMLDivElement>() {
  const ref = useRef<T>(null)
  const [size, setSize] = useState({ width: 0, height: 0 })

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect
      if (rect) setSize({ width: rect.width, height: rect.height })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  return { ref, ...size }
}
