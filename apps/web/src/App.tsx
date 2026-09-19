import * as stylex from '@stylexjs/stylex'
import { useHotkeys, type UseHotkeyDefinition } from '@tanstack/react-hotkeys'
import { useCallback, useEffect, useState } from 'react'
import { Navigate, Route, Routes, useLocation, useNavigate } from 'react-router'
import { api, apiBase, describeError, ProviderReport, ViewManifest } from './api'
import { CommandPalette } from './components/CommandPalette'
import { ShortcutsModal } from './components/ShortcutsModal'
import {
  fileUrl,
  pathForScreen,
  queryUrl,
  screenForPath,
  useOpenFile,
  type ScreenId,
} from './lib/navigation'
import { useSearchHistory } from './lib/searchHistory'
import { useAppStore } from './lib/store'
import { AboutScreen } from './screens/AboutScreen'
import { BatchChangesScreen } from './screens/BatchChangesScreen'
import { BranchesScreen } from './screens/BranchesScreen'
import { BrowseScreen } from './screens/BrowseScreen'
import { ChangesScreen } from './screens/ChangesScreen'
import { CommitScreen } from './screens/CommitScreen'
import { CommitsScreen } from './screens/CommitsScreen'
import { CompareScreen } from './screens/CompareScreen'
import { MapScreen } from './screens/MapScreen'
import { ContextScreen } from './screens/ContextScreen'
import { ContextsScreen } from './screens/ContextsScreen'
import { SavedSearchesScreen } from './screens/SavedSearchesScreen'
import { DashboardScreen } from './screens/DashboardScreen'
import { IndexScreen } from './screens/IndexScreen'
import { InsightsScreen } from './screens/InsightsScreen'
import { MonitoringScreen } from './screens/MonitoringScreen'
import { NotebooksScreen } from './screens/NotebooksScreen'
import { QueryScreen } from './screens/QueryScreen'
import { SymbolsScreen } from './screens/SymbolsScreen'
import { ActionButton } from './ui/ActionButton'
import { ActionIconButton } from './ui/ActionIconButton'
import { NotificationProvider, useNotification } from './ui/FeedbackToasts'
import { DisplayKbd } from './ui/DisplayKbd'
import {
  IconArrowUpRight,
  IconBell,
  IconBookmark,
  IconCaretLeft,
  IconCaretRight,
  IconChartLine,
  IconDatabase,
  IconDiff,
  IconFile,
  IconFolder,
  IconGauge,
  IconGitBranch,
  IconGitCommit,
  IconGitPullRequest,
  IconGlobe,
  IconGraph,
  IconHistoryClock,
  IconInfo,
  IconLayers,
  IconMoon,
  IconNotebook,
  IconPackage,
  IconRefresh,
  IconSearch,
  IconSun,
  IconWarning,
} from './ui/icons'
import { LogoMark } from './ui/LogoMark'
import { font, vars, type Severity } from './ui/tokens.stylex'
import {
  OverlayDropdown,
  OverlayDropdownItem,
  OverlayDropdownSeparator,
} from './ui/OverlayMenu'
import { OverlayTooltip } from './ui/OverlayTooltip'
import { ColorSchemeProvider, useDark } from './ui/useDark'

interface NavItem {
  id: ScreenId
  label: string
  icon: React.ReactNode
  hint: string
  /** `preview` surfaces have no backend endpoint — the badge keeps that
   * honest in the rail itself, not just on the screen. */
  preview?: boolean
  /** Digit shortcut (bare key, outside inputs). */
  key?: string
}

interface NavGroup {
  label: string
  items: NavItem[]
}

