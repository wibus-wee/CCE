import * as stylex from '@stylexjs/stylex'
import { FormEvent, useState } from 'react'
import { api, ContextPack, describeError } from '../api'
import { useOpenFile } from '../lib/navigation'
import { usePackHistory, type PackHistoryEntry } from '../lib/packHistory'
import { ActionButton } from '../ui/ActionButton'
import { CopyButton } from '../ui/CopyButton'
import { DisplayBadge } from '../ui/DisplayBadge'
import { DisplayNumber, DisplayTimeAgo } from '../ui/DisplayNumber'
import { DisplayProgressBar } from '../ui/DisplayProgressBar'
import { FeedbackEmptyState } from '../ui/FeedbackStates'
import { FeedbackTip } from '../ui/FeedbackTip'
import { useNotification } from '../ui/FeedbackToasts'
import { FormField } from '../ui/FormField'
import { FormTextarea } from '../ui/FormInputs'
import { FormNumberInput } from '../ui/FormNumberInput'
import { IconLayers, IconRefresh, IconX } from '../ui/icons'
import { LayoutDisclosure } from '../ui/LayoutDisclosure'
import { LayoutSeparator } from '../ui/LayoutPrimitives'
import { iconButtons } from '../ui/recipes.stylex'
import { font, vars } from '../ui/tokens.stylex'

/**
 * Context — the source-linked pack builder. A question and a token budget
 * go in; an ordered, provenance-tagged item list comes out. Uncertainties
 * and missing capabilities are reported, never hidden.
 */
export function ContextScreen() {
  const [query, setQuery] = useState('Where is snapshot freshness decided?')
  const [budget, setBudget] = useState(4096)
  const [pack, setPack] = useState<ContextPack>()
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState(false)
  const { packs, push, remove } = usePackHistory()

  async function runPack(text: string, tokens: number) {
    const q = text.trim()
    if (!q) return
    setBusy(true)
    setError(undefined)
    try {
      const built = await api.context(q, tokens)
      setPack(built)
      push({
        query: q,
        budgetTokens: tokens,
        snapshotId: built.snapshotId,
        intent: built.intent,
        itemCount: built.items.length,
        usedTokens: built.usedTokens,
        at: new Date().toISOString(),
      })
    } catch (value) {
      setError(describeError(value))
    } finally {
      setBusy(false)
    }
  }

  async function ask(event: FormEvent) {
    event.preventDefault()
    await runPack(query, budget)
  }

  // Rebuild for real — history rows restore the form and re-POST, so a
  // rerun answers against the current snapshot, not a cached pack.
  function rerun(entry: PackHistoryEntry) {
    setQuery(entry.query)
    setBudget(entry.budgetTokens)
    void runPack(entry.query, entry.budgetTokens)
  }

  return (
    <div {...stylex.props(styles.root)}>
      <form onSubmit={(event) => void ask(event)} {...stylex.props(styles.form)}>
        <FormField label="Repository question">
          <FormTextarea
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            rows={3}
            placeholder="What do you need from this repository?"
          />
        </FormField>
        <div {...stylex.props(styles.controls)}>
          <FormField label="Token budget" description="packed context ceiling">
            <div {...stylex.props(styles.budget)}>
              <FormNumberInput
                min={256}
                max={128000}
                step={256}
                value={budget}
                onValueChange={(v) => setBudget(v ?? 4096)}
              />
            </div>
          </FormField>
          <div {...stylex.props(styles.actions)}>
            <ActionButton variant="primary" type="submit" loading={busy} disabled={!query.trim()}>
              Build source-linked pack
            </ActionButton>
          </div>
        </div>
      </form>

      <RecentPacks
        packs={packs}
        onRerun={rerun}
        onRemove={(p) => remove(p.query, p.budgetTokens)}
      />

      {error && <FeedbackTip variant="error">{error}</FeedbackTip>}

      {pack ? (
        <PackResult pack={pack} />
      ) : (
        !error && (
          <FeedbackEmptyState
            icon={<IconLayers size={20} />}
            title="The smallest sufficient world"
            description="A pack is the smallest source-linked context sufficient for the task — intent, budget, ranked items and explicit uncertainties."
          />
        )
      )}
    </div>
  )
}

