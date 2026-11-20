/** The current session's native catalogue owns one opening; it is never reconstructed from present settings. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { orchToolsQuery } from '@/api/threads';
import type { OrchToolsSummary, OrchToolUnavailable } from '@/generated/api-types';
import { formatNumber } from '@/lib/format';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { FactDisclosure, FactDisclosureBody } from '@/ui/FactDisclosure';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { SkeletonRows } from '@/ui/Skeleton';

export function OrchToolsFacts({ thread, summary, authority = '' }: { thread: string; summary?: OrchToolsSummary | null; authority?: string }) {
  const reader = useStore(client.session, (state) => `${state.status}/${state.userId ?? ''}/${state.readerVersion}`);
  return <CurrentTools key={`${thread}/${summary?.session_revision ?? ''}/${reader}/${authority}`} thread={thread} summary={summary} reader={`${reader}/${authority}`} />;
}

function CurrentTools({ thread, summary, reader }: { thread: string; summary?: OrchToolsSummary | null; reader: string }) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const query = useCatalogueRead(thread, summary?.session_revision ?? null, search, reader, open);
  const current = query.data === undefined ? summary : query.data.pages[0]?.summary;
  const locale = i18n.resolvedLanguage ?? i18n.language;
  return <FactDisclosure open={open} onOpenChange={setOpen} fragment={current === undefined || current === null ? undefined : t('orchToolFacts.offeredCount', { count: current.offered_count, n: formatNumber(current.offered_count, locale) })}
    groups={[{ key: 'accepted-tools', kind: 'recorded_outcome', facts: [
      { key: 'count', label: t('orchToolFacts.title'), value: current === undefined || current === null ? undefined : formatNumber(current.offered_count, locale) },
      { key: 'at', label: t('orchToolFacts.at'), value: open && current !== undefined && current !== null ? <RelativeTime at={current.accepted_at} /> : undefined },
    ] }]}>
    {open && <ToolCatalogue thread={thread} query={query} search={search} onSearch={setSearch} />}
  </FactDisclosure>;
}

function useCatalogueRead(thread: string, revision: string | null, search: string, reader: string, open: boolean) {
  return useInfiniteQuery({ ...orchToolsQuery(thread, revision, search, reader), enabled: open, placeholderData: (previous) => previous });
}

function ToolCatalogue({ thread, query, search, onSearch }: { thread: string; query: ReturnType<typeof useCatalogueRead>; search: string; onSearch: (value: string) => void }) {
  const { t } = useTranslation();
  const reasons = { not_built: 'orchToolFacts.reasons.build', off_here: 'orchToolFacts.reasons.off', read_only: 'orchToolFacts.reasons.readOnly', no_tools: 'orchToolFacts.reasons.none', goal_required: 'orchToolFacts.reasons.goal', capability_unavailable: 'orchToolFacts.reasons.capability', program_unsupported: 'orchToolFacts.reasons.program', environment_tools_off: 'orchToolFacts.reasons.environment', capability_work: 'orchToolFacts.reasons.work' } satisfies Record<OrchToolUnavailable, string>;
  const rows = query.data?.pages.flatMap((page) => page.tools.items) ?? [];
  const source = rows.find((row) => row.source !== null)?.source;
  return <div className="flex min-w-0 flex-col gap-2">
    <Input label={t('orchToolFacts.search')} hideLabel placeholder={t('orchToolFacts.search')} value={search} onChange={(event) => onSearch(event.target.value)} />
    {query.isError ? <ErrorState compact error={query.error} onRetry={() => { void query.refetch(); }} />
      : query.isPending || query.isRefetching ? <SkeletonRows read={query} rows={3} />
      : <>
        {query.data.pages[0]?.summary === null && <span>{t('factDisclosure.notReported')}</span>}
        <ScrollRegion label={t('orchToolFacts.title')} kind="table" className="max-h-(--list-max-height) overflow-y-auto">
          <FactDisclosureBody groups={[{ key: 'tools', facts: rows.map((row) => ({ key: row.key,
            label: t(`settings.editor.words.${row.key}`, { defaultValue: row.key }), value: row.offered ? t('orchToolFacts.offered') : row.reason === null ? t('factDisclosure.notReported') : <span>{t('orchToolFacts.unavailable')} · <span>{t(reasons[row.reason])}</span></span>,
          })) }]} />
        </ScrollRegion>
        {query.hasNextPage && <Button size="sm" pending={query.isFetchingNextPage} onClick={() => { void query.fetchNextPage(); }}>{t('orchToolFacts.more')}</Button>}
        {rows.length > 0 && <FactDisclosureBody groups={[{ key: 'tool-source', facts: [{ key: 'source', label: t('orchToolFacts.source'), value: source === undefined || source === null ? t('settings.compact.hiddenSource') : <SourceSettingLink source={source} def={{ key: 'orch_tools.enabled', class: 'policy' }} target={{ kind: 'node', id: thread }} fallback={t('factDisclosure.notReported')} /> }] }]} />}
      </>}
  </div>;
}
