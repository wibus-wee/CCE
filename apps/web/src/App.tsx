import * as stylex from '@stylexjs/stylex'
import { useCallback, useEffect, useRef, useState } from 'react'
import { api, describeError, ProviderReport, ViewManifest } from './api'
import { ContextScreen } from './screens/ContextScreen'
import { IndexScreen } from './screens/IndexScreen'
import { QueryScreen } from './screens/QueryScreen'
import { ActionButton } from './ui/ActionButton'
import { ActionIconButton } from './ui/ActionIconButton'
import { IconBolt, IconDatabase, IconLayers, IconMoon, IconRefresh, IconSearch, IconSun, IconWarning } from './ui/icons'
import { LayoutToolbar } from './ui/LayoutToolbar'
import { fontMono, vars, type Severity } from './ui/tokens.stylex'
import { useDark } from './ui/useDark'

type Screen = 'query' | 'context' | 'index'

const NAV: { id: Screen; label: string; icon: React.ReactNode; hint: string }[] = [
  { id: 'query', label: 'Query', icon: <IconSearch size={15} />, hint: 'Search the index' },
  { id: 'context', label: 'Context', icon: <IconLayers size={15} />, hint: 'Build a source-linked pack' },
  { id: 'index', label: 'Index', icon: <IconDatabase size={15} />, hint: 'Views and providers' },
]

export function App() {
  const { dark, toggle } = useDark()
  const [screen, setScreen] = useState<Screen>('query')
  const [manifest, setManifest] = useState<ViewManifest>()
  const [providers, setProviders] = useState<ProviderReport[]>()
  const [providersError, setProvidersError] = useState<string>()
  const [statusError, setStatusError] = useState<string>()
  const [indexing, setIndexing] = useState(false)
  const [refreshing, setRefreshing] = useState(true)
  const searchRef = useRef<HTMLInputElement>(null)

  const refresh = useCallback(async () => {
    // Status and provider probes are independent: one failing must not
    // blank the other.
    const [status, providerReports] = await Promise.allSettled([api.status(), api.providers()])
    if (status.status === 'fulfilled') {
      setManifest(status.value)
      setStatusError(undefined)
    } else {
      setStatusError(describeError(status.reason))
    }
    if (providerReports.status === 'fulfilled') {
      setProviders(providerReports.value)
      setProvidersError(undefined)
    } else {
      setProvidersError(describeError(providerReports.reason))
    }
    setRefreshing(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    function onKeydown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setScreen('query')
        // wait a frame so the input is visible before focusing
        requestAnimationFrame(() => searchRef.current?.focus())
      }
    }
    window.addEventListener('keydown', onKeydown)
    return () => window.removeEventListener('keydown', onKeydown)
  }, [])

  async function reindex() {
    setIndexing(true)
    setStatusError(undefined)
    try {
      const report = await api.index()
      setManifest(report.manifest)
      // IndexReport.providers carries run outcomes (duration, SCIP counts);
      // fresher than re-probing.
      setProviders(report.providers)
      setProvidersError(undefined)
    } catch (value) {
      setStatusError(describeError(value))
    } finally {
      setIndexing(false)
    }
  }

  const degraded = manifest ? Object.entries(manifest.views).filter(([, v]) => v.state !== 'ready') : []

  return (
    <div {...stylex.props(styles.shell)}>
      <LayoutToolbar
        start={
          <>
            <span {...stylex.props(styles.mark)}>
              <IconBolt size={14} />
            </span>
            <strong {...stylex.props(styles.wordmark)}>CCE</strong>
            <span {...stylex.props(styles.tagline)}>repository intelligence</span>
          </>
        }
        end={
          <>
            {manifest && (
              <code {...stylex.props(styles.snapshot)} title={`snapshot ${manifest.snapshotId}`}>
                {manifest.snapshotId.slice(0, 12)}
              </code>
            )}
            {degraded.length > 0 && (
              <span
                {...stylex.props(styles.degraded)}
                title={degraded.map(([name, v]) => `${name}: ${v.state.replaceAll('_', ' ')}`).join('\n')}
              >
                <IconWarning size={12} />
                {degraded.length} degraded
              </span>
            )}
            <ActionButton
              variant="action"
              icon={<IconRefresh size={13} />}
              loading={indexing}
              onClick={() => void reindex()}
            >
              Reindex
            </ActionButton>
            <ActionIconButton tooltip={dark ? 'Light mode' : 'Dark mode'} onClick={toggle}>
              {dark ? <IconSun size={15} /> : <IconMoon size={15} />}
            </ActionIconButton>
          </>
        }
      />

      <div {...stylex.props(styles.body)}>
        <nav {...stylex.props(styles.nav)} aria-label="Sections">
          {NAV.map((item) => (
            <button
              key={item.id}
              type="button"
              title={item.hint}
              aria-current={screen === item.id}
              {...stylex.props(styles.navItem, screen === item.id && styles.navActive)}
              onClick={() => setScreen(item.id)}
            >
              {item.icon}
              <span>{item.label}</span>
            </button>
          ))}
          <div {...stylex.props(styles.navFoot)}>
            <HealthDot manifest={manifest} error={statusError} loading={refreshing} />
          </div>
        </nav>

        <main {...stylex.props(styles.main)}>
          <div {...stylex.props(styles.screen)} hidden={screen !== 'query'}>
            <QueryScreen
              ref={searchRef}
              dark={dark}
              onManifest={(m) => setManifest(m)}
              onReindex={() => void reindex()}
            />
          </div>
          <div {...stylex.props(styles.screen)} hidden={screen !== 'context'}>
            <ContextScreen dark={dark} />
          </div>
          <div {...stylex.props(styles.screen)} hidden={screen !== 'index'}>
            <IndexScreen
              manifest={manifest}
              providers={providers}
              providersError={providersError}
              statusError={statusError}
              loading={refreshing}
              onReindex={() => void reindex()}
            />
          </div>
        </main>
      </div>
    </div>
  )
}

