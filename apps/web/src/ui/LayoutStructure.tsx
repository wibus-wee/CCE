import * as stylex from '@stylexjs/stylex'
import { Menubar, ScrollArea, Tabs } from '@base-ui-components/react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useRef, type ReactNode } from 'react'
import { IconCaretLeft, IconCaretRight } from './icons'
import { vars } from './tokens.stylex'

/**
 * Port of `LayoutTabs` on Base UI `Tabs` — an underline-style tab strip.
 * Declarative items or compose `Tabs.List`/`Tabs.Panel` children yourself.
 */
export function LayoutTabs({
  items,
  value,
  onValueChange,
  children,
}: {
  items?: { value: string; label: ReactNode; content?: ReactNode; disabled?: boolean }[]
  value?: string
  onValueChange?: (value: string) => void
  children?: ReactNode
}) {
  return (
    <Tabs.Root
      value={value}
      defaultValue={items?.[0]?.value}
      onValueChange={(v) => onValueChange?.(v as string)}
      className={stylex.props(styles.tabs).className}
    >
      {items ? (
        <>
          <Tabs.List {...stylex.props(styles.tabList)}>
            {items.map((item) => (
              <Tabs.Tab
                key={item.value}
                value={item.value}
                disabled={item.disabled}
                {...stylex.props(styles.tab)}
              >
                {item.label}
              </Tabs.Tab>
            ))}
            <Tabs.Indicator {...stylex.props(styles.tabIndicator)} />
          </Tabs.List>
          {items.map((item) => (
            <Tabs.Panel key={item.value} value={item.value} {...stylex.props(styles.tabPanel)}>
              {item.content}
            </Tabs.Panel>
          ))}
        </>
      ) : (
        children
      )}
    </Tabs.Root>
  )
}

/**
 * Port of `LayoutMenubar` on Base UI `Menubar` — a horizontal menu strip;
 * compose `OverlayDropdown` menus inside.
 */
export function LayoutMenubar({ children }: { children: ReactNode }) {
  return (
    <Menubar className={stylex.props(styles.menubar).className}>{children}</Menubar>
  )
}

/** Port of `LayoutBreadcrumb` — slash-separated path crumbs. */
export function LayoutBreadcrumb({
  items,
}: {
  items: { label: ReactNode; href?: string; onClick?: () => void }[]
}) {
  return (
    <nav aria-label="Breadcrumb" {...stylex.props(styles.crumbs)}>
      {items.map((item, i) => (
        <span key={i} {...stylex.props(styles.crumb)}>
          {i > 0 && <span {...stylex.props(styles.crumbSep)}>/</span>}
          {item.href || item.onClick ? (
            <a
              href={item.href ?? '#'}
              onClick={(e) => {
                if (item.onClick) {
                  e.preventDefault()
                  item.onClick()
                }
              }}
              {...stylex.props(styles.crumbLink)}
            >
              {item.label}
            </a>
          ) : (
            <span {...stylex.props(styles.crumbCurrent)} aria-current="page">
              {item.label}
            </span>
          )}
        </span>
      ))}
    </nav>
  )
}

/** Port of `LayoutPagination` — prev/next + numbered pages with ellipsis. */
export function LayoutPagination({
  page,
  totalPages,
  onChange,
  siblingCount = 1,
}: {
  page: number
  totalPages: number
  onChange: (page: number) => void
  siblingCount?: number
}) {
  const pages = pageList(page, totalPages, siblingCount)
  return (
    <nav aria-label="Pagination" {...stylex.props(styles.pager)}>
      <button
        type="button"
        disabled={page <= 1}
        onClick={() => onChange(page - 1)}
        {...stylex.props(styles.pageBtn)}
      >
        <IconCaretLeft size={12} />
      </button>
      {pages.map((p, i) =>
        p === '…' ? (
          <span key={`e${i}`} {...stylex.props(styles.pageEllipsis)}>…</span>
        ) : (
          <button
            key={p}
            type="button"
            aria-current={p === page ? 'page' : undefined}
            onClick={() => onChange(p as number)}
            {...stylex.props(styles.pageBtn, p === page && styles.pageActive)}
          >
            {p}
          </button>
        ),
      )}
      <button
        type="button"
        disabled={page >= totalPages}
        onClick={() => onChange(page + 1)}
        {...stylex.props(styles.pageBtn)}
      >
        <IconCaretRight size={12} />
      </button>
    </nav>
  )
}

