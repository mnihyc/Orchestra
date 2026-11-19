/** One scoped audit trail for places and Management, with URL filters and continuous day or kind groups. */
import { useViewLocation, useViewNavigate } from '@/features/workbench/pageContext';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Download, RefreshCw, RotateCcw, ScrollText, SlidersHorizontal, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { ApiError } from '@/api/client';
import { downloadTrail, selectedPersonQuery, trailQuery, wireFilters } from '@/api/trace';
import { saveFile } from '@/features/export/save';
import type { AuditExportFormat, AuditEntry as TraceEntry } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useListChoice } from '@/lib/listState';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { FilterBar } from '@/ui/FilterBar';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { RequestRow } from './Requests';
import { requestGroups } from './requests';
import { RestoreDialog } from './Restore';
import { selectionKey } from './restore';
import { TraceFilters } from './Filters';
import { exportSpanValid, traceSearch, type GroupBy, type TraceScope, type TraceSearch } from './params';

function TrailRows({ entries, group, hasMore, select, refresh, scopeName }: { scopeName: string; entries: TraceEntry[]; group: GroupBy; hasMore: boolean; select: boolean; refresh: () => Promise<TraceEntry[]> }) {
  const { t } = useTranslation(); const format = useFormatContext(); const [picked, setPicked] = useState<Map<string, TraceEntry>>(new Map()); const [review, setReview] = useState<TraceEntry[] | null>(null);
  const groups = requestGroups(entries, group, format.timeZone, hasMore); const selected = new Set(picked.keys());
  const pick = (entry: TraceEntry, checked: boolean) => setPicked((current) => { const next = new Map(current); if (checked) next.set(selectionKey(entry), entry); else next.delete(selectionKey(entry)); return next; });
  return <>
    {picked.size > 0 && <div className="sticky top-0 z-10 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-surface p-3 shadow-sm" aria-label={t('trace.selection')}><span role="status" className="text-sm">{t('trace.selectedCount', { count: picked.size })}</span><div className="flex flex-wrap gap-2"><Button size="sm" icon={<RotateCcw aria-hidden />} onClick={() => setReview([...picked.values()])}>{t('trace.bringBack')}</Button><Button size="sm" variant="ghost" icon={<X aria-hidden />} onClick={() => setPicked(new Map())}>{t('trace.clearSelection')}</Button></div></div>}
    <div className="min-w-0 overflow-hidden rounded-lg border border-border bg-surface"><table aria-label={t('trace.title', { scope: scopeName })} className="w-full table-fixed text-left">{groups.map((part) => <tbody key={part.key}><tr><th scope="colgroup" className="border-y border-border bg-bg-subtle px-3 py-2 text-sm font-medium"><h3>{group === 'kind' ? t(`trace.kinds.${part.key}`, { defaultValue: t('trace.kinds.unknown') }) : new Intl.DateTimeFormat(format.locale, { dateStyle: 'long', timeZone: format.timeZone }).format(new Date(part.lines[0]?.chosen.at ?? ''))}</h3></th></tr>{part.lines.map((line) => <tr key={line.key} className="border-b border-border last:border-b-0"><td><ul><RequestRow line={line} select={select} selected={selected} onSelect={pick} onRestore={(entry) => setReview([entry])} /></ul></td></tr>)}</tbody>)}</table></div>
    {review !== null && <RestoreDialog key={review.map(selectionKey).sort().join('|')} selected={review} refresh={refresh} onIncludeParent={(parent) => { const next = [...review, parent]; setPicked(new Map(next.map((entry) => [selectionKey(entry), entry]))); setReview(next); }} onClose={() => { setReview(null); setPicked(new Map()); }} />}
  </>;
}