// Sidebar mirrors Sourcegraph's full product surface: search, repository,
// code-management tools, the personal library (live localStorage data),
// and system. Items without a backend are kept in the rail but marked
// `preview` — the rail documents the intended product, not just what ships.
const NAV: NavGroup[] = [
  {
    label: 'Search',
    items: [
      { id: 'dashboard', label: 'Home', icon: <IconGauge size={15} />, hint: 'Search home', key: '1' },
      { id: 'query', label: 'Query', icon: <IconSearch size={15} />, hint: 'Search the index', key: '2' },
      { id: 'contexts', label: 'Contexts', icon: <IconGlobe size={15} />, hint: 'Search contexts — repo/revision scopes', preview: true },
    ],
  },
  {
    label: 'Repository',
    items: [
      { id: 'browse', label: 'Files', icon: <IconFolder size={15} />, hint: 'Files in the snapshot', key: '3' },
      { id: 'symbols', label: 'Symbols', icon: <IconPackage size={15} />, hint: 'Symbol index', preview: true, key: '4' },
      { id: 'commits', label: 'Commits', icon: <IconGitCommit size={15} />, hint: 'Commit history', preview: true },
      { id: 'branches', label: 'Branches', icon: <IconGitBranch size={15} />, hint: 'Refs and divergence', preview: true },
      { id: 'changes', label: 'Changes', icon: <IconDiff size={15} />, hint: 'Snapshot delta — what changed since last index' },
      { id: 'map', label: 'Map', icon: <IconGraph size={15} />, hint: 'Package dependency graph' },
      { id: 'context', label: 'Packs', icon: <IconLayers size={15} />, hint: 'Build a source-linked context pack', key: '5' },
    ],
  },
  {
    label: 'Tools',
    items: [
      { id: 'notebooks', label: 'Notebooks', icon: <IconNotebook size={15} />, hint: 'Living docs — prose + query blocks', preview: true },
      { id: 'batch', label: 'Batch Changes', icon: <IconGitPullRequest size={15} />, hint: 'Fleet edits across the codebase', preview: true },
      { id: 'monitoring', label: 'Monitoring', icon: <IconBell size={15} />, hint: 'Snapshot watchers — new matches fire events on reindex' },
      { id: 'insights', label: 'Insights', icon: <IconChartLine size={15} />, hint: 'Trends over snapshots', preview: true },
    ],
  },
  {
    label: 'System',
    items: [
      { id: 'index', label: 'Index', icon: <IconDatabase size={15} />, hint: 'Views and providers', key: '6' },
    ],
  },
]

const NAV_SHORTCUTS = new Map(
  NAV.flatMap((g) => g.items)
    .filter((i) => i.key)
    .map((i) => [i.key as string, i.id]),
)

// Spring-approximating bezier — strong ease-out, no overshoot (overshoot on
// width causes overflow flicker at the end of the collapse).
const SPRING = 'cubic-bezier(0.22, 1, 0.36, 1)'
const SIDEBAR_DEFAULT = 208
const SIDEBAR_CLOSED = 52

export function App() {
  const { dark, toggle } = useDark()
  return (
    <ColorSchemeProvider value={dark ? 'dark' : 'light'}>
      <NotificationProvider>
        <AppShell dark={dark} toggle={toggle} />
      </NotificationProvider>
    </ColorSchemeProvider>
  )
}