function HealthDot({
  manifest,
  error,
  loading,
}: {
  manifest?: ViewManifest
  error?: string
  loading: boolean
}) {
  if (loading) {
    return <span {...stylex.props(styles.health)} title="connecting…">connecting</span>
  }
  if (error) {
    return (
      <span {...stylex.props(styles.health, healthColor.critical)} title={error}>
        daemon down
      </span>
    )
  }
  if (!manifest) return null
  const degraded = Object.entries(manifest.views).filter(([, v]) => v.state !== 'ready')
  const severity: Severity = degraded.length === 0 ? 'low' : degraded.length > 2 ? 'high' : 'medium'
  return (
    <span
      {...stylex.props(styles.health, healthColor[severity])}
      title={`snapshot ${manifest.snapshotId}`}
    >
      <span {...stylex.props(styles.dot)} />
      {degraded.length === 0 ? 'all views ready' : `${degraded.length} degraded`}
    </span>
  )
}

const healthColor = stylex.create({
  low: { color: vars.scaleLow },
  medium: { color: vars.scaleMedium },
  high: { color: vars.scaleHigh },
  critical: { color: vars.scaleCritical },
})

const styles = stylex.create({
  shell: {
    minHeight: '100vh',
    backgroundColor: vars.bgBase,
    color: vars.colorBase,
    display: 'flex',
    flexDirection: 'column',
  },
  mark: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 24,
    height: 24,
    borderRadius: 6,
    backgroundColor: vars.primary500,
    color: vars.onPrimary,
    flexShrink: 0,
  },
  wordmark: {
    fontSize: 14,
    fontWeight: 700,
    letterSpacing: '0.02em',
  },
  tagline: {
    fontSize: 11,
    color: vars.colorFaint,
    display: { default: 'inline', '@media (max-width: 640px)': 'none' },
  },
  snapshot: {
    fontFamily: fontMono,
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: vars.bgSunken,
    borderRadius: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 6,
    display: { default: 'inline-block', '@media (max-width: 720px)': 'none' },
  },
  degraded: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    fontSize: 11,
    color: vars.scaleMedium,
    whiteSpace: 'nowrap',
  },
  body: {
    display: 'flex',
    flex: 1,
    minHeight: 0,
  },
  nav: {
    width: 148,
    flexShrink: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    padding: 10,
    borderRightWidth: 1,
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
    position: 'sticky',
    top: 46,
    height: 'calc(100vh - 46px)',
    display: { default: 'flex', '@media (max-width: 640px)': 'none' },
  },
  navItem: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 6,
    border: 'none',
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorBase,
    },
    fontSize: 13,
    cursor: 'pointer',
    textAlign: 'left',
    transitionProperty: 'background-color, color',
    transitionDuration: '120ms',
  },
  navActive: {
    backgroundColor: vars.bgActive,
    color: vars.colorActive,
  },
  navFoot: {
    marginTop: 'auto',
    paddingTop: 10,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  health: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 10,
    color: vars.colorFaint,
    fontFamily: fontMono,
    whiteSpace: 'nowrap',
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: '50%',
    backgroundColor: 'currentColor',
    flexShrink: 0,
  },
  main: {
    flex: 1,
    minWidth: 0,
    padding: 20,
    paddingBottom: 64,
    maxWidth: 1080px,
  },
  screen: {
    maxWidth: 960px,
  },
})
