/** The thread's history, newest first, narrowed by kind and person. */
import { useListChoice } from '@/lib/listState';
import { useInfiniteQuery } from '@tanstack/react-query';
import { History as HistoryIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { historyQuery } from '@/api/access';
import { ApiError } from '@/api/client';
import { openAccess } from '@/features/access/AccessDialog';
import { openFile } from '@/features/files/FileTabs';
import { PlaceLink } from '@/features/navigation/places';
import { kindOf } from '@/features/trace/catalogue';
import { PersonLocator } from '@/features/people/PersonLocator';
import type { AuditKind, Thread, UserRef } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { HistoryLines } from '@/ui/HistoryLines';
import { Button } from '@/ui/Button';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ThreadPanel } from './ThreadPanel';

function History({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const [depth, setDepth] = useListChoice<'summary' | 'every'>('inspector-history', 'depth', 'summary');
  const [kind, setKind] = useListChoice<AuditKind | ''>('inspector-history', 'kind', '');
  const [person, setPerson] = useListChoice<UserRef | null>('inspector-history', 'person', null);
  const query = useInfiniteQuery(historyQuery({ kind: 'thread', id: thread.id }, depth, person?.id));
  if (query.error instanceof ApiError && (query.error.status === 403 || query.error.status === 404)) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const lines = query.data?.pages.flatMap((page) => page.items) ?? [];
  const actions = [...new Set([...(kind === '' ? [] : [kind]), ...lines.flatMap((line) => { const kind = kindOf(line.action); return kind === undefined ? [] : [kind]; })])];
  const shown = lines.filter((line) => kind === '' || kindOf(line.action) === kind);
  return <div className="flex h-full min-h-0 flex-col" data-testid="thread-history">
    <div className="flex flex-wrap gap-2 border-b border-border p-3">
      <SegmentedControl label={t('places.activity.depth')} value={depth} onValueChange={setDepth} segments={[{ value: 'summary', ...choiceOption(t, 'activityDepth.summary') }, { value: 'every', ...choiceOption(t, 'activityDepth.every') }]} />
      <Select<AuditKind | ''> label={t('threadViews.history.kind')} value={kind} onValueChange={setKind} options={[{ value: '', ...choiceOption(t, 'statusFilter.any') }, ...actions.map((action) => ({ value: action, ...choiceOption(t, 'dynamic.value', { name: t(`trace.kinds.${action}`), context: t('threadViews.history.kind') }) }))]} />
      {/* A directly shared thread does not confer View of its org; use the current reader's normal people lookup. */}
      <PersonLocator label={t('threadViews.history.person')} value={person?.id ?? null} person={person} onChoose={setPerson} />
    </div>
    <div className="min-h-0 flex-1 overflow-auto p-3">
      {query.data === undefined ? query.error === null ? <SkeletonRows rows={5} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} retrying={query.isRefetching} /> : <>
      {shown.length === 0 && <EmptyState compact icon={<HistoryIcon aria-hidden />} title={t(person !== null || kind !== '' ? 'threadViews.noMatches' : 'threadViews.history.empty')} />}
      <HistoryLines lines={shown} hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.error} onMore={() => void query.fetchNextPage()} renderDetails={(line) => {
        const other = line.about?.thread;
        const path = line.about?.path;
        const environmentId = thread.environment?.id;
        return <div className="flex flex-wrap items-center gap-2 text-sm">
          {other !== undefined && other !== null && !other.redacted && other.name !== null && <PlaceLink place={other} className="text-link hover:underline">{other.name}</PlaceLink>}
          {typeof path === 'string' && environmentId !== undefined && thread.environment_permissions.includes('files.view') && <Button size="sm" variant="ghost" onClick={() => openFile({ environmentId, threadId: thread.id, path, permanent: true })}>{t('threadViews.history.openFile', { path })}</Button>}
          {(line.action.startsWith('permission.') || line.action === 'node.transferred') && <Button size="sm" variant="ghost" onClick={() => openAccess({ kind: 'thread', id: thread.id, name: thread.name })}>{t('threadViews.people.access')}</Button>}
        </div>;
      }} /></>}
    </div>
  </div>;
}

export function HistoryView(_props: ViewProps) { return <ThreadPanel>{(thread) => <History key={thread.id} thread={thread} />}</ThreadPanel>; }
