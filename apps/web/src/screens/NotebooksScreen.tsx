import * as stylex from '@stylexjs/stylex'
import {
  CaretDown,
  CaretUp,
  FileText,
  Function as FunctionIcon,
  GlobeSimple,
  LockSimple,
  MagnifyingGlass,
  MarkdownLogo,
  PencilSimple,
  Play,
  Plus,
  X,
} from '@phosphor-icons/react'
import { Fragment, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useNavigate } from 'react-router'
import { fileUrl, queryUrl } from '../lib/navigation'
import {
  MOCK_NOTEBOOKS,
  type MockNotebook,
  type MockNotebookBlock,
  type MockNotebookBlockType,
  type MockNotebookFileBlock,
  type MockNotebookHit,
  type MockNotebookMarkdownBlock,
  type MockNotebookQueryBlock,
  type MockNotebookSymbolBlock,
} from '../mock/notebooks'
import { ActionButton } from '../ui/ActionButton'
import { ActionIconButton } from '../ui/ActionIconButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { CopyButton } from '../ui/CopyButton'
import { DisplayTimeAgo } from '../ui/DisplayNumber'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { useNotification } from '../ui/FeedbackToasts'
import { FormSearchField } from '../ui/FormSearchField'
import { IconDownload, IconNotebook, IconPlus, IconStar } from '../ui/icons'
import { LayoutBreadcrumb } from '../ui/LayoutStructure'
import { OverlayDropdown, OverlayDropdownItem } from '../ui/OverlayMenu'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Notebooks — Sourcegraph's living-documentation surface (prose + query +
 * file + symbol blocks in one document), as a master/detail screen: the
 * left rail lists notebooks with search/stars/owner/updated and a
 * block-type summary; the right pane renders the selected document block
 * by block with a sticky outline rail, per-block move/delete actions,
 * and `+` insert rows. MOCK: no `/v1/notebooks` endpoint; fixtures come
 * from `src/mock/notebooks` and the screen is labeled `preview` — never
 * index truth. Block adds/moves/removals are local state only. Query-block
 * Run buttons re-issue the query for real; export downloads a markdown
 * serialization of the document.
 */
export function NotebooksScreen() {
  const [text, setText] = useState('')
  const [selectedId, setSelectedId] = useState<string | undefined>(MOCK_NOTEBOOKS[0]?.id)
  // Viewer-star overrides — the fixture's `starred` flag is the seed;
  // toggling is a local mock of starring, never a write.
  const [starOverrides, setStarOverrides] = useState<Record<string, boolean>>({})

  const rows = useMemo(() => {
    const needle = text.trim().toLowerCase()
    return MOCK_NOTEBOOKS.filter(
      (n) =>
        !needle ||
        `${n.title} ${n.description} ${n.owner} ${n.namespace}`
          .toLowerCase()
          .includes(needle),
    ).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  }, [text])

  const selected = MOCK_NOTEBOOKS.find((n) => n.id === selectedId)
  const starred = (n: MockNotebook) => starOverrides[n.id] ?? n.starred
  const starCount = (n: MockNotebook) =>
    n.stars + (starred(n) === n.starred ? 0 : starred(n) ? 1 : -1)
  const toggleStar = (n: MockNotebook) =>
    setStarOverrides((o) => ({ ...o, [n.id]: !starred(n) }))

  return (
    <div {...stylex.props(styles.root)}>
      <header {...stylex.props(styles.head)}>
        <h2 {...stylex.props(styles.title)}>Notebooks</h2>
        <DisplayBadge severity="medium">preview</DisplayBadge>
        <span {...stylex.props(styles.count)}>{rows.length === MOCK_NOTEBOOKS.length ? `${rows.length} notebooks` : `${rows.length} of ${MOCK_NOTEBOOKS.length} notebooks`}</span>
        <span {...stylex.props(styles.spacer)} />
        <ActionButton
          size="sm"
          icon={<IconPlus size={12} />}
          disabled
          title="endpoint pending — /v1/notebooks"
        >
          New notebook
        </ActionButton>
        <div {...stylex.props(styles.search)}>
          <FormSearchField
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="title, owner, namespace"
            onClear={() => setText('')}
            aria-label="Filter notebooks"
          />
        </div>
      </header>

      <div {...stylex.props(styles.body)}>
        <div {...stylex.props(styles.list)}>
          {rows.map((n) => (
            <NotebookRow
              key={n.id}
              notebook={n}
              active={n.id === selectedId}
              starred={starred(n)}
              stars={starCount(n)}
              onSelect={() => setSelectedId(n.id)}
            />
          ))}
          {rows.length === 0 && (
            <FeedbackEmptyState
              title="No notebooks match"
              description="The fixture set is small — loosen the filter."
            />
          )}
        </div>
        <div {...stylex.props(styles.detail)}>
          {selected ? (
            <NotebookDetail
              key={selected.id}
              notebook={selected}
              starred={starred(selected)}
              stars={starCount(selected)}
              onToggleStar={() => toggleStar(selected)}
              onBack={() => setSelectedId(undefined)}
            />
          ) : (
            <FeedbackEmptyState
              icon={<IconNotebook size={20} />}
              title="Select a notebook"
              description="Blocks render here — markdown, live queries, file ranges, and symbols."
            />
          )}
        </div>
      </div>
      <p {...stylex.props(styles.note)}>
        Fixture data — block contents, hits, and file excerpts are stored fixtures,
        not snapshot reads. A real notebook would re-run query blocks against the
        current snapshot; Run re-issues the query live. Block adds, moves, and
        deletes are local state only — nothing is written back. Export downloads
        the document as markdown.
      </p>
    </div>
  )
}

const BLOCK_ORDER: MockNotebookBlockType[] = ['markdown', 'query', 'file', 'symbol']
const BLOCK_SHORT: Record<MockNotebookBlockType, string> = {
  markdown: 'md',
  query: 'q',
  file: 'f',
  symbol: 's',
}

