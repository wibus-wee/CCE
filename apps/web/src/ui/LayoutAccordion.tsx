import * as stylex from '@stylexjs/stylex'
import { Accordion } from '@base-ui-components/react'
import type { ReactNode } from 'react'
import { IconChevron } from './icons'
import { vars } from './tokens.stylex'

/**
 * Port of `LayoutAccordion` on Base UI `Accordion` — a collapsible section
 * (chevron + title + right-aligned `meta` slot) with height animation.
 */
export function LayoutAccordion({
  title,
  meta,
  defaultOpen = false,
  disabled = false,
  children,
}: {
  title: ReactNode
  meta?: ReactNode
  defaultOpen?: boolean
  disabled?: boolean
  children: ReactNode
}) {
  return (
    <Accordion.Root
      className={stylex.props(styles.root).className}
      defaultValue={defaultOpen ? [0] : []}
    >
      <Accordion.Item value={0} disabled={disabled} {...stylex.props(styles.item)}>
        <Accordion.Header className={stylex.props(styles.header).className}>
          <Accordion.Trigger {...stylex.props(styles.trigger)}>
            <span {...stylex.props(styles.chevron)}>
              <IconChevron size={12} />
            </span>
            <span {...stylex.props(styles.title)}>{title}</span>
            {meta && <span {...stylex.props(styles.meta)}>{meta}</span>}
          </Accordion.Trigger>
        </Accordion.Header>
        <Accordion.Panel {...stylex.props(styles.panel)}>
          <div {...stylex.props(styles.body)}>{children}</div>
        </Accordion.Panel>
      </Accordion.Item>
    </Accordion.Root>
  )
}

/**
 * `LayoutDisclosure` — a hairline-separated collapsible row used by pack
 * items and provider sections.
 */
export function LayoutDisclosure({
  summary,
  meta,
  defaultOpen,
  children,
}: {
  summary: ReactNode
  meta?: ReactNode
  defaultOpen?: boolean
  children: ReactNode
}) {
  return (
    <div {...stylex.props(styles.disclosure)}>
      <LayoutAccordion title={summary} meta={meta} defaultOpen={defaultOpen}>
        {children}
      </LayoutAccordion>
    </div>
  )
}

const styles = stylex.create({
  root: {
    display: 'flex',
    flexDirection: 'column',
  },
  item: {
    display: 'flex',
    flexDirection: 'column',
  },
  header: {
    display: 'flex',
    marginTop: 0,
    marginBottom: 0,
    marginLeft: 0,
    marginRight: 0,
  },
  trigger: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    paddingTop: 10,
    paddingBottom: 10,
    paddingLeft: 0,
    paddingRight: 0,
    borderWidth: 0,
    backgroundColor: 'transparent',
    cursor: 'pointer',
    userSelect: 'none',
    color: vars.colorBase,
    fontFamily: 'inherit',
    fontSize: 'inherit',
    textAlign: 'left',
  },
  chevron: {
    display: 'inline-flex',
    color: vars.colorFaint,
    transitionProperty: 'transform',
    transitionDuration: '150ms',
    flexShrink: 0,
    transform: {
      default: 'rotate(0deg)',
      [stylex.when.ancestor('[data-open]')]: 'rotate(90deg)',
    },
  },
  title: {
    flex: 1,
    minWidth: 0,
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    fontSize: 13,
    fontWeight: 500,
  },
  meta: {
    flexShrink: 0,
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    color: vars.colorFaint,
    fontSize: 11,
  },
  panel: {
    overflow: 'hidden',
  },
  body: {
    paddingBottom: 12,
    paddingLeft: 20,
    minWidth: 0,
  },
  disclosure: {
    borderTopWidth: 1,
    borderTopStyle: 'solid',
    borderTopColor: vars.borderBase,
  },
})
