/** The New thread location field, mounted again for an existing thread and its directory. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { placesQuery } from '@/api/forks';
import { ChangeDirectory } from '@/features/change/Directory';
import type { WhereChoice } from '@/features/change/choices';
import { openChange } from '@/features/change/target';
import type { PlaceEnvironment, Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { PlacePicker, type PlaceRow } from './PlacePicker';

export function ThreadWhere({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const me = useStore(client.session, (state) => state.userId);
  const [open, setOpen] = useState(false);
  const [directory, setDirectory] = useState<string | null>(null);
  const places = useQuery({ ...placesQuery(thread.topic_id), enabled: open });
  const environment = thread.environment;
  const mayChange = !thread.archived && holds(thread.access, 'thread.manage');
  const refusal = mayChange ? null : t(thread.archived ? 'start.where.threadArchived' : 'start.where.manageThread');
  if (environment === null) return null;
  const listed = (places.data ?? []).filter((row): row is PlaceEnvironment => 'environment' in row);
  const rows: PlaceRow[] = listed.map((row) => ({
    id: row.environment.id, name: row.environment.name, on: row.machine?.name ?? row.host?.name ?? null,
    owner: row.environment.owner.id === me ? null : row.environment.owner,
    state: row.usable && row.environment.status !== 'running' ? t(`environments.status.${row.environment.status}`) : null,
    status: row.environment.status, group: row.usable && mayChange ? 'ready' : 'unavailable',
    reason: refusal ?? (row.usable ? null : row.reasons.length === 0 ? t('uniformMoves.unavailable') : row.reasons.map((reason) => t(`change.where.envReason.${reason}`)).join(t('startFlow.separator'))),
    keywords: [row.environment.address, row.environment.owner.display_name],
  }));
  if (!rows.some((row) => row.id === environment.id)) rows.unshift({ id: environment.id, name: environment.name, on: null,
    owner: environment.owner.id === me ? null : environment.owner, state: null, status: environment.status, group: mayChange ? 'ready' : 'unavailable', reason: refusal });
  const review = (where?: WhereChoice) => {
    setOpen(false);
    openChange({ kind: 'thread', id: thread.id }, { kind: 'switch', focus: 'environment',
      placement: { ...(where === undefined ? {} : { where }), ...(directory === null ? {} : { directory }) } });
  };
  return <PlacePicker label={t('start.where.title')} value={environment.id} rows={rows} size="sm" fieldClassName="min-w-0 max-w-full"
    open={open} onOpenChange={setOpen}
    onPick={(id) => { if (mayChange && id !== environment.id) review({ kind: 'use', id, files: { mode: 'all' } }); }}
    {...(mayChange ? { newRow: { onNew: () => review({ kind: 'new', hostId: null, name: '', files: { mode: 'all' }, home: false, privateViews: true, onlyMe: false }) } } : {})}
    footer={<div className="flex min-w-0 flex-col gap-2">
      <span className="truncate text-sm text-text-muted" title={thread.directory}>{thread.directory || t('change.directory.root')}</span>
      {places.isPending && <SkeletonRows read={places} rows={1} />}
      {places.isError && <ErrorAlert error={places.error} action={<Button size="sm" onClick={() => void places.refetch()}>{t('common.tryAgain')}</Button>} />}
      {mayChange && <><ChangeDirectory value={directory} current={thread.directory} door={{ environmentId: environment.id, threadId: thread.id }} onChange={setDirectory} />
        {directory !== null && <Button size="sm" className="self-end" onClick={() => review()}>{t('start.where.review')}</Button>}</>}
    </div>} />;
}