/** Compact block-type summary — `2md·2q·1f·1s`, types absent are omitted. */
function blockSummary(blocks: MockNotebookBlock[]): string {
  const counts = new Map<MockNotebookBlockType, number>()
  for (const b of blocks) counts.set(b.type, (counts.get(b.type) ?? 0) + 1)
  return BLOCK_ORDER.filter((t) => counts.get(t))
    .map((t) => `${counts.get(t)}${BLOCK_SHORT[t]}`)
    .join('·')
}

function NotebookRow({
  notebook: n,
  active,
  starred,
  stars,
  onSelect,
}: {
  notebook: MockNotebook
  active: boolean
  starred: boolean
  stars: number
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      {...stylex.props(styles.row, active && styles.rowActive)}
    >
      <span {...stylex.props(styles.rowTitleLine)}>
        {n.visibility === 'private' && (
          <span {...stylex.props(styles.rowLock)}>
            <LockSimple size={11} />
          </span>
        )}
        <span {...stylex.props(styles.rowTitle)}>{n.title}</span>
      </span>
      <span {...stylex.props(styles.rowDesc)}>{n.description}</span>
      <span {...stylex.props(styles.rowMeta)}>
        <span {...stylex.props(styles.stars)}>
          <IconStar size={11} filled={starred} /> {stars}
        </span>
        <span {...stylex.props(styles.rowOwner)}>{n.owner}</span>
        <code {...stylex.props(styles.chips)}>{blockSummary(n.blocks)}</code>
        <span {...stylex.props(styles.rowMetaSpacer)} />
        <DisplayTimeAgo value={n.updatedAt} />
      </span>
    </button>
  )
}

function NotebookDetail({
  notebook: n,
  starred,
  stars,
  onToggleStar,
  onBack,
}: {
  notebook: MockNotebook
  starred: boolean
  stars: number
  onToggleStar: () => void
  onBack: () => void
}) {
  const notify = useNotification()
  // Local-only block list — adds, moves, removals, and content edits update
  // this state; the fixture is the seed and nothing is written back (see the
  // footnote).
  const [localBlocks, setLocalBlocks] = useState<MockNotebookBlock[] | null>(null)
  const blocks = localBlocks ?? n.blocks

  const move = (index: number, dir: -1 | 1) => {
    const next = [...blocks]
    const j = index + dir
    if (j < 0 || j >= next.length) return
    const [b] = next.splice(index, 1)
    if (!b) return
    next.splice(j, 0, b)
    setLocalBlocks(next)
  }
  const remove = (index: number) => setLocalBlocks(blocks.filter((_, i) => i !== index))
  const insert = (index: number, type: MockNotebookBlockType) => {
    const next = [...blocks]
    next.splice(index, 0, stubBlock(type))
    setLocalBlocks(next)
  }
  const patch = (index: number, p: Partial<MockNotebookBlock>) =>
    setLocalBlocks(
      blocks.map((b, i) => (i === index ? ({ ...b, ...p } as MockNotebookBlock) : b)),
    )

  const exportMarkdown = () => {
    const blob = new Blob([notebookToMarkdown(n, blocks)], { type: 'text/markdown' })
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = `${n.id}.md`
    a.click()
    URL.revokeObjectURL(a.href)
    notify.push('Notebook exported', { type: 'success', description: a.download })
  }

  return (
    <div {...stylex.props(styles.docLayout)}>
      <article {...stylex.props(styles.docCol)}>
        <LayoutBreadcrumb
          items={[{ label: 'Notebooks', onClick: onBack }, { label: n.title }]}
        />
        <header {...stylex.props(styles.docHead)}>
          <div {...stylex.props(styles.docTitleWrap)}>
            <h3 {...stylex.props(styles.docTitle)}>{n.title}</h3>
            <p {...stylex.props(styles.docDesc)}>{n.description}</p>
          </div>
          <CopyButton
            text={`${window.location.origin}/notebooks#${n.id}`}
            title="Copy link"
            label="Copy notebook link"
            size={13}
          />
          <ActionIconButton
            icon={<IconDownload size={13} />}
            tooltip="Export as markdown"
            label="Export notebook as markdown"
            onClick={exportMarkdown}
          />
          <ActionButton
            size="sm"
            icon={<IconStar size={13} filled={starred} />}
            onClick={onToggleStar}
            aria-pressed={starred}
          >
            {stars}
          </ActionButton>
        </header>
        <div {...stylex.props(styles.docMeta)}>
          <DisplayBadge text={n.namespace} />
          <DisplayBadge
            color={false}
            icon={
              n.visibility === 'private' ? (
                <LockSimple size={11} />
              ) : (
                <GlobeSimple size={11} />
              )
            }
          >
            {n.visibility}
          </DisplayBadge>
          <span {...stylex.props(styles.docMetaItem)}>owner {n.owner}</span>
          <span {...stylex.props(styles.docMetaItem)}>{blocks.length} blocks</span>
          <span {...stylex.props(styles.docMetaItem)}>{wordCount(blocks)} words</span>
          <span {...stylex.props(styles.docMetaItem)}>
            updated <DisplayTimeAgo value={n.updatedAt} />
          </span>
          <span {...stylex.props(styles.docMetaItem)}>
            created <DisplayTimeAgo value={n.createdAt} />
          </span>
        </div>
        <div>
          {blocks.map((block, i) => (
            <Fragment key={block.id}>
              <BlockView
                block={block}
                index={i}
                total={blocks.length}
                onMove={(dir) => move(i, dir)}
                onRemove={() => remove(i)}
                onPatch={(p) => patch(i, p)}
              />
              <AddBlockRow onAdd={(type) => insert(i + 1, type)} />
            </Fragment>
          ))}
          {blocks.length === 0 && <AddBlockRow onAdd={(type) => insert(0, type)} />}
        </div>
      </article>
      <BlockOutline blocks={blocks} />
    </div>
  )
}

