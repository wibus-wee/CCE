import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { FileListEntry, SearchResult, ViewManifest } from '../api'

/**
 * App-wide UI state — the single owner for values that were prop-drilled:
 * the shared query text, overlay visibility, sidebar collapse, the last
 * manifest seen (drives the topbar snapshot chip), and the last search
 * result so unmounting the Query route doesn't force a re-run.
 *
 * Route state itself lives in the URL (React Router): screen, file path,
 * line, executed query — this store holds only what URLs shouldn't.
 */
type AppState = {
  /** Search input text — shared by the topbar trigger, palette, and query bar. */
  query: string
  setQuery: (query: string) => void
  paletteOpen: boolean
  setPaletteOpen: (open: boolean) => void
  shortcutsOpen: boolean
  setShortcutsOpen: (open: boolean) => void
  collapsed: boolean
  toggleSidebar: () => void
  /** User-dragged sidebar width (expanded mode only) — persisted. */
  sidebarWidth: number
  setSidebarWidth: (width: number) => void
  /** Last manifest received — topbar reads it for snapshot + degraded count. */
  manifest?: ViewManifest
  setManifest: (manifest: ViewManifest) => void
  /** Last executed result + the query text that produced it — remounting
   * the Query route restores these instead of re-running the search. */
  result?: SearchResult
  resultFor?: string
  setResult: (result: SearchResult | undefined, forQuery?: string) => void
  /** Canonical link the current page wants `y` to copy — pages without one
   * fall back to `window.location.href`. Set/cleared by mount effects. */
  permalink?: string
  setPermalink: (permalink: string | undefined) => void
  /** Recently viewed files — powers Home's "jump back in" row. */
  recentFiles: { path: string; at: number }[]
  pushRecentFile: (path: string) => void
  /** Snapshot file list — fetched once by Browse, shared with the palette. */
  files?: FileListEntry[]
  setFiles: (files: FileListEntry[]) => void
}

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      query: '',
      setQuery: (query) => set({ query }),
      paletteOpen: false,
      setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
      shortcutsOpen: false,
      setShortcutsOpen: (shortcutsOpen) => set({ shortcutsOpen }),
      collapsed: false,
      toggleSidebar: () => set((s) => ({ collapsed: !s.collapsed })),
      sidebarWidth: 208,
      setSidebarWidth: (sidebarWidth) =>
        set({ sidebarWidth: Math.round(Math.min(360, Math.max(168, sidebarWidth))) }),
      manifest: undefined,
      setManifest: (manifest) => set({ manifest }),
      result: undefined,
      resultFor: undefined,
      setResult: (result, forQuery) => set({ result, resultFor: forQuery }),
      permalink: undefined,
      setPermalink: (permalink) => set({ permalink }),
      recentFiles: [],
      pushRecentFile: (path) =>
        set((s) => ({
          recentFiles: [
            { path, at: Date.now() },
            ...s.recentFiles.filter((f) => f.path !== path),
          ].slice(0, 8),
        })),
      files: undefined,
      setFiles: (files) => set({ files }),
    }),
    {
      name: 'cce-ui',
      partialize: (s) => ({
        collapsed: s.collapsed,
        recentFiles: s.recentFiles,
        sidebarWidth: s.sidebarWidth,
      }),
    },
  ),
)