function AppShell({ dark, toggle }: { dark: boolean; toggle: () => void }) {
  const notify = useNotification()
  const navigate = useNavigate()
  const location = useLocation()
  const openFile = useOpenFile()

  // Store-backed UI state — was prop-drilled useState.
  const query = useAppStore((s) => s.query)
  const setQuery = useAppStore((s) => s.setQuery)
  const paletteOpen = useAppStore((s) => s.paletteOpen)
  const setPaletteOpen = useAppStore((s) => s.setPaletteOpen)
  const shortcutsOpen = useAppStore((s) => s.shortcutsOpen)
  const setShortcutsOpen = useAppStore((s) => s.setShortcutsOpen)
  const collapsed = useAppStore((s) => s.collapsed)
  const toggleSidebar = useAppStore((s) => s.toggleSidebar)
  const sidebarWidth = useAppStore((s) => s.sidebarWidth)
  const setSidebarWidth = useAppStore((s) => s.setSidebarWidth)
  const [resizing, setResizing] = useState(false)
  const [resizeHover, setResizeHover] = useState(false)
  const manifest = useAppStore((s) => s.manifest)
  const setManifest = useAppStore((s) => s.setManifest)

  // Shell-local probes — server state, not UI state.
  const [providers, setProviders] = useState<ProviderReport[]>()
  const [providersError, setProvidersError] = useState<string>()
  const [statusError, setStatusError] = useState<string>()
  const [indexing, setIndexing] = useState(false)
  const [refreshing, setRefreshing] = useState(true)
  // Library — live localStorage data (SG's saved/recent sidebar section).
  const { recents, saved } = useSearchHistory()
  const recentFiles = useAppStore((s) => s.recentFiles)

  const screen = screenForPath(location.pathname)

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

  // Executed queries live in the URL — shareable, restorable on refresh.
  const runQuery = useCallback(
    (q: string) => {
      setQuery(q)
      navigate(queryUrl(q))
    },
    [navigate, setQuery],
  )

  // Global hotkeys — one registration table, managed by @tanstack/hotkeys.
  // `Mod` resolves to ⌘/Ctrl per platform; bare keys auto-ignore text inputs.
  useHotkeys([
    // ⌘K command palette; ⌘P is Sourcegraph's file-jump chord — the
    // palette's FILES group covers it, so both open the same surface.
    { hotkey: 'Mod+K', callback: () => setPaletteOpen(true) },
    { hotkey: 'Mod+P', callback: () => setPaletteOpen(true) },
    { hotkey: 'Mod+B', callback: toggleSidebar },
    // Shift+/ produces `?` — a shifted char, so it goes in as a RawHotkey.
    { hotkey: { key: '?', shift: true }, callback: () => setShortcutsOpen(true) },
    // Sourcegraph's `/` — focus search; the palette is our search surface.
    { hotkey: '/', callback: () => setPaletteOpen(true) },
    {
      // Sourcegraph's `y` — copy the canonical link. Pages publish their own
      // permalink (Browse); everywhere else the URL is already canonical.
      hotkey: 'Y',
      callback: () => {
        const link = useAppStore.getState().permalink ?? window.location.href
        navigator.clipboard
          .writeText(link)
          .then(() => notify.push('Link copied', { type: 'success' }))
          .catch(() =>
            notify.push('Copy failed', {
              type: 'error',
              description: 'Clipboard permission denied',
            }),
          )
      },
    },
    ...[...NAV_SHORTCUTS].map(
      ([key, id]): UseHotkeyDefinition => ({
        hotkey: key as UseHotkeyDefinition['hotkey'],
        callback: () => navigate(pathForScreen(id)),
      }),
    ),
  ])

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
      const degraded = Object.values(report.manifest.views).filter((v) => v.state !== 'ready')
      notify.push('Index refreshed', {
        type: degraded.length > 0 ? 'warning' : 'success',
        description:
          degraded.length > 0
            ? `${degraded.length} view${degraded.length === 1 ? '' : 's'} still degraded`
            : 'All views ready',
      })
    } catch (value) {
      const message = describeError(value)
      setStatusError(message)
      notify.push('Reindex failed', { type: 'error', description: message })
    } finally {
      setIndexing(false)
    }
  }

  const degraded = manifest ? Object.entries(manifest.views).filter(([, v]) => v.state !== 'ready') : []

  return (
    <div {...stylex.props(styles.shell)}>
      <header {...stylex.props(styles.topbar)}>
        <ActionIconButton
          tooltip={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          label="Toggle sidebar"
          onClick={toggleSidebar}
          icon={collapsed ? <IconCaretRight size={14} /> : <IconCaretLeft size={14} />}
        />
        <span {...stylex.props(styles.mark)}>
          <LogoMark size={15} />
        </span>
        <strong {...stylex.props(styles.wordmark)}>CCE</strong>
        <span {...stylex.props(styles.tagline)}>repository intelligence</span>
        <button
          type="button"
          onClick={() => setPaletteOpen(true)}
          {...stylex.props(styles.searchTrigger)}
          aria-label="Open command palette"
        >
          <IconSearch size={14} />
          <span {...stylex.props(styles.searchTriggerText)}>
            {query || 'search the index — symbols, paths, questions'}
          </span>
          <DisplayKbd keys="mod+k" />
        </button>
        <span {...stylex.props(styles.spacer)} />
        {manifest && (
          <code {...stylex.props(styles.snapshot)} title={`snapshot ${manifest.snapshotId}`}>
            {manifest.snapshotId.slice(0, 12)}
          </code>
        )}
        {degraded.length > 0 && (
          <OverlayTooltip
            tip={
              <span {...stylex.props(styles.degradedTip)}>
                {degraded.map(([name, v]) => (
                  <span key={name}>
                    {name}: {v.state.replaceAll('_', ' ')}
                  </span>
                ))}
              </span>
            }
          >
            <span {...stylex.props(styles.degraded)}>
              <IconWarning size={12} />
              {degraded.length} degraded
            </span>
          </OverlayTooltip>
        )}
        <ActionButton
          variant="action"
          icon={<IconRefresh size={13} />}
          loading={indexing}
          onClick={() => void reindex()}
        >
          Reindex
        </ActionButton>
        <OverlayDropdown
          align="end"
          trigger={
            <ActionIconButton
              tooltip="Help"
              label="Help"
              icon={<span {...stylex.props(styles.helpKey)}>?</span>}
            />
          }
        >
          <OverlayDropdownItem
            shortcut={<DisplayKbd keys="?" size="sm" />}
            onClick={() => setShortcutsOpen(true)}
          >
            Keyboard shortcuts
          </OverlayDropdownItem>
          <OverlayDropdownItem
            icon={<IconInfo size={13} />}
            onClick={() => navigate('/about')}
          >
            About CCE
          </OverlayDropdownItem>
          <OverlayDropdownSeparator />
          <OverlayDropdownItem
            icon={<IconArrowUpRight size={13} />}
            onClick={() => window.open(`${apiBase()}/docs`, '_blank', 'noopener')}
          >
            API reference
          </OverlayDropdownItem>
        </OverlayDropdown>
        <ActionIconButton
          tooltip={dark ? 'Light mode' : 'Dark mode'}
          label={dark ? 'Light mode' : 'Dark mode'}
          onClick={toggle}
          icon={dark ? <IconSun size={15} /> : <IconMoon size={15} />}
        />
      </header>

      <div {...stylex.props(styles.body)}>
        <aside
          {...stylex.props(styles.sidebar)}
          style={{
            width: collapsed ? SIDEBAR_CLOSED : sidebarWidth,
            transitionProperty: resizing ? 'none' : undefined,
          }}
          aria-label="Primary"
        >
          <nav {...stylex.props(styles.sideInner)}>
            <div {...stylex.props(styles.sideScroll)}>
              {NAV.map((group, gi) => (
                <div key={group.label} {...stylex.props(gi > 0 && styles.sideGroupGap)}>
                  {!collapsed && (
                    <div {...stylex.props(styles.sideGroupLabel)}>{group.label}</div>
                  )}
                  {collapsed && gi > 0 && <div {...stylex.props(styles.sideGroupRule)} />}
                  {group.items.map((item) => (
                    <button
                      key={item.id}
                      type="button"
                      title={item.hint}
                      aria-current={item.id === screen ? 'page' : undefined}
                      onClick={() => navigate(pathForScreen(item.id))}
                      {...stylex.props(styles.sideItem, item.id === screen && styles.sideItemActive)}
                    >
                      <span {...stylex.props(styles.sideIcon)}>{item.icon}</span>
                      <span
                        {...stylex.props(styles.sideLabel)}
                        style={{
                          opacity: collapsed ? 0 : 1,
                          transitionDelay: collapsed ? '0ms' : '90ms',
                        }}
                      >
                        {item.label}
                      </span>
                      {item.preview && !collapsed && (
                        <span {...stylex.props(styles.sidePreview)}>preview</span>
                      )}
                    </button>
                  ))}
                </div>
              ))}

              {!collapsed && (
                <div {...stylex.props(styles.sideGroupGap)}>
                  <div {...stylex.props(styles.sideGroupLabel)}>Library</div>
                  <button
                    type="button"
                    title="All saved searches"
                    aria-current={screen === 'saved' ? 'page' : undefined}
                    onClick={() => navigate(pathForScreen('saved'))}
                    {...stylex.props(styles.sideItem, screen === 'saved' && styles.sideItemActive)}
                  >
                    <span {...stylex.props(styles.sideIcon)}>
                      <IconBookmark size={13} />
                    </span>
                    <span {...stylex.props(styles.sideLabel)}>Saved searches</span>
                  </button>
                  {saved.slice(0, 3).map((q) => (
                    <LibraryRow
                      key={`s:${q}`}
                      icon={<IconBookmark size={13} />}
                      text={q}
                      title={`saved — ${q}`}
                      onClick={() => runQuery(q)}
                    />
                  ))}
                  {recents.slice(0, 3).map((q) => (
                    <LibraryRow
                      key={`r:${q}`}
                      icon={<IconHistoryClock size={13} />}
                      text={q}
                      title={`recent — ${q}`}
                      onClick={() => runQuery(q)}
                    />
                  ))}
                  {recentFiles.slice(0, 3).map((f) => (
                    <LibraryRow
                      key={`f:${f.path}`}
                      icon={<IconFile size={13} />}
                      text={f.path.split('/').pop() ?? f.path}
                      title={f.path}
                      onClick={() => navigate(fileUrl(f.path))}
                    />
                  ))}
                </div>
              )}
            </div>

            <HealthDot manifest={manifest} error={statusError} loading={refreshing} collapsed={collapsed} />
          </nav>
          {!collapsed && (
            <div
              {...stylex.props(styles.resize)}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize sidebar"
              title="Drag to resize · double-click to reset"
              onPointerEnter={() => setResizeHover(true)}
              onPointerLeave={() => setResizeHover(false)}
              onPointerDown={(e) => {
                e.preventDefault()
                const startX = e.clientX
                const startW = sidebarWidth
                setResizing(true)
                document.body.style.userSelect = 'none'
                document.body.style.cursor = 'col-resize'
                const move = (ev: PointerEvent) => setSidebarWidth(startW + ev.clientX - startX)
                const up = () => {
                  setResizing(false)
                  document.body.style.userSelect = ''
                  document.body.style.cursor = ''
                  window.removeEventListener('pointermove', move)
                  window.removeEventListener('pointerup', up)
                }
                window.addEventListener('pointermove', move)
                window.addEventListener('pointerup', up)
              }}
              onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT)}
            >
              <span
                {...stylex.props(
                  styles.resizeLine,
                  (resizeHover || resizing) && styles.resizeLineActive,
                )}
              />
            </div>
          )}
        </aside>

        <main {...stylex.props(styles.panel)}>
          <div {...stylex.props(styles.scroll)}>
            <Routes>
              <Route
                path="/"
                element={
                  <DashboardScreen
                    manifest={manifest}
                    providers={providers}
                    providersError={providersError}
                    statusError={statusError}
                    loading={refreshing}
                    onRunQuery={runQuery}
                  />
                }
              />
              <Route
                path="/query"
                element={<QueryScreen onOpenFile={openFile} />}
              />
              <Route path="/browse/*" element={<BrowseScreen />} />
              <Route path="/browse" element={<BrowseScreen />} />
              <Route path="/symbols" element={<SymbolsScreen onOpenFile={openFile} />} />
              <Route path="/commits" element={<CommitsScreen />} />
              <Route path="/commit/*" element={<CommitScreen />} />
              <Route path="/compare" element={<CompareScreen />} />
              <Route path="/branches" element={<BranchesScreen />} />
              <Route path="/map" element={<MapScreen />} />
              <Route path="/changes" element={<ChangesScreen />} />
              <Route path="/context" element={<ContextScreen />} />
              <Route path="/contexts" element={<ContextsScreen />} />
              <Route path="/saved" element={<SavedSearchesScreen />} />
              <Route path="/notebooks" element={<NotebooksScreen />} />
              <Route path="/batch-changes" element={<BatchChangesScreen />} />
              <Route path="/code-monitoring" element={<MonitoringScreen />} />
              <Route path="/insights" element={<InsightsScreen />} />
              <Route
                path="/index"
                element={
                  <IndexScreen
                    manifest={manifest}
                    providers={providers}
                    providersError={providersError}
                    statusError={statusError}
                    loading={refreshing}
                    onReindex={() => void reindex()}
                  />
                }
              />
              <Route path="/about" element={<AboutScreen />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </div>
        </main>
      </div>

      <ShortcutsModal open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        initial={query}
        onRunQuery={runQuery}
        onNavigate={(s) => navigate(pathForScreen(s))}
        onReindex={() => void reindex()}
        onToggleTheme={toggle}
        onToggleSidebar={toggleSidebar}
        onShowShortcuts={() => setShortcutsOpen(true)}
        dark={dark}
      />
    </div>
  )
}