const BLOCK_ICON: Record<MockNotebookBlockType, ReactNode> = {
  markdown: <MarkdownLogo size={13} />,
  query: <MagnifyingGlass size={13} />,
  file: <FileText size={13} />,
  symbol: <FunctionIcon size={13} />,
}

/**
 * The outline rail — a sticky third column listing every block (kind icon,
 * title preview, index). Clicking scrolls the block into view; the entry
 * nearest the top of the viewport is highlighted.
 */
function BlockOutline({ blocks }: { blocks: MockNotebookBlock[] }) {
  const [activeId, setActiveId] = useState<string>()
  useEffect(() => {
    const els = blocks
      .map((b) => document.getElementById(blockDomId(b.id)))
      .filter((el): el is HTMLElement => el != null)
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)
        const top = visible[0]
        if (top) setActiveId(top.target.id)
      },
      { rootMargin: '-15% 0px -70% 0px' },
    )
    for (const el of els) observer.observe(el)
    return () => observer.disconnect()
  }, [blocks])

  if (blocks.length === 0) return null
  return (
    <aside {...stylex.props(styles.outline)}>
      {blocks.map((b, i) => (
        <button
          key={b.id}
          type="button"
          {...stylex.props(
            styles.outlineRow,
            blockDomId(b.id) === activeId && styles.outlineRowActive,
          )}
          onClick={() =>
            document.getElementById(blockDomId(b.id))?.scrollIntoView({ block: 'start' })
          }
        >
          <span {...stylex.props(styles.outlineIcon)}>{BLOCK_ICON[b.type]}</span>
          <span {...stylex.props(styles.outlineText)}>{blockTitle(b)}</span>
          <span {...stylex.props(styles.outlineIdx)}>{i + 1}</span>
        </button>
      ))}
    </aside>
  )
}

/** One-line preview a block gets in the outline rail. */
function blockTitle(block: MockNotebookBlock): string {
  switch (block.type) {
    case 'markdown': {
      const line = block.markdown.split('\n').find((l) => l.trim())
      return line?.replace(/^#+\s*/, '').replace(/[*`~]/g, '').trim() || 'markdown'
    }
    case 'query':
      return block.query || 'empty query'
    case 'file':
      return block.path
    case 'symbol':
      return block.name
  }
}

const blockDomId = (blockId: string) => `nb-${blockId}`

/** A blank block for the add-row — `nb-local-*` ids mark mock-only inserts. */
function stubBlock(type: MockNotebookBlockType): MockNotebookBlock {
  const id = `nb-local-${Math.random().toString(36).slice(2, 8)}`
  switch (type) {
    case 'markdown':
      return { id, type, markdown: '## New section\n\nWrite here…' }
    case 'query':
      return { id, type, query: '', hits: [] }
    case 'file':
      return { id, type, path: '', lineRange: null, content: '' }
    case 'symbol':
      return { id, type, name: '', kind: 'function', path: '', line: 0, signature: '' }
  }
}

/**
 * One notebook cell — a kind header with hover-revealed actions over the
 * block body. Markdown cells swap their preview for an inline source editor;
 * the other kinds keep their structured card and edit their head fields in
 * place.
 */
function BlockView({
  block,
  index,
  total,
  onMove,
  onRemove,
  onPatch,
}: {
  block: MockNotebookBlock
  index: number
  total: number
  onMove: (dir: -1 | 1) => void
  onRemove: () => void
  onPatch: (p: Partial<MockNotebookBlock>) => void
}) {
  const [editing, setEditing] = useState(false)
  const [hover, setHover] = useState(false)
  return (
    <section
      id={blockDomId(block.id)}
      {...stylex.props(styles.block)}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      onFocusCapture={() => setHover(true)}
      onBlurCapture={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node)) setHover(false)
      }}
    >
      <div {...stylex.props(styles.blockHead)}>
        {block.type === 'markdown' && (
          <>
            <span {...stylex.props(styles.blockKind)}>markdown</span>
            <span {...stylex.props(styles.headSpacer)} />
          </>
        )}
        {block.type === 'query' && <QueryBlockHead block={block} onPatch={onPatch} />}
        {block.type === 'file' && <FileBlockHead block={block} onPatch={onPatch} />}
        {block.type === 'symbol' && <SymbolBlockHead block={block} onPatch={onPatch} />}
        <BlockActions
          index={index}
          total={total}
          revealed={hover}
          onMove={onMove}
          onRemove={onRemove}
          editing={block.type === 'markdown' ? editing : undefined}
          onToggleEdit={
            block.type === 'markdown' ? () => setEditing((v) => !v) : undefined
          }
        />
      </div>
      {block.type === 'markdown' &&
        (editing ? (
          <MarkdownEditor
            block={block}
            onPatch={onPatch}
            onDone={() => setEditing(false)}
          />
        ) : (
          <MarkdownBody md={block.markdown} />
        ))}
      {block.type === 'query' && <QueryBlockBody block={block} />}
      {block.type === 'file' && <FileBlockBody block={block} />}
      {block.type === 'symbol' && <SymbolBlockBody block={block} />}
    </section>
  )
}

/** Per-cell chrome — pencil on markdown cells, then move / remove. */
function BlockActions({
  index,
  total,
  revealed,
  onMove,
  onRemove,
  editing,
  onToggleEdit,
}: {
  index: number
  total: number
  revealed: boolean
  onMove: (dir: -1 | 1) => void
  onRemove: () => void
  editing?: boolean
  onToggleEdit?: () => void
}) {
  return (
    <span {...stylex.props(styles.actions, revealed && styles.actionsRevealed)}>
      {onToggleEdit && (
        <button
          type="button"
          {...stylex.props(styles.actionBtn, editing && styles.actionBtnActive)}
          onClick={onToggleEdit}
          title={editing ? 'Cancel editing' : 'Edit markdown'}
          aria-pressed={editing}
        >
          <PencilSimple size={12} />
        </button>
      )}
      <button
        type="button"
        {...stylex.props(styles.actionBtn)}
        onClick={() => onMove(-1)}
        disabled={index === 0}
        title="Move up"
      >
        <CaretUp size={12} />
      </button>
      <button
        type="button"
        {...stylex.props(styles.actionBtn)}
        onClick={() => onMove(1)}
        disabled={index === total - 1}
        title="Move down"
      >
        <CaretDown size={12} />
      </button>
      <button
        type="button"
        {...stylex.props(styles.actionBtn)}
        onClick={onRemove}
        title="Remove block"
      >
        <X size={12} />
      </button>
    </span>
  )
}