function pageList(page: number, total: number, siblings: number): (number | '…')[] {
  const out: (number | '…')[] = []
  const lo = Math.max(2, page - siblings)
  const hi = Math.min(total - 1, page + siblings)
  out.push(1)
  if (lo > 2) out.push('…')
  for (let p = lo; p <= hi; p++) out.push(p)
  if (hi < total - 1) out.push('…')
  if (total > 1) out.push(total)
  return out
}

/**
 * Port of `LayoutSideNav` — a vertical navigation rail with grouped,
 * icon-bearing links.
 */
export function LayoutSideNav<T extends string>({
  items,
  value,
  onChange,
  header,
}: {
  items: { value: T; label: ReactNode; icon?: ReactNode }[]
  value?: T
  onChange?: (value: T) => void
  header?: ReactNode
}) {
  return (
    <nav {...stylex.props(styles.sideNav)}>
      {header && <div {...stylex.props(styles.sideHeader)}>{header}</div>}
      {items.map((item) => (
        <button
          key={item.value}
          type="button"
          onClick={() => onChange?.(item.value)}
          {...stylex.props(
            styles.sideItem,
            item.value === value && styles.sideItemActive,
          )}
        >
          {item.icon && <span {...stylex.props(styles.sideIcon)}>{item.icon}</span>}
          <span {...stylex.props(styles.sideLabel)}>{item.label}</span>
        </button>
      ))}
    </nav>
  )
}

/**
 * Port of `LayoutDataTable` — a dense, hairline-separated table with
 * typed columns (`key`, `label`, `align`, `render`).
 */
