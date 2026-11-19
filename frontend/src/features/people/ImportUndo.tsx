/** Undo first shows the server's current plan, including later additions and what stays. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { client } from '@/api/client';
import { importKeys, planImportUndo, startImportUndo } from '@/api/imports';
import { importReason } from '@/features/people/importReason';
import type { ImportCounts, ImportUndoPlan, Job } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { NetworkError } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';

/** The effects that can change while the person is reading the plan. */
export function sameUndoEffects(one: ImportUndoPlan, other: ImportUndoPlan): boolean {
  return JSON.stringify(undoEffects(one)) === JSON.stringify(undoEffects(other));
}

function undoEffects(plan: ImportUndoPlan) { return [plan.counts, plan.merged, plan.added_since, plan.edited_since, plan.blockers]; }

/** The nonzero kinds of work the plan says would be removed. */
export function undoCounts(counts: ImportCounts, merged: ImportCounts, t: TFunction, locale: string): string {
  const amounts = {
    places: counts.places, folders: counts.folders, documents: counts.documents, chats: counts.chats, threads: counts.threads,
    messages: merged.messages, turns: merged.turns, comments: merged.comments, versions: merged.versions,
  };
  return (Object.entries(amounts) as [keyof typeof amounts, number][]).filter(([, count]) => count > 0)
    .map(([kind, count]) => t(`importWizard.undo.${kind}`, { count, number: formatNumber(count, locale) })).join(' · ');
}

export function ImportUndo({ id, onClose, onStarted }: { id: string; onClose: () => void; onStarted: (job: Job) => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const cache = useQueryClient();
  const key = useActKey();
  const sent = useRef<string | null>(null);
  const alive = useRef(true);
  const owner = useRef({ user: client.session.getState().userId, reader: client.session.getState().readerVersion ?? 0 });
  const owns = () => {
    const session = client.session.getState();
    return alive.current && session.userId === owner.current.user && (session.readerVersion ?? 0) === owner.current.reader;
  };
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const [changed, setChanged] = useState(false);
  const plan = useQuery({ queryKey: importKeys.undo(id), queryFn: ({ signal }) => planImportUndo(id, signal), staleTime: 0, retry: false });
  const act = useMutation({
    mutationFn: async () => {
      if (!owns()) throw new Error('Import reader changed');
      const send = async (token: string) => {
        try { return { kind: 'started' as const, job: await startImportUndo(id, token) }; }
        catch (error) { if (!(error instanceof NetworkError)) sent.current = null; throw error; }
      };
      if (sent.current !== null) return send(sent.current);
      if (plan.data === undefined) throw new Error('Undo plan missing');
      const latest = await planImportUndo(id);
      if (!owns()) throw new Error('Import reader changed');
      if (!sameUndoEffects(plan.data, latest)) return { kind: 'changed' as const, plan: latest };
      sent.current = key({ id, effects: undoEffects(latest) });
      return send(sent.current);
    },
    onSuccess: (result) => {
      if (!owns()) return;
      if (result.kind === 'changed') { cache.setQueryData(importKeys.undo(id), result.plan); setChanged(true); return; }
      onStarted(result.job);
      onClose();
    },
  });
  const value = plan.data;
  const uncertain = act.error instanceof NetworkError;
  const counts = value === undefined ? '' : undoCounts(value.counts, value.merged, t, locale);
  return <ConfirmDialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('importWizard.undo.title')}
    confirmLabel={t(uncertain ? 'importWizard.dialog.retry' : 'importWizard.undo.action')} danger pending={act.isPending} disabled={!uncertain && (value === undefined || plan.isFetching || value.blockers.length > 0)}
    error={act.error} onConfirm={() => act.mutate()}>
    {plan.isPending ? <SkeletonRows rows={3} /> : plan.isError ? <ErrorAlert error={plan.error} action={<Button size="sm" onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>} />
      : value !== undefined && <div className="flex flex-col gap-2">
        <p>{counts === '' ? t('importWizard.undo.none') : t('importWizard.undo.effect', { counts })}</p>
        <p>{t('importWizard.undo.stays')}</p>
        {value.added_since > 0 && <p>{t('importWizard.undo.added', { count: value.added_since, number: formatNumber(value.added_since, locale) })}</p>}
        {value.edited_since > 0 && <p>{t('importWizard.undo.edited', { count: value.edited_since, number: formatNumber(value.edited_since, locale) })}</p>}
        <p>{t('importWizard.undo.restoreUntil')} <RelativeTime at={value.restore_until} /></p>
        {changed && <InlineAlert tone="warning">{t('importWizard.undo.changed')}</InlineAlert>}
        {uncertain && <InlineAlert tone="warning">{t('importWizard.dialog.notConfirmed')}</InlineAlert>}
        {value.blockers.map((blocker, index) => <InlineAlert key={`${blocker.code}:${index}`} tone="danger">{importReason(blocker, t, locale)}</InlineAlert>)}
      </div>}
  </ConfirmDialog>;
}
