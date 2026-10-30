/** Referenced by: one row for each source that mentions this document, and under it, when opened, the line around each mention. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronRight } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { mentionsQuery } from '@/api/links';
import { mentionPath, sourcePath, whereOf } from '@/features/links/anchors';
import { useOpenLinked } from '@/features/links/open';
import type { ListState } from '@/features/links/listState';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { NamedChip } from '@/features/people/NamedChip';
import type { LinkOccurrence, LinkedThing } from '@/generated/api-types';
import { AuthorChip } from '@/ui/AuthorChip';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

/** Rows shown as placeholders while the first page comes. */
const LOADING_ROWS = 3;

export function ReferencedBy({ documentId, list }: { documentId: string; list: ListState }) {
  const { t } = useTranslation();
  const rows = list.rows;
  const labels = usePlaceLabels(rows.map((row) => ({ id: row.id, name: row.name ?? '', above: row.context })));
  if (list.pending) return <SkeletonRows rows={LOADING_ROWS} />;
  if (list.failed) return <ErrorAlert error={list.error} action={<Button size="sm" onClick={list.retry}>{t('common.tryAgain')}</Button>} />;
  // A page can hold nothing the reader may read and still be followed by more: the list then goes on by itself.
  if (rows.length === 0 && !list.hasMore) return <StateLine title={t('links.referencedBy.empty')} compact />;
  return <div className="flex min-w-0 flex-col" data-links-list="referenced_by">
    {rows.length > 0 && <ul className="flex min-w-0 flex-col gap-1" aria-label={t('links.tabs.referenced_by')}>
      {rows.map((row) => <SourceRow key={`${row.kind}:${row.id}`} documentId={documentId} source={row} label={labels.get(row.id)} />)}
    </ul>}
    <Pagination autoLoad hasMore={list.hasMore} loading={list.fetching} error={list.moreFailed ? list.error : undefined} onMore={list.more} />
  </div>;
}

const chipKind = (kind: LinkedThing['kind']) => (kind === 'document' ? 'doc' : kind === 'chat' ? 'chat' : 'thread');

function SourceRow({ documentId, source, label }: { documentId: string; source: LinkedThing; label?: string | undefined }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const name = label ?? source.name ?? '';
  const of = `${source.kind}:${source.id}`;
  const more = useInfiniteQuery({ ...mentionsQuery(documentId, of, source.next_mentions ?? ''), enabled: false });
  const later = more.data?.pages.flatMap((page) => page.items[0]?.occurrences ?? []) ?? [];
  const mentions = [...source.occurrences, ...later];
  const hasMore = more.data === undefined ? source.next_mentions !== null : more.hasNextPage;
  return <li className="min-w-0 rounded-md hover:bg-bg-subtle" data-link-source={source.id}>
    <div className="flex min-h-(--row-height) min-w-0 flex-wrap items-center gap-x-2 gap-y-1 px-1 py-1">
      <button type="button" aria-expanded={open} aria-label={t(open ? 'links.row.collapse' : 'links.row.expand', { name })} onClick={() => setOpen(!open)}
        className="focus-inset flex size-6 shrink-0 items-center justify-center rounded-sm text-text-subtle hover:bg-bg-muted hover:text-text">
        <ChevronRight aria-hidden className={cx('size-4 transition-transform', open && 'rotate-90')} />
      </button>
      <NamedChip id={source.id} kind={chipKind(source.kind)} path={sourcePath(source)} name={name} presentation={{ place: 'compact' }} />
      <span className="text-sm text-text-muted">{source.mentions_more ? t('links.mentionsMore', { number: source.mentions }) : t('links.mentions', { count: source.mentions })}</span>
    </div>
    {open && <div className="ml-7 flex min-w-0 flex-col gap-2 pr-2 pb-2">
      <ul className="flex min-w-0 flex-col gap-2" aria-label={t('links.row.mentions', { name })}>
        {mentions.map((mention) => <Mention key={mentionKey(mention)} source={source} name={name} mention={mention} />)}
      </ul>
      <Pagination hasMore={hasMore} loading={more.isFetchingNextPage || more.isFetching} error={more.isFetchNextPageError ? more.error : undefined} onMore={() => void more.fetchNextPage({ cancelRefetch: false })} className="py-1" />
    </div>}
  </li>;
}

function mentionKey(mention: LinkOccurrence): string {
  const anchor = mention.anchor;
  return anchor.type === 'block' ? anchor.block_id : anchor.type === 'comment' ? anchor.comment_id : anchor.type === 'message' ? anchor.message_id : anchor.item_id;
}

/** One mention: its line, with the reference in bold, and under it who wrote it and when. */
function Mention({ source, name, mention }: { source: LinkedThing; name: string; mention: LinkOccurrence }) {
  const { t } = useTranslation();
  const open = useOpenLinked();
  const where = whereOf(mention.anchor);
  const { before, reference, after } = mention.line;
  return <li className="flex min-w-0 flex-col gap-0.5 border-l-2 border-border-strong pl-2" data-link-mention="">
    <button type="button" aria-label={t('links.row.open', { name })} onClick={() => open(mentionPath(source, mention.anchor))}
      className="focus-inset block min-w-0 rounded-sm text-left text-sm wrap-anywhere hover:bg-bg-muted">
      <span>{before}</span>{reference !== '' && <strong className="font-semibold">{reference}</strong>}<span>{after}</span>
    </button>
    <span className="flex min-w-0 flex-wrap items-center gap-x-1 text-sm text-text-muted">
      <span>{t(where.key, { number: where.number ?? 0 })}</span>
      {mention.by !== null && <><span aria-hidden>·</span><AuthorChip author={mention.by} /></>}
      <span aria-hidden>·</span><RelativeTime at={mention.at} />
    </span>
  </li>;
}