export function LayoutDataTable<T>({
  columns,
  rows,
  rowKey,
  onRowClick,
}: {
  columns: {
    key: string
    label: ReactNode
    align?: 'start' | 'center' | 'end'
    width?: number | string
    render?: (row: T) => ReactNode
  }[]
  rows: T[]
  rowKey: (row: T, index: number) => string
  onRowClick?: (row: T) => void
}) {
  return (
    <div {...stylex.props(styles.tableWrap)}>
      <table {...stylex.props(styles.table)}>
        <thead>
          <tr>
            {columns.map((col) => (
              <th
                key={col.key}
                style={{ width: col.width, textAlign: col.align ?? 'start' }}
                {...stylex.props(styles.th)}
              >
                {col.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr
              key={rowKey(row, i)}
              onClick={onRowClick ? () => onRowClick(row) : undefined}
              {...stylex.props(styles.tr, onRowClick && styles.trClickable)}
            >
              {columns.map((col) => (
                <td key={col.key} style={{ textAlign: col.align ?? 'start' }} {...stylex.props(styles.td)}>
                  {col.render ? col.render(row) : String((row as Record<string, unknown>)[col.key] ?? '')}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/**
 * Port of `LayoutVirtualList` on `@tanstack/react-virtual` — renders a
 * slice of `items` sized by `estimateSize`.
 */
export function LayoutVirtualList<T>({
  items,
  estimateSize = 28,
  height,
  overscan = 8,
  renderItem,
}: {
  items: T[]
  estimateSize?: number
  height: number | string
  overscan?: number
  renderItem: (item: T, index: number) => ReactNode
}) {
  const parentRef = useRef<HTMLDivElement>(null)
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => estimateSize,
    overscan,
  })
  return (
    <div ref={parentRef} {...stylex.props(styles.vlistViewport)} style={{ height }}>
      <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {virtualizer.getVirtualItems().map((vi) => (
          <div
            key={vi.key}
            style={{
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              transform: `translateY(${vi.start}px)`,
            }}
          >
            {renderItem(items[vi.index] as T, vi.index)}
          </div>
        ))}
      </div>
    </div>
  )
}

/**
 * Port of `LayoutScrollArea` on Base UI `ScrollArea` — a scrollable region
 * with a hover-revealed thin scrollbar.
 */
export function LayoutScrollArea({
  height,
  children,
}: {
  height?: number | string
  children: ReactNode
}) {
  return (
    <ScrollArea.Root {...stylex.props(styles.scrollRoot)} style={height != null ? { height } : undefined}>
      <ScrollArea.Viewport {...stylex.props(styles.scrollViewport)}>
        <ScrollArea.Content {...stylex.props(styles.scrollContent)}>{children}</ScrollArea.Content>
      </ScrollArea.Viewport>
      <ScrollArea.Scrollbar orientation="vertical" {...stylex.props(styles.scrollbar)}>
        <ScrollArea.Thumb {...stylex.props(styles.scrollThumb)} />
      </ScrollArea.Scrollbar>
    </ScrollArea.Root>
  )
}

const styles = stylex.create({
  tabs: { display: 'flex', flexDirection: 'column', minWidth: 0 },
  tabList: {
    display: 'flex',
    alignItems: 'center',
    gap: 2,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
    position: 'relative',
  },
  tab: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: 'inherit',
    fontSize: 13,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    paddingRight: 10,
    cursor: 'pointer',
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorBase,
      '[data-selected]': vars.colorActive,
    },
    fontWeight: {
      default: 400,
      '[data-selected]': 500,
    },
    position: 'relative',
  },
  tabIndicator: {
    position: 'absolute',
    bottom: -1,
    height: 2,
    backgroundColor: vars.primary500,
    transitionProperty: 'left, width',
    transitionDuration: '150ms',
  },
  tabPanel: { paddingTop: 12 },
  menubar: {
    display: 'flex',
    alignItems: 'center',
    gap: 2,
  },
  crumbs: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    fontSize: 12,
    minWidth: 0,
    overflow: 'hidden',
  },
  crumb: { display: 'inline-flex', alignItems: 'center', gap: 6, minWidth: 0 },
  crumbSep: { color: vars.colorFaint },
  crumbLink: {
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorActive,
    },
    textDecoration: 'none',
    whiteSpace: 'nowrap',
  },
  crumbCurrent: {
    color: vars.colorBase,
    whiteSpace: 'nowrap',
    overflow: 'hidden',
    textOverflow: 'ellipsis',
  },
  pager: { display: 'flex', alignItems: 'center', gap: 2 },
  pageBtn: {
    minWidth: 26,
    height: 26,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: 6,
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: vars.colorMuted,
    fontSize: 12,
    fontFamily: 'inherit',
    fontVariantNumeric: 'tabular-nums',
    cursor: 'pointer',
    padding: 0,
  },
  pageActive: {
    backgroundColor: vars.bgActive,
    color: vars.colorActive,
    fontWeight: 500,
  },
  pageEllipsis: {
    minWidth: 20,
    textAlign: 'center',
    color: vars.colorFaint,
    fontSize: 12,
  },
  sideNav: {
    display: 'flex',
    flexDirection: 'column',
    gap: 1,
    padding: 6,
  },
  sideHeader: {
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorFaint,
    padding: '6px 8px',
    textTransform: 'uppercase',
    letterSpacing: '0.06em',
  },
  sideItem: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 6,
    paddingBottom: 6,
    paddingLeft: 8,
    paddingRight: 8,
    borderRadius: 6,
    borderWidth: 0,
    backgroundColor: 'transparent',
    color: {
      default: vars.colorMuted,
      ':hover': vars.colorBase,
    },
    fontSize: 13,
    fontFamily: 'inherit',
    cursor: 'pointer',
    textAlign: 'left',
    width: '100%',
  },
  sideItemActive: {
    backgroundColor: vars.bgActive,
    color: vars.colorActive,
    fontWeight: 500,
  },
  sideIcon: { display: 'inline-flex', flexShrink: 0, color: vars.colorFaint },
  sideLabel: { flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
  tableWrap: {
    overflowX: 'auto',
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
  },
  table: {
    width: '100%',
    borderCollapse: 'collapse',
    fontSize: 12,
  },
  th: {
    fontSize: 11,
    fontWeight: 600,
    color: vars.colorFaint,
    padding: '7px 10px',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
    whiteSpace: 'nowrap',
  },
  tr: {},
  trClickable: { cursor: 'pointer' },
  td: {
    padding: '7px 10px',
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    color: vars.colorBase,
    whiteSpace: 'nowrap',
  },
  vlistViewport: {
    overflowY: 'auto',
    position: 'relative',
  },
  scrollRoot: {
    position: 'relative',
    overflow: 'hidden',
  },
  scrollViewport: {
    height: '100%',
    width: '100%',
  },
  scrollContent: {
    minWidth: '100%',
  },
  scrollbar: {
    width: 8,
    padding: 2,
    display: 'flex',
    opacity: {
      default: 0,
      ':hover': 1,
    },
    transitionProperty: 'opacity',
    transitionDuration: '150ms',
  },
  scrollThumb: {
    flex: 1,
    borderRadius: 4,
    backgroundColor: vars.borderBase,
  },
})
