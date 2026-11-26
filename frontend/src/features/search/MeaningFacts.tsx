/** Actual query receipts share one status opening; inspecting them never restarts retrieval. */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { meaningSourcesQuery, type SearchFilters } from '@/api/search';
import type { SearchMeaningModel, SearchStreamEvent } from '@/generated/api-types';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { modelResource } from '@/features/resource-facts/facts';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

interface Props {
  filters: SearchFilters;
  summary: Extract<SearchStreamEvent, { type: 'meaning_sources' }> | null;
  label?: string;
  reason?: string;
  inlineReason?: string;
}

function ServingModel({ model }: { model: SearchMeaningModel }) {
  return <ResourceChip hint={false} subject={modelResource({ model_name: model.model_name, display_name: model.display_name, parameters: {} }, model.provider_id)} />;
}

export function MeaningFacts(props: Props) {
  const reader = useStore(client.session, (session) => `${session.userId ?? ''}:${session.readerVersion ?? 0}`);
  // Replacing the receipt or reader retires the opening and every prior private fact immediately.
  return <MeaningSubject key={`${reader}:${props.summary?.cursor ?? ''}`} {...props} reader={reader} />;
}

function MeaningSubject({ filters, summary, label, reason = '', inlineReason = '', reader }: Props & { reader: string }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const owner = useId();
  const [open, setOpen] = useState(false);
  const [opening, setOpening] = useState(0);
  const [text, setText] = useState('');
  const filter = useDebounced(text.trim());
  // Each opening owns its cancellation and starts with a current authorized read, including a reopen.
  const options = meaningSourcesQuery(filters, summary?.cursor ?? '', filter, JSON.stringify([reader, owner, opening]));
  const readKey = options.queryKey[2];
  const read = useInfiniteQuery({ ...options,
    enabled: open && summary?.cursor !== undefined && summary.cursor !== null,
    refetchOnWindowFocus: false, refetchOnReconnect: false, refetchOnMount: 'always' });
  useEffect(() => {
    const key = ['search', 'meaning-sources', readKey];
    if (!open) void cache.cancelQueries({ queryKey: key, exact: true });
    return () => { void cache.cancelQueries({ queryKey: key, exact: true }); };
  }, [cache, open, readKey]);
  const changeOpen = (next: boolean) => { if (next) setOpening((before) => before + 1); setOpen(next); };
  const page = read.isError ? undefined : read.data?.pages.at(-1);
  const unreported = page?.unreported ?? summary?.unreported ?? true;
  const groups: FactGroup[] = [{ key: 'receipt', kind: 'recorded_outcome', facts: [
    ...(reason === '' ? [] : [{ key: 'reason', label: t('search.sources.reason'), value: reason }]),
    { key: 'indexes', label: t('search.sources.indexes'), value: unreported || summary === null ? null : formatNumber(page?.total ?? summary.total, locale) },
  ] }];
  if (open && !read.isError) for (const [index, source] of (read.data?.pages.flatMap((one) => one.sources) ?? []).entries()) {
    groups.push({ key: `source:${index}`, facts: [
      { key: 'root', label: t('search.sources.indexFor'), value: source.root.redacted || source.root.name === null ? t('search.sources.notShared') : <KnownNodeChip node={source.root} /> },
      { key: 'model', label: t('search.sources.model'), value: <ServingModel model={source.model} /> },
      { key: 'usage', label: t('search.sources.usage'), value: source.cached ? t('search.sources.cached') : source.usage_owner_withheld ? t('search.sources.notShared') : source.usage_owner === null ? null : <UserChip user={source.usage_owner} /> },
      { key: 'observed', label: t('search.sources.observed'), value: <RelativeTime at={source.observed_at} /> },
    ] });
  }
  const fragment = label === undefined ? undefined : <>{label}{inlineReason !== '' && <> · {inlineReason}</>}</>;
  const name = label === undefined ? t('common.details') : `${label}${inlineReason === '' ? '' : ` · ${inlineReason}`}`;
  return <span className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1">
    {label === undefined && summary !== null && !summary.unreported && summary.total > 0 && <span className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1">
      {t('search.sources.meaning')} · {summary.single === null ? t('search.sources.count', { count: summary.total, number: formatNumber(summary.total, locale) }) : <ServingModel model={summary.single} />}
    </span>}
    <FactDisclosure label={name} fragment={fragment} groups={groups} open={open} onOpenChange={changeOpen}
      header={open && (summary?.total ?? 0) > 10 ? <Input label={t('search.sources.find')} hideLabel placeholder={t('search.sources.find')} value={text} onChange={(event) => setText(event.target.value)} /> : undefined}>
      {open && summary?.cursor !== undefined && summary.cursor !== null && <>
        {read.data === undefined && !read.isError && <SkeletonRows read={read} />}
        {read.isError && !read.isFetchNextPageError && <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => { void read.refetch(); }}>{t('common.tryAgain')}</Button>} />}
        <Pagination hasMore={read.hasNextPage} loading={read.isFetchingNextPage} error={read.isFetchNextPageError ? read.error : null} onMore={() => { void read.fetchNextPage(); }} />
      </>}
    </FactDisclosure>
  </span>;
}
