import type { CSSProperties } from 'react'
// Phosphor is the icon set @antfu/design itself uses (i-ph:*); the wrappers
// below expose it through the same `Icon*` API so call sites stay uniform.
import {
  ArrowUpRight,
  BellRinging,
  BookmarkSimple,
  ChartLine,
  ClockCounterClockwise,
  Cube,
  DownloadSimple,
  FileCode,
  Gauge,
  GitPullRequest,
  Globe,
  Graph,
  HardDrives,
  Notebook,
  Pulse,
  Sparkle,
  Timer,
  TrendUp,
} from '@phosphor-icons/react'

/**
 * Minimal stroke-icon set (16x16, currentColor) — the devtools-vocabulary
 * glyphs the shell and screens need. Stroke-only keeps them crisp at the
 * small sizes this UI uses.
 */

function Svg({ children, size = 16 }: { children: React.ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      style={{ display: 'block', flexShrink: 0 } as CSSProperties}
    >
      {children}
    </svg>
  )
}

export const IconSearch = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="7" cy="7" r="4.5" />
    <path d="m14 14-3.5-3.5" />
  </Svg>
)

export const IconLayers = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m8 1.5 6.5 3.5L8 8.5 1.5 5 8 1.5Z" />
    <path d="m1.5 8.5 6.5 3.5 6.5-3.5" />
    <path d="m1.5 12 6.5 3.5L14.5 12" />
  </Svg>
)

export const IconDatabase = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <ellipse cx="8" cy="3.5" rx="5.5" ry="2" />
    <path d="M2.5 3.5v9c0 1.1 2.46 2 5.5 2s5.5-.9 5.5-2v-9" />
    <path d="M2.5 8c0 1.1 2.46 2 5.5 2s5.5-.9 5.5-2" />
  </Svg>
)

export const IconSun = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="3" />
    <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M12.6 3.4l-1 1M4.4 11.6l-1 1" />
  </Svg>
)

export const IconMoon = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M13.5 9.5A6 6 0 0 1 6.5 2.5a6 6 0 1 0 7 7Z" />
  </Svg>
)

export const IconRefresh = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M14 8a6 6 0 1 1-1.76-4.24" />
    <path d="M14 2v3.5h-3.5" />
  </Svg>
)

export const IconChevron = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m6 4 4 4-4 4" />
  </Svg>
)

export const IconX = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m4 4 8 8M12 4l-8 8" />
  </Svg>
)

export const IconFile = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3.5 1.5h6l3 3v10h-9v-13Z" />
    <path d="M9.5 1.5v3h3" />
  </Svg>
)

export const IconBolt = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M8.5 1.5 3 9h4l-1.5 5.5L11 7.5H7L8.5 1.5Z" />
  </Svg>
)

export const IconInfo = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="6.5" />
    <path d="M8 7.5V11M8 5v.01" />
  </Svg>
)

export const IconWarning = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M8 1.5 15 14H1L8 1.5Z" />
    <path d="M8 6.5v3M8 11.5v.01" />
  </Svg>
)

export const IconCheck = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="6.5" />
    <path d="m5 8.5 2 2 4-4.5" />
  </Svg>
)

export const IconError = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="6.5" />
    <path d="m5.5 5.5 5 5M10.5 5.5l-5 5" />
  </Svg>
)

export const IconArrowRight = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M2.5 8h11M10 4.5 13.5 8 10 11.5" />
  </Svg>
)

export const IconGitCommit = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="2.5" />
    <path d="M1.5 8h4M10.5 8h4" />
  </Svg>
)

export const IconDiff = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M5.5 1.5v13M10.5 1.5v13" />
    <path d="m3.5 6 2 2-2 2M12.5 6l-2 2 2 2" />
  </Svg>
)

export const IconSpinner = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M8 1.5a6.5 6.5 0 1 1-6.13 8.7" />
  </Svg>
)

export const IconFolder = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M1.5 3.5v9h13v-8h-6l-2-2h-5Z" />
  </Svg>
)

export const IconFolderOpen = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M1.5 3.5v9h11l2-6H5.5l-2 4v-7Z" />
    <path d="M1.5 3.5h5l2 2h5" />
  </Svg>
)

export const IconCaretDown = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m4 6 4 4 4-4" />
  </Svg>
)

export const IconCaretUp = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m4 10 4-4 4 4" />
  </Svg>
)

export const IconCaretLeft = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m10 4-4 4 4 4" />
  </Svg>
)

export const IconCaretRight = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m6 4 4 4-4 4" />
  </Svg>
)

export const IconCheckSmall = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m4 8.5 2.5 2.5L12 5" />
  </Svg>
)

export const IconMinus = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3.5 8h9" />
  </Svg>
)

export const IconPlus = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M8 3.5v9M3.5 8h9" />
  </Svg>
)

export const IconLink = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M6.5 9.5a3 3 0 0 0 4.24.24l2-2a3 3 0 0 0-4.24-4.24l-1 1" />
    <path d="M9.5 6.5a3 3 0 0 0-4.24-.24l-2 2a3 3 0 0 0 4.24 4.24l1-1" />
  </Svg>
)