/** Sidebar library row — saved/recent query or recent file, one click deep. */
function LibraryRow({
  icon,
  text,
  title,
  onClick,
}: {
  icon: React.ReactNode
  text: string
  title: string
  onClick: () => void
}) {
  return (
    <button type="button" title={title} onClick={onClick} {...stylex.props(styles.libraryRow)}>
      <span {...stylex.props(styles.libraryIcon)}>{icon}</span>
      <span {...stylex.props(styles.libraryText)}>{text}</span>
    </button>
  )
}

function HealthDot({
  manifest,
  error,
  loading,
  collapsed,
}: {
  manifest?: ViewManifest
  error?: string
  loading: boolean
  collapsed: boolean
}) {
  const label = (text: string, severity: Severity | 'none', title?: string) => (
    <span
      {...stylex.props(styles.health, severity !== 'none' && healthColor[severity])}
      title={title}
    >
      <span {...stylex.props(styles.dot)} />
      <span
        {...stylex.props(styles.sideLabel)}
        style={{ opacity: collapsed ? 0 : 1, transitionDelay: collapsed ? '0ms' : '90ms' }}
      >
        {text}
      </span>
    </span>
  )

  if (loading) return label('connecting', 'none', 'connecting…')
  if (error) return label('daemon down', 'critical', error)
  if (!manifest) return null
  const degraded = Object.entries(manifest.views).filter(([, v]) => v.state !== 'ready')
  const severity: Severity = degraded.length === 0 ? 'low' : degraded.length > 2 ? 'high' : 'medium'
  return label(
    degraded.length === 0 ? 'all views ready' : `${degraded.length} degraded`,
    severity,
    `snapshot ${manifest.snapshotId}`,
  )
}

