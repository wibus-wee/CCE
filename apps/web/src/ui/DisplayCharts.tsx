import * as stylex from '@stylexjs/stylex'
import { getHashColorFromString } from './format'
import { vars } from './tokens.stylex'
import { useColorScheme, type ColorScheme } from './useDark'

/**
 * `DisplayDonut` — a progress ring (0–1 `value`, indeterminate spin when
 * omitted), token-colored.
 */
export function DisplayDonut({
  value,
  size = 16,
  stroke = 2,
  color,
}: {
  /** 0–1; omit for the indeterminate spinner. */
  value?: number
  size?: number
  stroke?: number
  color?: string
}) {
  const r = (size - stroke) / 2
  const c = 2 * Math.PI * r
  const frac = Math.max(0, Math.min(1, value ?? 0))
  const indeterminate = value == null
  return (
    <svg
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      role="progressbar"
      aria-valuenow={indeterminate ? undefined : Math.round(frac * 100)}
      {...stylex.props(styles.svg, indeterminate && styles.spin)}
    >
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        strokeWidth={stroke}
        {...stylex.props(styles.track)}
      />
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        strokeWidth={stroke}
        strokeLinecap="round"
        strokeDasharray={c}
        strokeDashoffset={indeterminate ? c * 0.7 : c * (1 - frac)}
        transform={`rotate(-90 ${size / 2} ${size / 2})`}
        style={color ? { stroke: color } : undefined}
        {...stylex.props(styles.arc)}
      />
    </svg>
  )
}

export interface ProportionSegment {
  value: number
  label?: string
  color?: string
}

/**
 * Port of `DisplayProportionBar` — a stacked proportion bar; segments take an
 * explicit `color` or hash-color from `label`/`index`.
 */
export function DisplayProportionBar({
  segments,
  height = 8,
  colorScheme,
  showLegend = false,
}: {
  segments: ProportionSegment[]
  height?: number
  colorScheme?: ColorScheme
  /** Render `label` + share under the bar. */
  showLegend?: boolean
}) {
  const scheme = useColorScheme(colorScheme)
  const dark = scheme === 'dark'
  const total = segments.reduce((sum, s) => sum + s.value, 0) || 1
  const segColor = (seg: ProportionSegment, i: number) =>
    seg.color ?? getHashColorFromString(seg.label ?? String(i), 1, dark)

  return (
    <div {...stylex.props(styles.wrap)}>
      <div {...stylex.props(styles.bar)} style={{ height }}>
        {segments.map((seg, i) => (
          <div
            key={i}
            title={seg.label}
            style={{
              width: `${(seg.value / total) * 100}%`,
              background: segColor(seg, i),
              height: '100%',
            }}
          />
        ))}
      </div>
      {showLegend && (
        <div {...stylex.props(styles.legend)}>
          {segments.map((seg, i) => (
            <span key={i} {...stylex.props(styles.legendItem)}>
              <span
                {...stylex.props(styles.swatch)}
                style={{ background: segColor(seg, i) }}
              />
              {seg.label}
              <span {...stylex.props(styles.legendValue)}>
                {((seg.value / total) * 100).toFixed(0)}%
              </span>
            </span>
          ))}
        </div>
      )}
    </div>
  )
}

const styles = stylex.create({
  svg: {
    display: 'block',
    flexShrink: 0,
  },
  spin: {
    animationName: stylex.keyframes({ to: { transform: 'rotate(360deg)' } }),
    animationDuration: '0.9s',
    animationTimingFunction: 'linear',
    animationIterationCount: 'infinite',
    transformOrigin: 'center',
  },
  track: {
    stroke: vars.bgActive,
  },
  arc: {
    stroke: vars.colorActive,
    transitionProperty: 'stroke-dashoffset',
    transitionDuration: '200ms',
  },
  wrap: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
    width: '100%',
  },
  bar: {
    borderRadius: 999,
    backgroundColor: vars.bgActive,
    display: 'flex',
    width: '100%',
    overflow: 'hidden',
  },
  legend: {
    display: 'flex',
    flexWrap: 'wrap',
    gap: 8,
    fontSize: 11,
    color: vars.colorMuted,
  },
  legendItem: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
  },
  swatch: {
    width: 8,
    height: 8,
    borderRadius: 2,
    flexShrink: 0,
  },
  legendValue: {
    opacity: 0.6,
  },
})