export const IconEllipsis = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="3.5" cy="8" r="0.8" fill="currentColor" />
    <circle cx="8" cy="8" r="0.8" fill="currentColor" />
    <circle cx="12.5" cy="8" r="0.8" fill="currentColor" />
  </Svg>
)

export const IconClock = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="8" cy="8" r="6.5" />
    <path d="M8 4.5V8l2.5 1.5" />
  </Svg>
)

export const IconTag = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M2 2h5.5L14 8.5 8.5 14 2 7.5V2Z" />
    <circle cx="5.5" cy="5.5" r="1" fill="currentColor" stroke="none" />
  </Svg>
)

export const IconGitBranch = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <circle cx="4.5" cy="3.5" r="1.8" />
    <circle cx="4.5" cy="12.5" r="1.8" />
    <circle cx="11.5" cy="5.5" r="1.8" />
    <path d="M4.5 5.3v5.4M11.5 7.3c0 2.5-3 3.7-5.5 4" />
  </Svg>
)

export const IconCopy = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="5.5" y="5.5" width="8" height="8" rx="1" />
    <path d="M10.5 3.5v-1h-8v8h1" />
  </Svg>
)

export const IconExternal = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M6.5 3.5h-3v9h9v-3" />
    <path d="M9.5 3.5h3v3M12.5 3.5 8 8" />
  </Svg>
)

export const IconImage = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="2" y="3" width="12" height="10" rx="1" />
    <circle cx="5.5" cy="6.5" r="1" />
    <path d="m3 12 4-4 2.5 2.5L12 8l2 2" />
  </Svg>
)

export const IconPackage = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="m8 1.5 5.5 3v7L8 14.5 2.5 11.5v-7L8 1.5Z" />
    <path d="M8 8 2.5 4.5M8 8l5.5-3.5M8 8v6.5M5.2 2.7l5.6 3.4" />
  </Svg>
)

export const IconTerminal = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <rect x="2" y="3" width="12" height="10" rx="1" />
    <path d="m5 6.5 2 2-2 2M8.5 10.5H11" />
  </Svg>
)

export const IconStar = ({ size, filled }: { size?: number; filled?: boolean }) => (
  <Svg size={size}>
    <path
      d="m8 2.5 1.7 3.5 3.8.5-2.8 2.7.7 3.8L8 11.2 4.6 13l.7-3.8L2.5 6.5l3.8-.5Z"
      fill={filled ? 'currentColor' : 'none'}
    />
  </Svg>
)

export const IconBook = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M3 2.5h6.5a1 1 0 0 1 1 1V13a1 1 0 0 0-1-1H3Z M10.5 3.5H13v9h-3.5" />
  </Svg>
)

export const IconWrap = ({ size }: { size?: number }) => (
  <Svg size={size}>
    <path d="M2.5 4h11M2.5 8h6.5a2.5 2.5 0 0 1 0 5H7" />
    <path d="m8.6 10.6-1.8 1.9 1.8 1.9" />
  </Svg>
)

// --- Phosphor-backed icons --------------------------------------------------

export const IconGauge = ({ size = 16 }: { size?: number }) => <Gauge size={size} />

export const IconPulse = ({ size = 16 }: { size?: number }) => <Pulse size={size} />

export const IconTimer = ({ size = 16 }: { size?: number }) => <Timer size={size} />

export const IconFileCode = ({ size = 16 }: { size?: number }) => <FileCode size={size} />

export const IconCube = ({ size = 16 }: { size?: number }) => <Cube size={size} />

export const IconTrendUp = ({ size = 16 }: { size?: number }) => <TrendUp size={size} />

export const IconChartLine = ({ size = 16 }: { size?: number }) => <ChartLine size={size} />

export const IconHistoryClock = ({ size = 16 }: { size?: number }) => (
  <ClockCounterClockwise size={size} />
)

export const IconArrowUpRight = ({ size = 16 }: { size?: number }) => (
  <ArrowUpRight size={size} />
)

export const IconGraph = ({ size = 16 }: { size?: number }) => <Graph size={size} />
export const IconNotebook = ({ size = 16 }: { size?: number }) => <Notebook size={size} />
export const IconGitPullRequest = ({ size = 16 }: { size?: number }) => (
  <GitPullRequest size={size} />
)
export const IconBell = ({ size = 16 }: { size?: number }) => <BellRinging size={size} />
export const IconBookmark = ({ size = 16, filled }: { size?: number; filled?: boolean }) => (
  <BookmarkSimple size={size} weight={filled ? 'fill' : 'regular'} />
)
export const IconGlobe = ({ size = 16 }: { size?: number }) => <Globe size={size} />

export const IconHardDrives = ({ size = 16 }: { size?: number }) => <HardDrives size={size} />

export const IconSparkle = ({ size = 16 }: { size?: number }) => <Sparkle size={size} />

export const IconDownload = ({ size = 16 }: { size?: number }) => (
  <DownloadSimple size={size} />
)
