/** References: what this document points to, each as the chip its author wrote; what the reader may not open stays a locked chip with its written label. */
import { useTranslation } from 'react-i18next';

import type { ListState } from '@/features/links/listState';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { NamedChip, TypedChip } from '@/features/people/NamedChip';
import type { InlineRefKind, LinkedThing } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

const LOADING_ROWS = 3;

/** The kind of reference a target is written as. */
function refKind(kind: LinkedThing['kind']): InlineRefKind {
  switch (kind) {
    case 'document': return 'document';
    case 'chat': return 'chat';
    case 'file': return 'file';
    case 'environment_file': return 'environment_file';
    case 'thread':
    case 'place': return 'node';
  }
}

const chipKind = (target: LinkedThing) => target.kind === 'document' ? 'doc' as const : target.kind === 'chat' ? 'chat' as const : target.kind === 'thread' ? 'thread' as const : target.node_kind ?? 'node' as const;

const pathOf = (target: LinkedThing): string => target.kind === 'document' ? `/doc/${target.id}` : target.kind === 'chat' ? `/chat/${target.id}` : target.kind === 'thread' ? `/thread/${target.id}` : `/node/${target.id}`;

export function References({ list }: { list: ListState }) {
  const { t } = useTranslation();
  const rows = list.rows;
  const labels = usePlaceLabels(rows.filter((row) => row.readable && row.name !== null).map((row) => ({ id: row.id, name: row.name ?? '', above: row.context })));
  if (list.pending) return <SkeletonRows rows={LOADING_ROWS} />;
  if (list.failed) return <ErrorAlert error={list.error} action={<Button size="sm" onClick={list.retry}>{t('common.tryAgain')}</Button>} />;
  if (rows.length === 0 && !list.hasMore) return <StateLine title={t('links.references.empty')} compact />;
  return <div className="flex min-w-0 flex-col" data-links-list="references">
    {rows.length > 0 && <ul className="flex min-w-0 flex-col gap-1" aria-label={t('links.tabs.references')}>
      {rows.map((target) => <li key={`${target.kind}:${target.id}:${target.path ?? ''}`} className="flex min-h-(--row-height) min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-md px-2 py-1 hover:bg-bg-subtle">
        {target.readable && target.name !== null && (target.kind === 'document' || target.kind === 'chat' || target.kind === 'thread')
          ? <NamedChip id={target.id} kind={chipKind(target)} path={pathOf(target)} name={labels.get(target.id) ?? target.name} presentation={{ place: 'compact' }} />
          : <TypedChip reference={{ kind: refKind(target.kind), id: target.id, ...(target.path === undefined || target.path === null ? {} : { path: target.path }), ...(target.label === null ? {} : { label: target.label }) }} />}
        {target.mentions > 1 && <span className="text-sm text-text-muted">{t('links.mentions', { count: target.mentions })}</span>}
      </li>)}
    </ul>}
    <Pagination autoLoad hasMore={list.hasMore} loading={list.fetching} error={list.moreFailed ? list.error : undefined} onMore={list.more} />
  </div>;
}
