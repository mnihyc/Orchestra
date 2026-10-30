/** The tree of ticks in the dialog of a place: the place and what lies in it that its person may read, each row with a box, its name, what is in it, and how many things there are left out.
 *
 * It is the tree of the left bar in the shape of a list of choices: the arrow keys walk it and open and close
 * its places, the space bar ticks the row, and a press on a row does. A place ticked takes what is in it. A row
 * whose thing may be read and not taken away is shown empty and greyed, with the reason on hover and focus.
 */
import type { TFunction } from 'i18next';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ByteSize } from '@/ui/ByteSize';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { cx } from '@/ui/cx';
import { NodeIcon } from '@/ui/NodeIcon';
import { Tooltip } from '@/ui/Tooltip';
import { Tree, type TreeRow } from '@/ui/Tree';

import type { RowItem } from './rows';

const nameOf = (item: RowItem): string => (item.type === 'node' ? item.node.name : item.document.title);

/** The words under a place's name: what lies in it, the kinds that are there. */
function inside(item: Extract<RowItem, { type: 'node' }>, t: TFunction, locale: string): string {
  const count = (value: number) => formatNumber(value, locale);
  const parts: string[] = [];
  const { places, topics, threads } = item.tally;
  if (places > 0) parts.push(item.node.kind === 'org' ? t('export.tree.places', { count: places, formatted: count(places) }) : t('export.tree.folders', { count: places, formatted: count(places) }));
  if (topics > 0) parts.push(t('export.tree.topics', { count: topics, formatted: count(topics) }));
  if (threads > 0) parts.push(t('export.tree.threads', { count: threads, formatted: count(threads) }));
  if (item.documents > 0) parts.push(t(item.documentsComplete ? 'export.tree.documents' : 'export.tree.documentsAtLeast', { count: item.documents, formatted: count(item.documents) }));
  return new Intl.ListFormat(locale).format(parts);
}

function Content({ item }: { item: RowItem }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  if (item.type === 'document') {
    const row = (
      <span className={cx('flex min-w-0 flex-1 items-center gap-2', item.greyed && 'text-text-disabled')}>
        <NodeIcon kind="document" size={16} className="shrink-0" />
        <span className="truncate">{item.document.title}</span>
        <AsAdminMark of={adminAccessOf(item.document)?.place} iconOnly />
        {item.greyed && <span className="sr-only">{t('export.tree.cannot')}</span>}
        <span className="ml-auto shrink-0 text-xs text-text-muted">
          <ByteSize bytes={item.document.bytes + item.document.files_bytes} />
        </span>
      </span>
    );
    return item.greyed ? <Tooltip content={t('export.tree.cannot')}>{row}</Tooltip> : row;
  }
  const { node } = item;
  const words = node.kind === 'thread' ? '' : inside(item, t, locale);
  // The name takes the room it needs first and what lies in the place has what is left (a grid of an auto column and one that is flexible, not two items that shrink together: those take a hair off the name and cut it short): the name is what is chosen.
  const row = (
    <span className={cx('flex min-w-0 flex-1 items-center gap-2', item.greyed && 'text-text-disabled')}>
      <NodeIcon kind={node.kind === 'project' && item.folder ? 'folder' : node.kind} color={node.color} own={node.custom_color} done={node.topic?.status === 'done'} size={16} className="shrink-0" />
      <span className="grid min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-2">
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate text-text">{node.name}</span>
          <AsAdminMark of={adminAccessOf(node)?.place} iconOnly />
          {item.greyed && <span className="sr-only">{t('export.tree.cannot')}</span>}
          {node.archived && <span className="shrink-0 text-xs text-text-muted">{t('export.tree.archived')}</span>}
        </span>
        <span className="truncate text-right text-xs text-text-muted" title={words}>
          {words}
        </span>
      </span>
      {item.leftOut > 0 && (
        <span className="shrink-0 rounded-full bg-bg-muted px-2 text-xs text-text-muted" title={t('export.tree.leftOutHere', { count: item.leftOut })}>
          {t('export.tree.leftOut', { count: item.leftOut })}
        </span>
      )}
    </span>
  );
  return item.greyed ? <Tooltip content={t('export.tree.cannot')}>{row}</Tooltip> : row;
}

const renderRow = (row: TreeRow<RowItem>) => <Content item={row.item} />;

export interface PlaceTreeProps {
  rows: readonly TreeRow<RowItem>[];
  onExpandedChange: (id: string, expanded: boolean) => void;
  onTick: (row: TreeRow<RowItem>, on: boolean) => void;
  className?: string;
}

export function PlaceTree({ rows, onExpandedChange, onTick, className }: PlaceTreeProps) {
  const { t } = useTranslation();
  const holder = useRef<HTMLDivElement>(null);
  const focused = useRef(false);
  const drawn = rows.length > 0;
  // The keyboard begins in the tree once it is there: the dialog opened before its rows had arrived, and a person who came by the keyboard walks it from its first row.
  useEffect(() => {
    if (!drawn || focused.current) return;
    focused.current = true;
    holder.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]')?.focus();
  }, [drawn]);
  return (
    <div ref={holder}>
      <Tree<RowItem> label={t('export.tree.label')} rows={rows} onExpandedChange={onExpandedChange} onTick={onTick} nameOf={nameOf} renderRow={renderRow} {...(className === undefined ? {} : { className })} />
    </div>
  );
}