/**
 * Recent packs — the builder's local history, mirroring the query screen's
 * recent searches: hairline rows with the intent badge, counts and
 * hover-revealed rerun/copy/remove actions. Stored in localStorage — a
 * single-operator convenience, never index truth.
 */
function RecentPacks({
  packs,
  onRerun,
  onRemove,
}: {
  packs: PackHistoryEntry[]
  onRerun: (p: PackHistoryEntry) => void
  onRemove: (p: PackHistoryEntry) => void
}) {
  return (
    <section aria-label="Recent packs" {...stylex.props(styles.histSection)}>
      <LayoutSeparator
        label="Recent packs"
        aside={`${packs.length} recent · stored locally`}
      />
      {packs.length === 0 ? (
        <p {...stylex.props(styles.histEmpty)}>no packs yet — ask above to build one</p>
      ) : (
        <ol {...stylex.props(styles.histList)}>
          {packs.map((p) => (
            <PackHistoryRow
              key={`${p.query}:${p.budgetTokens}`}
              pack={p}
              onRerun={() => onRerun(p)}
              onRemove={() => onRemove(p)}
            />
          ))}
        </ol>
      )}
    </section>
  )
}

function PackHistoryRow({
  pack: p,
  onRerun,
  onRemove,
}: {
  pack: PackHistoryEntry
  onRerun: () => void
  onRemove: () => void
}) {
  // StyleX has no parent-hover/child selectors — reveal the trailing actions
  // from row hover in state (same approach as BranchesScreen).
  const [hover, setHover] = useState(false)
  return (
    <li
      {...stylex.props(styles.histRow)}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
    >
      <DisplayBadge text={p.intent}>{p.intent.replaceAll('_', ' ')}</DisplayBadge>
      <span {...stylex.props(styles.histQuery)} title={p.query}>
        {p.query}
      </span>
      <span {...stylex.props(styles.histMeta)}>
        {p.itemCount} items · {p.usedTokens}/{p.budgetTokens}t
      </span>
      <span {...stylex.props(styles.histWhen)}>
        <DisplayTimeAgo value={p.at} />
      </span>
      <span {...stylex.props(styles.rowActions, hover && styles.rowActionsShown)}>
        <button
          type="button"
          title="rerun — rebuilds against the current snapshot"
          aria-label={`Rerun pack for '${p.query}'`}
          onClick={onRerun}
          {...stylex.props(iconButtons.mini)}
        >
          <IconRefresh size={11} />
        </button>
        <CopyButton text={p.query} title="copy query" size={11} />
        <button
          type="button"
          title="remove from history"
          aria-label={`Remove '${p.query}' from history`}
          onClick={onRemove}
          {...stylex.props(iconButtons.mini)}
        >
          <IconX size={11} />
        </button>
      </span>
    </li>
  )
}

function packToMarkdown(pack: ContextPack): string {
  const lines: string[] = [
    `# Context pack — ${pack.intent.replaceAll('_', ' ')}`,
    `snapshot ${pack.snapshotId} · ${pack.usedTokens}/${pack.budgetTokens} tokens`,
    '',
  ]
  for (const item of pack.items) {
    const src = item.provenance.sourceAddress
    lines.push(`## ${item.kind.replaceAll('_', ' ')} — ${item.title}`)
    if (src) lines.push(`source: ${src.path}:${src.startLine}`)
    lines.push(item.body.trim(), '')
  }
  if (pack.uncertainties.length) {
    lines.push('## uncertainties')
    for (const u of pack.uncertainties) lines.push(`- ${u.capability}: ${u.message}`)
  }
  if (pack.missingCapabilities.length) {
    lines.push('## missing capabilities')
    for (const c of pack.missingCapabilities) lines.push(`- ${c}`)
  }
  return lines.join('\n')
}

