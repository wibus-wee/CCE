import * as stylex from '@stylexjs/stylex'
import { Dialog } from '@base-ui-components/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { api } from '../api'
import { useOpenFile, type ScreenId } from '../lib/navigation'
import { useSearchHistory } from '../lib/searchHistory'
import { useAppStore } from '../lib/store'
import { DisplayFileIcon } from '../ui/DisplayFileIcon'
import { DisplayKbd } from '../ui/DisplayKbd'
import {
  IconArrowUpRight,
  IconFile,
  IconHistoryClock,
  IconLayers,
  IconMoon,
  IconRefresh,
  IconSearch,
  IconStar,
} from '../ui/icons'
import { font, vars } from '../ui/tokens.stylex'

export type PaletteScreen = ScreenId

type Item = {
  id: string
  group: 'query' | 'files' | 'recent' | 'saved' | 'go to' | 'action'
  icon: React.ReactNode
  label: string
  hint?: string
  run: () => void
}

const GROUP_ORDER: Item['group'][] = ['query', 'files', 'recent', 'saved', 'go to', 'action']

/**
 * Subsequence fuzzy match over a path — returns a score (lower = better)
 * or Infinity for no match. Basename matches beat deep-path matches.
 */
function fuzzy(needle: string, path: string): number {
  const n = needle.toLowerCase()
  const p = path.toLowerCase()
  const base = p.split('/').pop() ?? p
  let score = p.includes(n) ? (base.includes(n) ? 0 : 40) : Infinity
  if (score === Infinity) {
    let i = 0
    for (const ch of p) {
      if (ch === n[i]) i++
      if (i === n.length) break
    }
    score = i === n.length ? 80 + p.length : Infinity
  }
  return score
}

const SCREENS: { id: PaletteScreen; label: string }[] = [
  { id: 'dashboard', label: 'Home' },
  { id: 'query', label: 'Query' },
  { id: 'browse', label: 'Files' },
  { id: 'symbols', label: 'Symbols' },
  { id: 'commits', label: 'Commits' },
  { id: 'branches', label: 'Branches' },
  { id: 'changes', label: 'Changes' },
  { id: 'map', label: 'Map' },
  { id: 'contexts', label: 'Contexts' },
  { id: 'context', label: 'Packs' },
  { id: 'notebooks', label: 'Notebooks' },
  { id: 'batch', label: 'Batch Changes' },
  { id: 'monitoring', label: 'Monitoring' },
  { id: 'insights', label: 'Insights' },
  { id: 'saved', label: 'Saved searches' },
  { id: 'index', label: 'Index' },
  { id: 'about', label: 'About' },
]

/**
 * ⌘K command palette — the global loop hub. One input runs snapshot queries,
 * jumps between screens, re-runs recent/saved searches, and fires app
 * actions; fully keyboard driven (↑↓ move, ↵ run, Esc close).
 */
