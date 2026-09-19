import * as stylex from '@stylexjs/stylex'
import { Progress } from '@base-ui-components/react'
import { vars } from './tokens.stylex'

/**
 * Port of `DisplayProgressBar` on Base UI `Progress` — linear track, `value`
 * 0–1 determinate or the indeterminate slide when omitted. `color` overrides
 * the primary fill; `label`/`showValue` render the accessible text parts.
 */
export function DisplayProgressBar({
  value,
  color,
  rounded = true,
  label,
  showValue = false,
  format,
}: {
  /** 0–1; omit for indeterminate. */
  value?: number
  color?: string
  rounded?: boolean
  label?: string
  /** Render the formatted value beside the track. */
  showValue?: boolean
  format?: Intl.NumberFormatOptions
}) {
  const pct = value == null ? null : Math.min(1, Math.max(0, value))
  return (
    <Progress.Root
      value={pct == null ? 0 : pct * 100}
      format={format ?? { style: 'percent' }}
      {...stylex.props(styles.root)}
    >
      {(label != null || showValue) && (
        <div {...stylex.props(styles.meta)}>
          {label != null && <Progress.Label {...stylex.props(styles.label)}>{label}</Progress.Label>}
          {showValue && <Progress.Value {...stylex.props(styles.value)} />}
        </div>
      )}
      <Progress.Track
        {...stylex.props(styles.track, rounded && styles.rounded)}
        aria-label={pct == null ? (label ?? 'Loading') : undefined}
      >
        <Progress.Indicator
          {...stylex.props(
            styles.fill,
            rounded && styles.rounded,
            pct == null && styles.indeterminate,
          )}
          style={{
            transform: pct == null ? undefined : `translateX(-${(1 - pct) * 100}%)`,
            backgroundColor: color,
          }}
        />
      </Progress.Track>
    </Progress.Root>
  )
}

const slide = stylex.keyframes({
  '0%': { transform: 'translateX(-100%)' },
  '100%': { transform: 'translateX(400%)' },
})

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    width: '100%',
  },
  meta: {
    display: 'flex',
    justifyContent: 'space-between',
    gap: 8,
    fontSize: 11,
    color: vars.colorMuted,
  },
  label: {
    minWidth: 0,
  },
  value: {
    fontVariantNumeric: 'tabular-nums',
  },
  track: {
    width: '100%',
    height: 4,
    backgroundColor: vars.bgSunken,
    overflow: 'hidden',
    position: 'relative',
  },
  rounded: { borderRadius: 999 },
  fill: {
    height: '100%',
    width: '100%',
    backgroundColor: vars.primary500,
    borderRadius: 'inherit',
    transitionProperty: 'transform',
    transitionDuration: '200ms',
  },
  indeterminate: {
    width: '25%',
    transform: 'none',
    animationName: slide,
    animationDuration: '1.2s',
    animationTimingFunction: 'ease-in-out',
    animationIterationCount: 'infinite',
  },
})
