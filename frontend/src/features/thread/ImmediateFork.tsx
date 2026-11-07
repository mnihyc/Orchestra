/** Confirm only the new holder's contribution; the existing quick fork keeps its current audience rules. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { planFork } from '@/api/forks';
import { initialChoices, requestOf } from '@/features/change/choices';
import type { AccessSummary, AgentAccessChoice, Thread } from '@/generated/api-types';
import { NewAudienceSummary } from '@/features/access/NewPage';
import { CreationAgentAccess } from '@/features/start/AgentAccess';
import { offeredAgentAccess, useAgentAccessSelection } from '@/features/start/agentAccessOffer';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';
import { useForkNow } from './forkNow';

function CurrentAudience({ thread, summary }: { thread: Thread; summary: AccessSummary | undefined }) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-2">
    <p className="text-sm text-text-muted">{t('agentAccess.forkAudience', { name: thread.name })}</p>
    {summary === undefined ? <SkeletonRows rows={2} /> : <NewAudienceSummary summary={summary} place={thread.name} />}
  </div>;
}

function ImmediateForkChoice({ thread, through, onConfirm, onClose }: {
  thread: Thread; through: number | undefined; onConfirm: (choice: AgentAccessChoice) => void; onClose: () => void;
}) {
  const { t } = useTranslation();
  const selection = useAgentAccessSelection(thread.id);
  const request = { ...requestOf(initialChoices('new', through ?? null)), ...(selection.chosen === null ? {} : { agent_access: selection.chosen }) };
  const preview = useQuery({ queryKey: ['immediate-fork-access', thread.id, request], queryFn: ({ signal }) => planFork(thread.id, request, signal), retry: false });
  const access = offeredAgentAccess(selection, { offer: preview.data?.agent_access, pending: preview.isPending || preview.isFetching,
    error: preview.error, retry: () => void preview.refetch() });
  if (selection.chosen === null && preview.isSuccess && !preview.isFetching && preview.data.agent_access !== undefined && preview.data.agent_access !== null) selection.onChange(access.value);
  const reason = access.pending ? t('ui.unavailable.checkingFork') : preview.isError || access.unavailable || selection.chosen === null ? t('ui.unavailable.forkUnavailable') : access.blocked ? t('agentAccess.unavailable') : undefined;
  return <Dialog open title={t('agentAccess.forkTitle', { name: thread.name })} onOpenChange={(open) => { if (!open) onClose(); }}
    footer={<><Button onClick={onClose}>{t('common.cancel')}</Button><Button variant="primary" disabled={!access.ready || selection.chosen === null || preview.isError} reason={reason} onClick={() => onConfirm(access.value)}>{t('change.menu.forkNow')}</Button></>}>
    <div className="flex flex-col gap-4">
      <CurrentAudience thread={thread} summary={preview.data?.access.summary} />
      {through !== undefined && <p className="text-sm text-text-muted">{t('agentAccess.forkThrough', { turn: through })}</p>}
      <CreationAgentAccess access={access} />
    </div>
  </Dialog>;
}

/** Both existing immediate-fork owners mount this one dialog; closed owners run no creation readers. */
export function useImmediateFork(thread: Thread | null) {
  const actor = useStore(client.session, (state) => state.userId);
  const readerVersion = useStore(client.session, (state) => state.readerVersion ?? 0);
  const fork = useForkNow(thread?.id ?? '');
  const [opening, setOpening] = useState<{ actor: string | null; readerVersion: number; thread: string; through: number | undefined } | null>(null);
  const shown = thread !== null && opening?.actor === actor && opening.readerVersion === readerVersion && opening.thread === thread.id ? opening : null;
  return { pending: fork.pending, start: (through?: number) => { if (thread !== null) setOpening({ actor, readerVersion, thread: thread.id, through }); },
    confirmation: shown === null || thread === null ? null : <ImmediateForkChoice key={thread.id} thread={thread} through={shown.through}
      onClose={() => setOpening(null)} onConfirm={(choice) => { fork.start(shown.through, choice); setOpening(null); }} />,
  };
}