function PackResult({ pack }: { pack: ContextPack }) {
  const openFile = useOpenFile()
  const notify = useNotification()
  const used = pack.budgetTokens > 0 ? pack.usedTokens / pack.budgetTokens : 0
  return (
    <section aria-label="Context pack" {...stylex.props(styles.pack)}>
      <div {...stylex.props(styles.meta)}>
        <DisplayBadge text={pack.intent}>
          {pack.intent.replaceAll('_', ' ')}
        </DisplayBadge>
        <strong {...stylex.props(styles.packTitle)}>Context pack</strong>
        <span {...stylex.props(styles.metaSep)} />
        <span {...stylex.props(styles.tokens)}>
          <DisplayNumber value={pack.usedTokens} /> / <DisplayNumber value={pack.budgetTokens} /> tokens
        </span>
        <ActionButton
          onClick={() =>
            void navigator.clipboard
              .writeText(packToMarkdown(pack))
              .then(() =>
                notify.push('Pack copied', {
                  type: 'success',
                  description: `${pack.items.length} items as markdown`,
                }),
              )
              .catch(() => {})
          }
        >
          Copy pack
        </ActionButton>
      </div>
      <DisplayProgressBar value={used} />
      <code {...stylex.props(styles.snap)} title={`snapshot ${pack.snapshotId}`}>
        snapshot {pack.snapshotId.slice(0, 12)}
      </code>

      <div {...stylex.props(styles.items)}>
        {pack.items.map((item) => (
          <LayoutDisclosure
            key={item.id}
            defaultOpen={item.kind === 'orientation' || item.provenance.rank < 4}
            summary={
              <>
                <DisplayBadge color={false}>{item.kind.replaceAll('_', ' ')}</DisplayBadge>
                <span {...stylex.props(styles.itemTitle)}>{item.title}</span>
              </>
            }
            meta={
              <span {...stylex.props(styles.itemMeta)}>
                {item.estimatedTokens}t · rank {item.provenance.rank}
              </span>
            }
          >
            <pre {...stylex.props(styles.itemBody)}>{item.body}</pre>
            <p {...stylex.props(styles.provenance)}>
              {item.provenance.route.replaceAll('_', ' ')} · rank {item.provenance.rank} ·{' '}
              {item.provenance.whyRetrieved}
              {item.provenance.sourceAddress && (
                <>
                  {' · '}
                  <button
                    type="button"
                    {...stylex.props(styles.sourceLink)}
                    onClick={() =>
                      openFile(
                        item.provenance.sourceAddress!.path,
                        item.provenance.sourceAddress!.startLine,
                      )
                    }
                    title="Open at source"
                  >
                    <code>
                      {item.provenance.sourceAddress.path}:
                      {item.provenance.sourceAddress.startLine}
                    </code>
                  </button>
                </>
              )}
            </p>
          </LayoutDisclosure>
        ))}
      </div>

      {pack.uncertainties.length > 0 && (
        <FeedbackTip variant="warning" title="Uncertainties">
          {pack.uncertainties.map((uncertainty) => (
            <p key={`${uncertainty.capability}:${uncertainty.message}`} {...stylex.props(styles.uncertainty)}>
              <strong>{uncertainty.capability}</strong> — {uncertainty.message}
            </p>
          ))}
        </FeedbackTip>
      )}
      {pack.missingCapabilities.length > 0 && (
        <FeedbackTip variant="error" title="Missing capabilities">
          {pack.missingCapabilities.map((capability) => (
            <p key={capability} {...stylex.props(styles.uncertainty)}>
              {capability}
            </p>
          ))}
        </FeedbackTip>
      )}
    </section>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
    gap: 14,
  },
  form: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  controls: {
    display: 'flex',
    alignItems: 'flex-end',
    gap: 12,
    flexWrap: 'wrap',
  },
  budget: {
    width: 140,
  },
  actions: {
    display: 'flex',
    gap: 8,
    paddingBottom: 1,
  },
  // --- recent packs -----------------------------------------------------------
  histSection: {
    display: 'flex',
    flexDirection: 'column',
    gap: 6,
  },
  histList: {
    margin: 0,
    padding: 0,
    listStyle: 'none',
    display: 'flex',
    flexDirection: 'column',
  },
  // Flat hairline items — separators only, no card chrome.
  histRow: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    paddingTop: 6,
    paddingBottom: 6,
    borderTopWidth: { default: 1, ':first-child': 0 },
    borderTopStyle: 'solid',
    borderTopColor: vars.borderMute,
    fontSize: 12,
    backgroundColor: {
      default: 'transparent',
      ':hover': vars.bgHover,
    },
  },
  histQuery: {
    flex: 1,
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    color: vars.colorMuted,
  },
  histMeta: {
    flexShrink: 0,
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  histWhen: {
    flexShrink: 0,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
    width: 76,
    textAlign: 'right',
  },
  histEmpty: {
    margin: 0,
    fontFamily: font.mono,
    fontSize: 11,
    color: vars.colorFaint,
  },
  // Trailing hover actions — opacity-0 space is always reserved so the `when`
  // column never shifts; `:focus-within` keeps them reachable by keyboard.
  rowActions: {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 2,
    flexShrink: 0,
    paddingLeft: 4,
    paddingRight: 4,
    opacity: { default: 0, ':focus-within': 1 },
    pointerEvents: { default: 'none', ':focus-within': 'auto' },
    transitionProperty: 'opacity',
    transitionDuration: '120ms',
  },
  rowActionsShown: {
    opacity: 1,
    pointerEvents: 'auto',
  },
  pack: {
    display: 'flex',
    flexDirection: 'column',
    gap: 10,
  },
  meta: {
    display: 'flex',
    alignItems: 'center',
    gap: 10,
    flexWrap: 'wrap',
  },
  packTitle: {
    fontSize: 13,
    fontWeight: 600,
  },
  metaSep: {
    flex: 1,
  },
  tokens: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 12,
    color: vars.colorMuted,
  },
  snap: {
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  items: {
    borderRadius: 8,
    borderWidth: 1,
    borderStyle: 'solid',
    borderColor: vars.borderBase,
    backgroundColor: vars.bgRaised,
    paddingLeft: 12,
    paddingRight: 12,
  },
  itemTitle: {
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
    minWidth: 0,
  },
  itemMeta: {
    fontFamily: font.mono,
    fontVariantNumeric: 'tabular-nums',
    fontSize: 10,
    color: vars.colorFaint,
  },
  itemBody: {
    margin: 0,
    padding: 10,
    borderRadius: 6,
    backgroundColor: vars.bgCode,
    fontFamily: font.mono,
    fontSize: 11,
    lineHeight: '1.55',
    whiteSpace: 'pre-wrap',
    overflowWrap: 'anywhere',
    color: vars.colorBase,
    overflow: 'auto',
    maxHeight: 320,
  },
  provenance: {
    marginTop: 6,
    marginBottom: 2,
    fontFamily: font.mono,
    fontSize: 10,
    color: vars.colorFaint,
  },
  sourceLink: {
    borderWidth: 0,
    backgroundColor: 'transparent',
    fontFamily: font.mono,
    fontSize: 'inherit',
    color: vars.colorActive,
    padding: 0,
    cursor: 'pointer',
    textDecoration: 'underline',
    textDecorationStyle: 'dotted',
    textUnderlineOffset: 2,
    ':hover': { textDecorationStyle: 'solid' },
  },
  uncertainty: {
    margin: 0,
  },
})