/** The dashed `+` divider between cells — opens the block-kind menu. */
function AddBlockRow({ onAdd }: { onAdd: (type: MockNotebookBlockType) => void }) {
  return (
    <div {...stylex.props(styles.addRow)}>
      <OverlayDropdown
        trigger={
          <button type="button" {...stylex.props(styles.addTrigger)}>
            <Plus size={11} /> add block
          </button>
        }
      >
        <OverlayDropdownItem
          icon={<MarkdownLogo size={13} />}
          onClick={() => onAdd('markdown')}
        >
          Markdown
        </OverlayDropdownItem>
        <OverlayDropdownItem
          icon={<MagnifyingGlass size={13} />}
          onClick={() => onAdd('query')}
        >
          Query
        </OverlayDropdownItem>
        <OverlayDropdownItem icon={<FileText size={13} />} onClick={() => onAdd('file')}>
          File
        </OverlayDropdownItem>
        <OverlayDropdownItem
          icon={<FunctionIcon size={13} />}
          onClick={() => onAdd('symbol')}
        >
          Symbol
        </OverlayDropdownItem>
      </OverlayDropdown>
    </div>
  )
}

/** Query cell head — editable query text, stored hit count, run-through. */
function QueryBlockHead({
  block,
  onPatch,
}: {
  block: MockNotebookQueryBlock
  onPatch: (p: Partial<MockNotebookQueryBlock>) => void
}) {
  const navigate = useNavigate()
  return (
    <>
      <span {...stylex.props(styles.blockKind)}>query</span>
      <input
        value={block.query}
        onChange={(e) => onPatch({ query: e.target.value })}
        placeholder="kind:fn snapshot"
        spellCheck={false}
        {...stylex.props(styles.headInput)}
      />
      <span {...stylex.props(styles.blockNote)}>{block.hits.length} hits</span>
      <button
        type="button"
        {...stylex.props(styles.headButton)}
        onClick={() => navigate(queryUrl(block.query))}
      >
        <Play size={10} weight="fill" /> Run
      </button>
    </>
  )
}

/** Query cell body — the stored hit set, each row routes to the file. */
function QueryBlockBody({ block }: { block: MockNotebookQueryBlock }) {
  const navigate = useNavigate()
  if (block.hits.length === 0) {
    return <div {...stylex.props(styles.blockEmpty)}>no stored hits</div>
  }
  return (
    <div {...stylex.props(styles.hitList)}>
      {block.hits.map((h: MockNotebookHit, i) => (
        <button
          key={i}
          type="button"
          {...stylex.props(styles.hitRow)}
          onClick={() => navigate(fileUrl(h.path, h.line))}
        >
          <span {...stylex.props(styles.hitPath)}>
            {h.path}:{h.line}
          </span>
          <span {...stylex.props(styles.hitPreview)}>{h.preview}</span>
          <DisplayBadge text={h.route} />
        </button>
      ))}
    </div>
  )
}

/** File cell head — editable path + line range, open-through. */
function FileBlockHead({
  block,
  onPatch,
}: {
  block: MockNotebookFileBlock
  onPatch: (p: Partial<MockNotebookFileBlock>) => void
}) {
  const navigate = useNavigate()
  const setRange = (edge: 0 | 1, raw: string) => {
    const n = parseInt(raw, 10)
    const cur: [number, number] = [
      block.lineRange?.[0] ?? 1,
      block.lineRange?.[1] ?? 1,
    ]
    cur[edge] = Number.isFinite(n) ? n : 1
    onPatch({ lineRange: raw === '' ? null : cur })
  }
  return (
    <>
      <span {...stylex.props(styles.blockKind)}>file</span>
      <input
        value={block.path}
        onChange={(e) => onPatch({ path: e.target.value })}
        placeholder="path/to/file.rs"
        spellCheck={false}
        {...stylex.props(styles.headInput)}
      />
      <span {...stylex.props(styles.rangeWrap)}>
        L
        <input
          value={block.lineRange?.[0] ?? ''}
          onChange={(e) => setRange(0, e.target.value)}
          placeholder="1"
          inputMode="numeric"
          {...stylex.props(styles.numInput)}
        />
        –
        <input
          value={block.lineRange?.[1] ?? ''}
          onChange={(e) => setRange(1, e.target.value)}
          placeholder="∞"
          inputMode="numeric"
          {...stylex.props(styles.numInput)}
        />
      </span>
      <button
        type="button"
        {...stylex.props(styles.headButton)}
        onClick={() => navigate(fileUrl(block.path, block.lineRange ?? undefined))}
      >
        <Play size={10} weight="fill" /> Open
      </button>
    </>
  )
}

/** File cell body — the frozen excerpt with gutter line numbers. */
function FileBlockBody({ block }: { block: MockNotebookFileBlock }) {
  if (!block.content) {
    return <div {...stylex.props(styles.blockEmpty)}>no stored excerpt</div>
  }
  const start = block.lineRange?.[0] ?? 1
  const lines = block.content.split('\n')
  return (
    <div {...stylex.props(styles.code)}>
      {lines.map((l, i) => (
        <div key={i} {...stylex.props(styles.codeLine)}>
          <span {...stylex.props(styles.gutter)}>{start + i}</span>
          <span {...stylex.props(styles.codeText)}>{l || ' '}</span>
        </div>
      ))}
    </div>
  )
}

