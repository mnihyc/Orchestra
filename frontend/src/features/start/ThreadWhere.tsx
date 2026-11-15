/** The New thread location field, mounted again for an existing thread and its directory. */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { placesQuery } from '@/api/forks';
import { nodeQuery } from '@/api/nodes';
import { ChangeDirectory } from '@/features/change/Directory';
import type { WhereChoice } from '@/features/change/choices';
import { openChange } from '@/features/change/target';
import type { PlaceEnvironment, Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { PromptScope, usePrompt } from '@/ui/PromptScope';
import { StateLine } from '@/ui/StateLine';
import { SkeletonRows } from '@/ui/Skeleton';
import { PlacePicker, type PlaceRow } from './PlacePicker';

export function ThreadWhere({ thread, autoOpen = false }: { thread: Thread; autoOpen?: boolean }) {
  const { t } = useTranslation();
  const me = useStore(client.session, (state) => state.userId);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const scope = JSON.stringify([thread.id, me, reader]);
  const [open, setOpen] = useState(autoOpen);
  const field = useRef<HTMLDivElement>(null);
  useEffect(() => { if (autoOpen) field.current?.querySelector<HTMLInputElement>('[role="combobox"]')?.focus(); }, [autoOpen]);
  const [draft, setDraft] = useState<{ scope: string; directory: string | null }>({ scope, directory: null });
  const directory = draft.scope === scope ? draft.directory : null;
  if (draft.scope !== scope) { setDraft({ scope, directory: null }); setOpen(false); }
  const environment = thread.environment;
  const mayChange = !thread.archived && holds(thread.access, 'thread.manage') && thread.side_of === null && thread.lock === null;
  const places = useQuery({ ...placesQuery(thread.topic_id), enabled: open && mayChange });
  const refusal = mayChange ? null : !holds(thread.access, 'thread.manage') ? t('start.where.manageThread') : thread.archived ? t('start.where.threadArchived')
    : thread.side_of !== null ? t('composer.row.sideBinding') : t('threadMove.jobMustEnd');
  if (environment === null) return null;
  const listed = (mayChange ? places.data ?? [] : []).filter((row): row is PlaceEnvironment => 'environment' in row);
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
  return <div ref={field} className="min-w-0"><PlacePicker label={t('start.where.title')} value={environment.id} rows={rows} size="sm" fieldClassName="min-w-0 max-w-full"
    open={open} onOpenChange={setOpen}
    onPick={(id) => { if (mayChange && id !== environment.id) review({ kind: 'use', id, files: { mode: 'all' } }); }}
    {...(mayChange ? { newRow: { onNew: () => review({ kind: 'new', hostId: null, name: '', files: { mode: 'all' }, home: false, privateViews: true, onlyMe: false }) } } : {})}
    footer={<div className="flex min-w-0 flex-col gap-2">
      <span className="truncate text-sm text-text-muted" title={thread.directory}>{thread.directory || t('change.directory.root')}</span>
      {mayChange && places.isPending && <SkeletonRows read={places} rows={1} />}
      {mayChange && places.isError && <ErrorAlert error={places.error} action={<Button size="sm" onClick={() => void places.refetch()}>{t('common.tryAgain')}</Button>} />}
      {mayChange && <><ChangeDirectory value={directory} current={thread.directory} door={{ environmentId: environment.id, threadId: thread.id }} onChange={(directory) => setDraft({ scope, directory })} />
        {directory !== null && <Button size="sm" className="self-end" onClick={() => review()}>{t('start.where.review')}</Button>}</>}
    </div>} /></div>;
}

/** The directory command opens the same field, with the current thread read under current authority. */
export function ThreadWhereDialog({ threadId, open, onClose }: { threadId: string; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const layer = usePrompt(open, 'scope');
  const node = useQuery({ ...nodeQuery(threadId), enabled: open });
  const thread = node.data?.kind === 'thread' ? node.data : null;
  return <PromptScope layer={layer}><Dialog open={open} onOpenChange={(shown) => { if (!shown) onClose(); }} title={t('change.directory.label')}
    footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    {thread !== null ? thread.environment === null ? <StateLine compact message={t('thread.environment.none')} /> : <ThreadWhere key={thread.id} thread={thread} autoOpen />
      : node.isPending ? <SkeletonRows read={node} rows={3} /> : <ErrorState compact error={node.error} onRetry={() => void node.refetch()} />}
  </Dialog></PromptScope>;
}