const healthColor = stylex.create({
  neutral: { color: vars.colorFaint },
  low: { color: vars.scaleLow },
  medium: { color: vars.scaleMedium },
  high: { color: vars.scaleHigh },
  critical: { color: vars.scaleCritical },
})

const styles = stylex.create({
  // ── Integrated shell: topbar + sidebar share one surface (bgShell);
  // the content panel floats inside it on bgBase. No borders between the
  // chrome regions — hierarchy comes from background + radius + spacing.
  shell: {
    height: '100vh',
    backgroundColor: vars.bgShell,
    color: vars.colorBase,
    display: 'flex',
    flexDirection: 'column',
    overflow: 'hidden',
  },
  topbar: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    height: 52,
    paddingLeft: 10,
    paddingRight: 12,
    flexShrink: 0,
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
    whiteSpace: 'nowrap',
    display: { default: 'inline', '@media (max-width: 800px)': 'none' },
  },
  searchTrigger: {
    flex: 1,
    maxWidth: 520,
    minWidth: 0,
    marginLeft: 8,
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    height: 32,
    paddingLeft: 10,
    paddingRight: 8,
    borderRadius: 6,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgSunken,
    color: vars.colorFaint,
    fontFamily: 'inherit',
    fontSize: 12.5,
    cursor: 'text',
    textAlign: 'left',
  },
  searchTriggerText: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    color: vars.colorMuted,
  },
  spacer: {
    flex: 1,
    minWidth: 4,
  },
  snapshot: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: vars.bgSunken,
    borderRadius: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 6,
    paddingRight: 6,
    whiteSpace: 'nowrap',
    display: { default: 'inline-block', '@media (max-width: 900px)': 'none' },
  },
  degraded: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    fontSize: 11,
    color: vars.scaleMedium,
    whiteSpace: 'nowrap',
  },
  degradedTip: {
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  helpKey: {
    fontSize: 12,
    fontWeight: 600,
    fontFamily: font.mono,
  },
  body: {
    flex: 1,
    minHeight: 0,
    display: 'flex',
    paddingRight: 10,
    paddingBottom: 10,
  },
  // Width animates via inline style; inner keeps a constant left padding so
  // icons stay put — labels clip (never squeeze) behind overflow:hidden and
  // only change through opacity.
  sidebar: {
    flexShrink: 0,
    overflow: 'hidden',
    position: 'relative',
    transitionProperty: 'width',
    transitionDuration: '240ms',
    transitionTimingFunction: SPRING,
  },
  // Resize grip — an invisible 6px hit strip hugging the rail's right edge;
  // the only visual affordance is a centered 1px hairline that fades in on
  // hover and stays lit while dragging. Nothing painted at rest.
  resize: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    right: 0,
    width: 6,
    cursor: 'col-resize',
    zIndex: 20,
    display: 'flex',
    justifyContent: 'center',
  },
  resizeLine: {
    width: 1,
    height: '100%',
    backgroundColor: 'transparent',
    transitionProperty: 'background-color',
    transitionDuration: '140ms',
    transitionTimingFunction: 'ease-out',
  },
  resizeLineActive: {
    backgroundColor: vars.colorFaint,
  },
  sideInner: {
    height: '100%',
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
    paddingTop: 4,
    paddingLeft: 8,
    paddingRight: 8,
  },
  // Groups + library scroll independently; the health dot stays pinned.
  sideScroll: {
    flex: 1,
    minHeight: 0,
    overflowY: 'auto',
    overflowX: 'hidden',
    display: 'flex',
    flexDirection: 'column',
    gap: 2,
  },
  sideGroupGap: {
    marginTop: 10,
  },
  sideGroupLabel: {
    fontFamily: font.mono,
    fontSize: 9,
    fontWeight: 600,
    letterSpacing: '0.08em',
    textTransform: 'uppercase',
    color: vars.colorFaint,
    paddingLeft: 10,
    paddingTop: 2,
    paddingBottom: 4,
  },
  // Collapsed rail can't show labels — a hairline keeps group boundaries.
  sideGroupRule: {
    height: 1,
    marginTop: 4,
    marginBottom: 4,
    marginLeft: 8,
    marginRight: 8,
    backgroundColor: vars.borderBase,
  },
  sidePreview: {
    marginLeft: 'auto',
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 8.5,
    letterSpacing: '0.04em',
    textTransform: 'uppercase',
    color: vars.scaleMedium,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.scaleMedium,
    borderRadius: 4,
    paddingLeft: 4,
    paddingRight: 4,
    paddingTop: 1,
    paddingBottom: 1,
    opacity: 0.75,
  },
  libraryRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    height: 26,
    paddingLeft: 12,
    paddingRight: 10,
    borderRadius: 6,
    borderWidth: 0,
    width: '100%',
    overflow: 'hidden',
    whiteSpace: 'nowrap',
    textAlign: 'left',
    cursor: 'pointer',
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorMuted,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  libraryIcon: {
    width: 13,
    flexShrink: 0,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    color: vars.colorFaint,
  },
  libraryText: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
  },
  sideItem: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    height: 32,
    paddingLeft: 10,
    paddingRight: 10,
    borderRadius: 6,
    borderWidth: 0,
    width: '100%',
    overflow: 'hidden',
    whiteSpace: 'nowrap',
    textAlign: 'left',
    cursor: 'pointer',
    fontFamily: 'inherit',
    fontSize: 12.5,
    color: vars.colorMuted,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  sideItemActive: {
    color: vars.colorActive,
    backgroundColor: vars.bgActive,
  },
  sideIcon: {
    width: 16,
    flexShrink: 0,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
  },
  // flex-shrink 0: the label keeps its natural width and clips — it never
  // squeezes while the rail animates. Opacity is the only animated channel.
  sideLabel: {
    flexShrink: 0,
    overflow: 'hidden',
    transitionProperty: 'opacity',
    transitionDuration: '150ms',
    transitionTimingFunction: 'ease-out',
  },
  health: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 10,
    color: vars.colorFaint,
    fontFamily: font.mono,
    whiteSpace: 'nowrap',
    paddingTop: 8,
    paddingBottom: 6,
    paddingLeft: 10,
    overflow: 'hidden',
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: '50%',
    backgroundColor: 'currentColor',
    flexShrink: 0,
  },
  // The inset panel — the only bordered/rounded element; everything around
  // it is the shared shell surface.
  panel: {
    flex: 1,
    minWidth: 0,
    borderRadius: 10,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgBase,
    overflow: 'hidden',
    display: 'flex',
  },
  scroll: {
    flex: 1,
    minWidth: 0,
    overflowY: 'auto',
    overflowX: 'hidden',
    paddingTop: 18,
    paddingBottom: 48,
    paddingLeft: 22,
    paddingRight: 22,
  },
})