/** Symbol cell head — editable name, kind badge, source link. */
function SymbolBlockHead({
  block,
  onPatch,
}: {
  block: MockNotebookSymbolBlock
  onPatch: (p: Partial<MockNotebookSymbolBlock>) => void
}) {
  const navigate = useNavigate()
  return (
    <>
      <span {...stylex.props(styles.blockKind)}>symbol</span>
      <input
        value={block.name}
        onChange={(e) => onPatch({ name: e.target.value })}
        placeholder="SymbolName"
        spellCheck={false}
        {...stylex.props(styles.headInput, styles.headInputName)}
      />
      <DisplayBadge text={block.kind} />
      <button
        type="button"
        {...stylex.props(styles.headButton)}
        onClick={() => navigate(fileUrl(block.path, block.line))}
      >
        {block.path}:{block.line}
      </button>
    </>
  )
}

/** Symbol cell body — the declaration signature card plus doc note. */
function SymbolBlockBody({ block }: { block: MockNotebookSymbolBlock }) {
  return (
    <div>
      <div {...stylex.props(styles.code)}>
        <div {...stylex.props(styles.codeLine)}>
          <span {...stylex.props(styles.codeText)}>{block.signature || ' '}</span>
        </div>
      </div>
      {block.doc && <p {...stylex.props(styles.symDoc)}>{block.doc}</p>}
    </div>
  )
}

/**
 * Inline markdown source editor — a plain textarea that grows with its
 * content. `⌘⏎` saves, `Esc` cancels, `Tab` indents.
 */
function MarkdownEditor({
  block,
  onPatch,
  onDone,
}: {
  block: MockNotebookMarkdownBlock
  onPatch: (p: Partial<MockNotebookMarkdownBlock>) => void
  onDone: () => void
}) {
  const [draft, setDraft] = useState(block.markdown)
  const ref = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    const el = ref.current
    if (el) {
      el.style.height = '0px'
      el.style.height = `${el.scrollHeight}px`
    }
  }, [draft])
  useEffect(() => {
    const el = ref.current
    if (el) {
      el.focus()
      el.setSelectionRange(el.value.length, el.value.length)
    }
  }, [])
  const save = () => {
    onPatch({ markdown: draft })
    onDone()
  }
  return (
    <div>
      <textarea
        ref={ref}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
            e.preventDefault()
            save()
          } else if (e.key === 'Escape') {
            e.preventDefault()
            onDone()
          } else if (e.key === 'Tab') {
            e.preventDefault()
            const el = e.currentTarget
            const s = el.selectionStart
            setDraft(`${draft.slice(0, s)}  ${draft.slice(el.selectionEnd)}`)
            requestAnimationFrame(() => el.setSelectionRange(s + 2, s + 2))
          }
        }}
        spellCheck={false}
        {...stylex.props(styles.mdEditArea)}
      />
      <div {...stylex.props(styles.mdEditBar)}>
        <span {...stylex.props(styles.mdEditHint)}>⌘⏎ save · esc cancel</span>
        <span {...stylex.props(styles.headSpacer)} />
        <button type="button" {...stylex.props(styles.actionBtn)} onClick={onDone}>
          cancel
        </button>
        <ActionButton size="sm" onClick={save}>
          Save
        </ActionButton>
      </div>
    </div>
  )
}

/**
 * The compact markdown preview — a deliberate mini renderer (headings, lists,
 * task items, quotes, fences, rules, inline code/bold/em/del/links). It keeps
 * the notebook's developer-tool density instead of pulling in a full MD stack.
 */
function MarkdownBody({ md }: { md: string }) {
  const nodes = useMemo(() => renderMarkdown(md), [md])
  return <div {...stylex.props(styles.md)}>{nodes}</div>
}

const INLINE_RE =
  /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\*[^*\n]+)|(~~[^~\n]+~~)|(\[[^\]\n]+\]\([^)\n]+\))/g