export function CommandPalette({
  open,
  onOpenChange,
  initial = '',
  onRunQuery,
  onNavigate,
  onReindex,
  onToggleTheme,
  onToggleSidebar,
  onShowShortcuts,
  dark,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  initial?: string
  onRunQuery: (query: string) => void
  onNavigate: (screen: PaletteScreen) => void
  onReindex: () => void
  onToggleTheme: () => void
  onToggleSidebar: () => void
  onShowShortcuts: () => void
  dark: boolean
}) {
  const [q, setQ] = useState(initial)
  const [active, setActive] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const { recents, saved } = useSearchHistory()
  const openFile = useOpenFile()
  const files = useAppStore((s) => s.files)
  const setFiles = useAppStore((s) => s.setFiles)
  const recentFiles = useAppStore((s) => s.recentFiles)

  useEffect(() => {
    if (open) {
      setQ(initial)
      setActive(0)
      requestAnimationFrame(() => inputRef.current?.select())
      // Warm the shared file list once — Browse reuses it.
      if (!files) {
        api
          .files()
          .then((r) => setFiles(r.files))
          .catch(() => {})
      }
    }
  }, [open, initial, files, setFiles])

  const items = useMemo<Item[]>(() => {
    const needle = q.trim().toLowerCase()
    const match = (s: string) => !needle || s.toLowerCase().includes(needle)
    const list: Item[] = []

    if (needle) {
      list.push({
        id: 'run',
        group: 'query',
        icon: <IconSearch size={13} />,
        label: `Search snapshot for “${q.trim()}”`,
        hint: '↵',
        run: () => onRunQuery(q.trim()),
      })
      // Fuzzy file jump — Sourcegraph's palette Files tab, folded in.
      const scored = (files ?? [])
        .map((f) => ({ f, score: fuzzy(q.trim(), f.path) }))
        .filter((x) => x.score < Infinity)
        .sort((a, b) => a.score - b.score || a.f.path.localeCompare(b.f.path))
      for (const { f } of scored.slice(0, 6)) {
        list.push({
          id: `file:${f.path}`,
          group: 'files',
          icon: <DisplayFileIcon path={f.path} />,
          label: f.path,
          hint: 'file',
          run: () => openFile(f.path),
        })
      }
    } else {
      // Empty query → recently viewed files, the palette's "recently
      // navigated" section (SG 7.1).
      for (const f of recentFiles.slice(0, 4)) {
        list.push({
          id: `recent-file:${f.path}`,
          group: 'files',
          icon: <IconFile size={13} />,
          label: f.path,
          hint: 'recent file',
          run: () => openFile(f.path),
        })
      }
    }
    for (const r of recents.filter(match).slice(0, 4)) {
      list.push({
        id: `recent:${r}`,
        group: 'recent',
        icon: <IconHistoryClock size={13} />,
        label: r,
        hint: 'recent',
        run: () => onRunQuery(r),
      })
    }
    for (const s of saved.filter(match).slice(0, 3)) {
      list.push({
        id: `saved:${s}`,
        group: 'saved',
        icon: <IconStar size={13} filled />,
        label: s,
        hint: 'saved',
        run: () => onRunQuery(s),
      })
    }
    for (const s of SCREENS.filter((s) => match(s.label))) {
      list.push({
        id: `go:${s.id}`,
        group: 'go to',
        icon: <IconArrowUpRight size={13} />,
        label: `Go to ${s.label}`,
        run: () => onNavigate(s.id),
      })
    }
    const actions: Item[] = [
      {
        id: 'a:reindex',
        group: 'action',
        icon: <IconRefresh size={13} />,
        label: 'Reindex snapshot',
        run: onReindex,
      },
      {
        id: 'a:theme',
        group: 'action',
        icon: <IconMoon size={13} />,
        label: dark ? 'Switch to light mode' : 'Switch to dark mode',
        run: onToggleTheme,
      },
      {
        id: 'a:sidebar',
        group: 'action',
        icon: <IconLayers size={13} />,
        label: 'Toggle sidebar',
        run: onToggleSidebar,
      },
      {
        id: 'a:shortcuts',
        group: 'action',
        icon: <DisplayKbd keys="?" size="sm" />,
        label: 'Keyboard shortcuts',
        run: onShowShortcuts,
      },
    ]
    for (const a of actions.filter((a) => match(a.label))) list.push(a)
    return list
  }, [q, recents, saved, files, recentFiles, openFile, onRunQuery, onNavigate, onReindex, onToggleTheme, onToggleSidebar, onShowShortcuts, dark])

  useEffect(() => {
    setActive(0)
  }, [q])

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-idx="${active}"]`)
      ?.scrollIntoView({ block: 'nearest' })
  }, [active])

  function pick(i: number) {
    const item = items[i]
    if (!item) return
    item.run()
    onOpenChange(false)
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setActive((a) => Math.min(a + 1, items.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setActive((a) => Math.max(a - 1, 0))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      pick(active)
    }
  }

  let flatIdx = -1

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop {...stylex.props(styles.backdrop)} />
        <Dialog.Popup {...stylex.props(styles.popup)} onKeyDown={onKeyDown}>
          <div {...stylex.props(styles.inputRow)}>
            <IconSearch size={15} />
            <input
              ref={inputRef}
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="Search snapshot, go to a screen, run an action…"
              aria-label="Command palette"
              {...stylex.props(styles.input)}
            />
            <DisplayKbd keys="Escape" size="sm" />
          </div>
          <div ref={listRef} {...stylex.props(styles.list)} role="listbox">
            {items.length === 0 && (
              <p {...stylex.props(styles.empty)}>nothing matches — type a query to search the snapshot</p>
            )}
            {GROUP_ORDER.map((group) => {
              const groupItems = items.filter((i) => i.group === group)
              if (groupItems.length === 0) return null
              return (
                <div key={group}>
                  <div {...stylex.props(styles.groupLabel)}>{group}</div>
                  {groupItems.map((item) => {
                    flatIdx += 1
                    const idx = flatIdx
                    return (
                      <button
                        key={item.id}
                        type="button"
                        role="option"
                        aria-selected={idx === active}
                        data-idx={idx}
                        onClick={() => pick(idx)}
                        onMouseMove={() => setActive(idx)}
                        {...stylex.props(styles.item, idx === active && styles.itemActive)}
                      >
                        <span {...stylex.props(styles.itemIcon)}>{item.icon}</span>
                        <span {...stylex.props(styles.itemLabel)}>{item.label}</span>
                        {item.hint && <span {...stylex.props(styles.itemHint)}>{item.hint}</span>}
                      </button>
                    )
                  })}
                </div>
              )
            })}
          </div>
          <div {...stylex.props(styles.foot)}>
            <span>
              <DisplayKbd keys="ArrowUp" size="sm" /> <DisplayKbd keys="ArrowDown" size="sm" /> move
            </span>
            <span>
              <DisplayKbd keys="Enter" size="sm" /> run
            </span>
            <span>
              <DisplayKbd keys="Escape" size="sm" /> close
            </span>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

const styles = stylex.create({
  backdrop: {
    position: 'fixed',
    inset: 0,
    backgroundColor: 'rgba(0, 0, 0, 0.32)',
    zIndex: 60,
    animationName: stylex.keyframes({ from: { opacity: 0 }, to: { opacity: 1 } }),
    animationDuration: '120ms',
  },
  popup: {
    position: 'fixed',
    top: '14vh',
    left: '50%',
    transform: 'translateX(-50%)',
    width: 560,
    maxWidth: 'calc(100vw - 40px)',
    zIndex: 61,
    borderRadius: 12,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgOverlay,
    boxShadow: vars.shadowOverlay,
    overflow: 'hidden',
    display: 'flex',
    flexDirection: 'column',
    animationName: stylex.keyframes({
      from: { opacity: 0, transform: 'translateX(-50%) translateY(-8px) scale(0.98)' },
      to: { opacity: 1, transform: 'translateX(-50%) translateY(0) scale(1)' },
    }),
    animationDuration: '160ms',
    animationTimingFunction: 'cubic-bezier(0.22, 1, 0.36, 1)',
  },
  inputRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 13,
    paddingBottom: 13,
    paddingLeft: 14,
    paddingRight: 12,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    color: vars.colorFaint,
  },
  input: {
    flex: 1,
    minWidth: 0,
    borderWidth: 0,
    padding: 0,
    backgroundColor: 'transparent',
    color: vars.colorBase,
    fontSize: 14.5,
    fontFamily: 'inherit',
    outline: 'none',
    '::placeholder': { color: vars.colorFaint },
  },
  list: {
    maxHeight: 340,
    overflowY: 'auto',
    padding: 6,
  },
  groupLabel: {
    fontSize: 9.5,
    color: vars.colorFaint,
    letterSpacing: '0.1em',
    textTransform: 'uppercase',
    paddingTop: 8,
    paddingBottom: 4,
    paddingLeft: 10,
    paddingRight: 10,
  },
  item: {
    display: 'flex',
    alignItems: 'center',
    gap: 9,
    width: '100%',
    paddingTop: 7,
    paddingBottom: 7,
    paddingLeft: 10,
    paddingRight: 10,
    borderWidth: 0,
    borderRadius: 7,
    fontFamily: 'inherit',
    fontSize: 12.5,
    color: vars.colorBase,
    backgroundColor: 'transparent',
    cursor: 'pointer',
    textAlign: 'left',
  },
  itemActive: {
    backgroundColor: vars.bgActive,
  },
  itemIcon: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
    width: 16,
    justifyContent: 'center',
  },
  itemLabel: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  itemHint: {
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  empty: {
    margin: 0,
    padding: '18px 12px',
    fontSize: 12,
    color: vars.colorFaint,
    textAlign: 'center',
  },
  foot: {
    display: 'flex',
    alignItems: 'center',
    gap: 16,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 14,
    paddingRight: 14,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontFamily: font.mono,
    fontSize: 9.5,
    color: vars.colorFaint,
  },
})