function DownloadDialog({ scope, filters, onClose, onLost }: { scope: TraceScope; filters: TraceSearch; onClose: () => void; onLost: () => void }) {
  const { t } = useTranslation(); const [format, setFormat] = useState<AuditExportFormat>('csv'); const [working, setWorking] = useState(false); const [error, setError] = useState<unknown>(null);
  const valid = exportSpanValid(filters.from, filters.to);
  const download = async () => { if (working || !valid) return; setWorking(true); setError(null); try { const blob = await downloadTrail(scope, filters, format); saveFile(blob, `audit.${format}`); onClose(); } catch (failure) { setError(failure); if (failure instanceof ApiError && failure.status === 403) onLost(); } finally { setWorking(false); } };
  return <Dialog open title={t('trace.downloadTitle')} description={t('trace.downloadAbout')} onOpenChange={(open) => !open && !working && onClose()} dismissible={!working} footer={<><Button disabled={working} onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" icon={<Download aria-hidden />} blocked={!valid} pending={working} onClick={() => void download()}>{t('trace.download')}</Button></>}><div className="flex flex-col gap-3 py-2"><Select label={t('trace.format')} value={format} onValueChange={setFormat} options={[{ value: 'csv', ...choiceOption(t, 'traceFormat.csv') }, { value: 'jsonl', ...choiceOption(t, 'traceFormat.jsonl') }]} /><p className="text-sm wrap-anywhere text-text-muted">{t('trace.downloadFilters', { from: filters.from ?? t('trace.unset'), to: filters.to ?? t('trace.unset') })}</p>{!valid && <p role="status" className="text-sm text-warning-text">{t('trace.spanRequired')}</p>}{error !== null && <ErrorAlert error={error} message={t('trace.incomplete')} />}</div></Dialog>;
}

export function Trail({ scope, embedded = false }: { scope: TraceScope; embedded?: boolean }) { return <ScopedTrail key={scope.type === 'system' ? 'system' : scope.id} scope={scope} embedded={embedded} />; }
function ScopedTrail({ scope, embedded }: { scope: TraceScope; embedded: boolean }) {
  const { t } = useTranslation(); const location = useViewLocation(); const navigate = useViewNavigate(); const filters = traceSearch(location.search);
  const [remembered, setRemembered] = useListChoice<GroupBy>('trace', 'group', 'day');
  // A first phone visit starts with the trail; this page keeps its own controls.
  const [filtersOpen, setFiltersOpen] = useListChoice('trace', 'filters', window.matchMedia('(min-width: 640px)').matches);
  const group = filters.group ?? remembered;
  const list = useInfiniteQuery(trailQuery(scope, filters)); const [downloading, setDownloading] = useState(false); const [lost, setLost] = useState(false);
  const denied = lost || (list.error instanceof ApiError && (list.error.status === 403 || list.error.status === 404));
  const entries = denied ? [] : (list.data?.pages.flatMap((page) => page.items) ?? []);
  const person = entries.flatMap((entry) => entry.actor.kind === 'user' ? [entry.actor.user] : entry.actor.kind === 'agent' && entry.actor.for_user ? [entry.actor.for_user] : []).find((user) => user.id === filters.actor);
  const chosen = useQuery({ ...selectedPersonQuery(filters.actor ?? ''), enabled: !denied && filters.actor !== undefined });
  const selected = chosen.data ?? person ?? null;
  const filtered = ['actor', 'action', 'kind', 'from', 'to'].some((key) => filters[key as keyof TraceSearch] !== undefined);
  const update = (next: TraceSearch) => {
    const keys = ['actor', 'include_agents', 'action', 'kind', 'from', 'to', 'group'] as const;
    const search: Record<string, unknown> = Object.fromEntries(Object.entries(location.search).filter(([key]) => !keys.some((filter) => filter === key)));
    for (const key of keys) if (next[key] !== undefined && next[key] !== '') search[key] = next[key];
    void navigate({ to: location.pathname, search, replace: true });
  };
  const refresh = async () => { const answer = await list.refetch(); if (answer.error) throw answer.error; return answer.data?.pages.flatMap((page) => page.items) ?? []; };
  const content = <div className="@container flex min-w-0 flex-col gap-4">
      {denied ? <ErrorState compact error={list.error ?? new Error('audit authority was lost')} title={t('trace.lostAccess')} onRetry={() => { setLost(false); void list.refetch(); }} /> : <>
        <details open={filtersOpen} onToggle={(event) => setFiltersOpen(event.currentTarget.open)} className="min-w-0"><summary className="mb-3 flex w-fit cursor-default items-center gap-2 rounded-md px-2 py-1 text-sm text-text-muted hover:bg-bg-muted"><SlidersHorizontal aria-hidden />{t('trace.filters')}{filtered && <Badge>{t('trace.filtered')}</Badge>}</summary><TraceFilters scope={scope} filters={filters} person={selected} onChange={update} /></details>
        <FilterBar label={t('trace.filters')} action={<>{filtered && <Button size="sm" variant="ghost" icon={<X aria-hidden />} onClick={() => update({ group })}>{t('trace.clearFilters')}</Button>}<Button size="sm" icon={<RefreshCw aria-hidden />} pending={list.isFetching && !list.isPending} onClick={() => void list.refetch()}>{t('trace.refresh')}</Button>{!list.isPending && !list.isError && <Button size="sm" icon={<Download aria-hidden />} onClick={() => setDownloading(true)}>{t('trace.download')}</Button>}</>}>
          <Select label={t('trace.group')} value={group} onValueChange={(next) => { setRemembered(next); update({ ...filters, group: next }); }} options={[{ value: 'day', ...choiceOption(t, 'traceGroup.day') }, { value: 'kind', ...choiceOption(t, 'traceGroup.kind') }]} />
        </FilterBar>
        {list.isPending ? <SkeletonRows rows={4} /> : list.isError && entries.length === 0 ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} retrying={list.isRefetching} /> : entries.length === 0 ? <EmptyState compact icon={<ScrollText aria-hidden />} title={t(filtered ? 'trace.noMatch' : 'trace.empty')} /> : <><TrailRows key={JSON.stringify(wireFilters(filters))} entries={entries} group={group} hasMore={list.hasNextPage} select={filters.kind === 'deleted'} refresh={refresh} scopeName={scope.name} /><Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} /></>}
      </>}
      {downloading && !denied && <DownloadDialog scope={scope} filters={filters} onClose={() => setDownloading(false)} onLost={() => { setLost(true); setDownloading(false); }} />}
    </div>;
  return embedded ? content : <Section id="audit" title={t('trace.title', { scope: scope.name })} lead={filters.actor ? t('trace.forPerson', { name: selected?.display_name ?? t('trace.selectedPerson') }) : t('trace.lead')}>{content}</Section>;
}