const BLOCK_START_RE = /^(```|#{1,6}\s|>|[-+*]\s|\d+[.)]\s|(-{3,}|\*{3,}|_{3,})$)/

function renderInline(text: string): ReactNode[] {
  const parts: ReactNode[] = []
  let last = 0
  let n = 0
  for (const m of text.matchAll(INLINE_RE)) {
    const idx = m.index
    if (idx > last) parts.push(text.slice(last, idx))
    const [raw, code, bold, em, del, link] = m
    const key = `i${n++}`
    if (code) {
      parts.push(
        <code key={key} {...stylex.props(styles.mdCode)}>
          {code.slice(1, -1)}
        </code>,
      )
    } else if (bold) {
      parts.push(<strong key={key}>{renderInline(bold.slice(2, -2))}</strong>)
    } else if (em) {
      parts.push(<em key={key}>{renderInline(em.slice(1, -1))}</em>)
    } else if (del) {
      parts.push(<del key={key}>{renderInline(del.slice(2, -2))}</del>)
    } else if (link) {
      const lm = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(raw)
      if (lm) {
        parts.push(
          <a
            key={key}
            href={lm[2]}
            target="_blank"
            rel="noreferrer"
            {...stylex.props(styles.mdLink)}
          >
            {lm[1]}
          </a>,
        )
      }
    }
    last = idx + raw.length
  }
  if (last < text.length) parts.push(text.slice(last))
  return parts
}

function renderMarkdown(md: string): ReactNode[] {
  const nodes: ReactNode[] = []
  const lines = md.split('\n')
  let i = 0
  let n = 0
  const key = () => `b${n++}`
  while (i < lines.length) {
    const t = lines[i]!.trim()
    if (!t) {
      i++
      continue
    }
    const fence = /^```(\w*)/.exec(t)
    if (fence) {
      const buf: string[] = []
      i++
      while (i < lines.length && !lines[i]!.trim().startsWith('```')) {
        buf.push(lines[i]!)
        i++
      }
      i++
      nodes.push(
        <div key={key()} {...stylex.props(styles.mdFence)}>
          {fence[1] && <span {...stylex.props(styles.mdFenceLang)}>{fence[1]}</span>}
          <pre {...stylex.props(styles.mdFenceCode)}>{buf.join('\n')}</pre>
        </div>,
      )
      continue
    }
    const h = /^(#{1,6})\s+(.*)/.exec(t)
    if (h) {
      const lvl = h[1]!.length
      nodes.push(
        <div
          key={key()}
          {...stylex.props(
            lvl <= 2 ? styles.mdH2 : lvl <= 4 ? styles.mdH4 : styles.mdH6,
          )}
        >
          {renderInline(h[2]!)}
        </div>,
      )
      i++
      continue
    }
    if (/^(-{3,}|\*{3,}|_{3,})$/.test(t)) {
      nodes.push(<hr key={key()} {...stylex.props(styles.mdHr)} />)
      i++
      continue
    }
    if (t.startsWith('>')) {
      const buf: string[] = []
      while (i < lines.length && lines[i]!.trim().startsWith('>')) {
        buf.push(lines[i]!.trim().replace(/^>\s?/, ''))
        i++
      }
      nodes.push(
        <div key={key()} {...stylex.props(styles.mdQuote)}>
          {renderMarkdown(buf.join('\n'))}
        </div>,
      )
      continue
    }
    const item = /^([-+*]|\d+[.)])\s+(\[[ xX]\]\s+)?(.*)/.exec(t)
    if (item) {
      const ordered = /\d/.test(item[1]!.charAt(0))
      const items: { check: boolean | null; text: string }[] = []
      while (i < lines.length) {
        const m = /^([-+*]|\d+[.)])\s+(\[[ xX]\]\s+)?(.*)/.exec(lines[i]!.trim())
        if (!m) break
        items.push({
          check: m[2] ? m[2].toLowerCase().includes('x') : null,
          text: m[3]!,
        })
        i++
      }
      nodes.push(
        ordered ? (
          <ol key={key()} {...stylex.props(styles.mdList)}>
            {items.map((it, j) => (
              <li key={j} {...stylex.props(styles.mdLi)}>
                {it.check !== null && (
                  <span {...stylex.props(styles.mdCheck, it.check && styles.mdCheckDone)}>
                    {it.check ? '☑' : '☐'}
                  </span>
                )}
                {renderInline(it.text)}
              </li>
            ))}
          </ol>
        ) : (
          <ul key={key()} {...stylex.props(styles.mdList)}>
            {items.map((it, j) => (
              <li key={j} {...stylex.props(styles.mdLi)}>
                {it.check !== null && (
                  <span {...stylex.props(styles.mdCheck, it.check && styles.mdCheckDone)}>
                    {it.check ? '☑' : '☐'}
                  </span>
                )}
                {renderInline(it.text)}
              </li>
            ))}
          </ul>
        ),
      )
      continue
    }
    const buf: string[] = []
    while (i < lines.length && lines[i]!.trim() && !BLOCK_START_RE.test(lines[i]!.trim())) {
      buf.push(lines[i]!.trim())
      i++
    }
    nodes.push(
      <p key={key()} {...stylex.props(styles.mdP)}>
        {renderInline(buf.join(' '))}
      </p>,
    )
  }
  return nodes
}

/** Words across every block's text — markdown prose, queries, excerpts. */
function wordCount(blocks: MockNotebookBlock[]): number {
  let words = 0
  for (const b of blocks) {
    const text =
      b.type === 'markdown'
        ? b.markdown
        : b.type === 'query'
          ? `${b.query} ${b.hits.map((h) => h.preview).join(' ')}`
          : b.type === 'file'
            ? b.content
            : `${b.signature} ${b.doc ?? ''}`
    words += text.split(/\s+/).filter(Boolean).length
  }
  return words
}

/** Serializes a notebook to a standalone markdown document (real export). */
function notebookToMarkdown(n: MockNotebook, blocks: MockNotebookBlock[]): string {
  const out: string[] = [
    `# ${n.title}`,
    '',
    n.description,
    '',
    `> ${n.namespace} · owner ${n.owner} · ${n.visibility} · ${blocks.length} blocks`,
    '',
  ]
  for (const b of blocks) {
    switch (b.type) {
      case 'markdown':
        out.push(b.markdown, '')
        break
      case 'query':
        out.push('```txt', b.query, '```', '')
        for (const h of b.hits) {
          out.push(`- \`${h.path}:${h.line}\` — ${h.preview} *(${h.route})*`)
        }
        out.push('')
        break
      case 'file':
        out.push(
          `**\`${b.path}${b.lineRange ? `#L${b.lineRange[0]}-${b.lineRange[1]}` : ''}\`**`,
          '',
          '```',
          b.content,
          '```',
          '',
        )
        break
      case 'symbol':
        out.push(
          `**\`${b.name}\`** · ${b.kind} · \`${b.path}:${b.line}\``,
          '',
          '```',
          b.signature,
          '```',
          '',
        )
        if (b.doc) out.push(b.doc, '')
        break
    }
  }
  return out.join('\n')
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },
  head: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  title: {
    margin: 0,
    fontSize: 15,
    fontWeight: 600,
  },
  count: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  spacer: {
    flex: 1,
  },
  search: {
    width: 240,
  },
  body: {
    display: 'flex',
    alignItems: 'flex-start',
    flexDirection: {
      default: 'row',
      '@media (max-width: 900px)': 'column',
    },
  },
  list: {
    width: {
      default: 340,
      '@media (max-width: 900px)': '100%',
    },
    flexShrink: 0,
    display: 'flex',
    flexDirection: 'column',
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderBase,
    borderRightWidth: {
      default: 1,
      '@media (max-width: 900px)': 0,
    },
    borderRightStyle: 'solid',
    borderRightColor: vars.borderMute,
    paddingRight: {
      default: 16,
      '@media (max-width: 900px)': 0,
    },
  },
  detail: {
    flex: 1,
    minWidth: 0,
    width: {
      default: 'auto',
      '@media (max-width: 900px)': '100%',
    },
    paddingLeft: {
      default: 20,
      '@media (max-width: 900px)': 0,
    },
  },
  row: {
    display: 'flex',
    flexDirection: 'column',
    gap: 4,
    width: '100%',
    textAlign: 'left',
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 8,
    paddingRight: 8,
    borderWidth: 0,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: 'inherit',
    fontFamily: 'inherit',
    cursor: 'pointer',
  },
  rowActive: {
    backgroundColor: vars.bgActive,
  },
  rowTitleLine: {
    display: 'flex',
    alignItems: 'center',
    gap: 5,
    minWidth: 0,
  },
  rowLock: {
    display: 'inline-flex',
    color: vars.colorFaint,
    flexShrink: 0,
  },
  rowTitle: {
    fontSize: 12,
    fontWeight: 600,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  rowDesc: {
    fontSize: 11,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  rowMeta: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    fontSize: 11,
    color: vars.colorFaint,
  },
  stars: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorMuted,
  },
  rowOwner: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  chips: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: vars.bgSunken,
    borderRadius: 4,
    paddingTop: 1,
    paddingBottom: 1,
    paddingLeft: 5,
    paddingRight: 5,
  },
  rowMetaSpacer: {
    flex: 1,
  },
  docHead: {
    display: 'flex',
    alignItems: 'flex-start',
    gap: 10,
  },
  docTitleWrap: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    flexDirection: 'column',
    gap: 3,
  },
  docTitle: {
    margin: 0,
    fontSize: 16,
    fontWeight: 600,
    lineHeight: 1.3,
  },
  docDesc: {
    margin: 0,
    fontSize: 12,
    color: vars.colorMuted,
  },
  docMeta: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    flexWrap: 'wrap',
    marginTop: 10,
    paddingBottom: 10,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderBase,
  },
  docMetaItem: {
    display: 'inline-flex',
    alignItems: 'baseline',
    gap: 4,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 11,
    color: vars.colorFaint,
  },
  docLayout: {
    display: 'flex',
    alignItems: 'flex-start',
    gap: 20,
  },
  docCol: {
    flex: 1,
    minWidth: 0,
  },
  outline: {
    width: 150,
    flexShrink: 0,
    position: 'sticky',
    top: 0,
    display: {
      default: 'flex',
      '@media (max-width: 1150px)': 'none',
    },
    flexDirection: 'column',
    paddingTop: 14,
    paddingBottom: 14,
    paddingLeft: 12,
    borderLeftWidth: 1,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderMute,
  },
  outlineRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    width: '100%',
    paddingTop: 4,
    paddingBottom: 4,
    paddingLeft: 4,
    paddingRight: 4,
    borderWidth: 0,
    borderRadius: 4,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: vars.colorFaint,
    fontFamily: 'inherit',
    cursor: 'pointer',
    textAlign: 'left',
  },
  outlineRowActive: {
    color: vars.colorBase,
    backgroundColor: vars.bgActive,
  },
  outlineIcon: {
    display: 'inline-flex',
    flexShrink: 0,
    color: vars.colorMuted,
  },
  outlineText: {
    flex: 1,
    minWidth: 0,
    fontSize: 11,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  outlineIdx: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  note: {
    margin: 0,
    fontSize: 11,
    color: vars.colorFaint,
  },
  block: {
    paddingTop: 8,
    paddingBottom: 10,
  },
  blockHead: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    minHeight: 22,
  },
  blockKind: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    userSelect: 'none',
  },
  blockNote: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
    whiteSpace: 'nowrap',
  },
  headSpacer: {
    flex: 1,
  },
  headInput: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    backgroundColor: 'transparent',
    borderWidth: 0,
    borderRadius: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 4,
    paddingRight: 4,
    marginLeft: -4,
    outline: 'none',
    '::placeholder': {
      color: vars.colorFaint,
    },
    ':focus': {
      backgroundColor: vars.bgSunken,
    },
  },
  headInputName: {
    flex: 'none',
    width: 180,
  },
  numInput: {
    width: 28,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: 'transparent',
    borderWidth: 0,
    textAlign: 'center',
    outline: 'none',
    ':focus': {
      color: vars.colorBase,
    },
  },
  rangeWrap: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    flexShrink: 0,
  },
  headButton: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorMuted,
    backgroundColor: 'transparent',
    borderWidth: 0,
    borderRadius: 4,
    paddingTop: 2,
    paddingBottom: 2,
    paddingLeft: 5,
    paddingRight: 5,
    cursor: 'pointer',
    flexShrink: 0,
    whiteSpace: 'nowrap',
    ':hover': {
      color: vars.colorBase,
      backgroundColor: vars.bgHover,
    },
  },
  actions: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
    flexShrink: 0,
    opacity: 0,
    transition: 'opacity 120ms ease',
  },
  actionsRevealed: {
    opacity: 1,
  },
  actionBtn: {
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 24,
    height: 24,
    fontFamily: 'inherit',
    fontSize: 10,
    color: vars.colorFaint,
    backgroundColor: 'transparent',
    borderWidth: 0,
    borderRadius: 4,
    cursor: 'pointer',
    ':hover': {
      color: vars.colorBase,
      backgroundColor: vars.bgHover,
    },
    ':disabled': {
      opacity: 0.35,
      cursor: 'default',
      color: vars.colorFaint,
      backgroundColor: 'transparent',
    },
  },
  actionBtnActive: {
    color: vars.colorBase,
    backgroundColor: vars.bgActive,
  },
  addRow: {
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    paddingTop: 2,
    paddingBottom: 2,
    borderTopWidth: 1,
    borderTopStyle: 'dashed',
    borderTopColor: vars.borderMute,
    opacity: {
      default: 0.4,
      ':hover': 1,
    },
    transition: 'opacity 120ms ease',
  },
  addTrigger: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 5,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
    backgroundColor: 'transparent',
    borderWidth: 0,
    borderRadius: 4,
    paddingTop: 3,
    paddingBottom: 3,
    paddingLeft: 8,
    paddingRight: 8,
    cursor: 'pointer',
    ':hover': {
      color: vars.colorBase,
      backgroundColor: vars.bgHover,
    },
  },
  blockEmpty: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    paddingTop: 4,
    paddingBottom: 4,
  },
  hitList: {
    display: 'flex',
    flexDirection: 'column',
    marginTop: 4,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
  },
  hitRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    width: '100%',
    paddingTop: 5,
    paddingBottom: 5,
    paddingLeft: 4,
    paddingRight: 4,
    borderWidth: 0,
    borderBottomWidth: 1,
    borderBottomStyle: 'solid',
    borderBottomColor: vars.borderMute,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
    color: 'inherit',
    fontFamily: 'inherit',
    cursor: 'pointer',
    textAlign: 'left',
  },
  hitPath: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorBase,
    flexShrink: 0,
  },
  hitPreview: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorMuted,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  },
  code: {
    marginTop: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 6,
    backgroundColor: vars.bgCode,
    paddingTop: 6,
    paddingBottom: 6,
    overflowX: 'auto',
  },
  codeLine: {
    display: 'flex',
    alignItems: 'baseline',
    gap: 12,
    paddingLeft: 10,
    paddingRight: 10,
  },
  gutter: {
    width: 24,
    flexShrink: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    lineHeight: 1.6,
    color: vars.colorFaint,
    textAlign: 'right',
    userSelect: 'none',
  },
  codeText: {
    flex: 1,
    minWidth: 0,
    fontFamily: font.mono,
    fontSize: 11,
    lineHeight: 1.6,
    color: vars.colorBase,
    whiteSpace: 'pre',
  },
  symDoc: {
    margin: 0,
    marginTop: 6,
    fontSize: 11,
    color: vars.colorMuted,
  },
  mdEditArea: {
    display: 'block',
    width: '100%',
    marginTop: 4,
    fontFamily: font.mono,
    fontSize: 11.5,
    lineHeight: 1.65,
    color: vars.colorBase,
    backgroundColor: vars.bgSunken,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 6,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    paddingRight: 10,
    outline: 'none',
    resize: 'none',
    overflowY: 'hidden',
    ':focus': {
      borderColor: vars.borderBase,
    },
  },
  mdEditBar: {
    display: 'flex',
    alignItems: 'center',
    gap: 6,
    marginTop: 6,
  },
  mdEditHint: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  md: {
    fontSize: 12,
    lineHeight: 1.55,
    color: vars.colorBase,
  },
  mdH2: {
    fontSize: 13.5,
    fontWeight: 600,
    lineHeight: 1.4,
    marginTop: 10,
    marginBottom: 4,
    ':first-child': {
      marginTop: 2,
    },
  },
  mdH4: {
    fontSize: 12,
    fontWeight: 600,
    lineHeight: 1.4,
    marginTop: 8,
    marginBottom: 3,
    ':first-child': {
      marginTop: 2,
    },
  },
  mdH6: {
    fontSize: 11,
    fontWeight: 600,
    lineHeight: 1.4,
    color: vars.colorMuted,
    marginTop: 6,
    marginBottom: 2,
    ':first-child': {
      marginTop: 2,
    },
  },
  mdP: {
    margin: 0,
    marginTop: 3,
    marginBottom: 3,
  },
  mdCode: {
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.primary500,
    backgroundColor: vars.bgCode,
    borderRadius: 3,
    paddingTop: 0,
    paddingBottom: 1,
    paddingLeft: 4,
    paddingRight: 4,
  },
  mdLink: {
    color: vars.primary500,
    textDecoration: 'underline',
    textDecorationColor: vars.borderBase,
    textUnderlineOffset: 2,
    ':hover': {
      textDecorationColor: vars.primary500,
    },
  },
  mdFence: {
    position: 'relative',
    marginTop: 4,
    marginBottom: 4,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderMute,
    borderRadius: 6,
    backgroundColor: vars.bgCode,
    overflowX: 'auto',
  },
  mdFenceLang: {
    position: 'absolute',
    top: 4,
    right: 8,
    fontFamily: font.mono,
    fontSize: 9,
    color: vars.colorFaint,
    textTransform: 'uppercase',
    userSelect: 'none',
  },
  mdFenceCode: {
    margin: 0,
    paddingTop: 8,
    paddingBottom: 8,
    paddingLeft: 10,
    paddingRight: 10,
    fontFamily: font.mono,
    fontSize: 11,
    lineHeight: 1.6,
    color: vars.colorBase,
    whiteSpace: 'pre',
  },
  mdQuote: {
    marginTop: 4,
    marginBottom: 4,
    paddingLeft: 10,
    borderLeftWidth: 2,
    borderLeftStyle: 'solid',
    borderLeftColor: vars.borderBase,
    color: vars.colorMuted,
  },
  mdHr: {
    borderWidth: 0,
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderBase,
    marginTop: 8,
    marginBottom: 8,
  },
  mdList: {
    margin: 0,
    marginTop: 3,
    marginBottom: 3,
    paddingLeft: 18,
    display: 'flex',
    flexDirection: 'column',
    gap: 1,
  },
  mdLi: {
    lineHeight: 1.55,
    '::marker': {
      color: vars.colorFaint,
    },
  },
  mdCheck: {
    display: 'inline-block',
    marginRight: 5,
    color: vars.colorFaint,
    fontSize: 11,
    userSelect: 'none',
  },
  mdCheckDone: {
    color: vars.primary500,
  },
})
