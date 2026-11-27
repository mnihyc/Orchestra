/** Selected places move, archive and come back at once: the result is one notice with Undo, a refusal stays named by its row. */
import { useMutation, useQueries } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { applyArchivePlan, moveNode, previewArchive } from '@/api/nodes';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import type { NodeArchivePlan, Topic } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { toast } from '@/ui/Toast';
import { ReviewBlockers } from './ReviewConsequences';
import { StopLine, stoppedWork } from './archiving';

/** `uncertain`: the write was sent and its answer lost, so it may have gone through. */
interface Refused { row: Topic; error: unknown; uncertain?: boolean }
const refusal = (row: Topic, error: unknown): Refused => ({ row, error, uncertain: !(error instanceof ApiError && error.status < 500) });

/** A refusal is the server's; a lost answer may have gone through, so Undo leaves it out and says so. */
function RefusedRows({ rows }: { rows: readonly Refused[] }) {
  const { t } = useTranslation();
  return rows.length === 0 ? null : <ul className="flex flex-col gap-2">{rows.map(({ row, error, uncertain }) => <li key={row.id} className="flex flex-col gap-1">
    <span className="font-medium wrap-anywhere">{row.name}</span><ErrorAlert error={error} />
    {uncertain === true && <InlineAlert tone="warning">{t('day100.batch.uncertain')}</InlineAlert>}
  </li>)}</ul>;
}

function Results({ count, rows, onClose }: { count: number; rows: readonly Refused[]; onClose: () => void }) {
  const { t } = useTranslation();
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('day100.batch.results', { count })} footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    <RefusedRows rows={rows} />
  </Dialog>;
}

/** Each place is written on its own revision; the notice's Undo moves back what moved. */
export function MovePlaces({ rows, onClose, onApplied }: { rows: readonly Topic[]; onClose: () => void; onApplied: () => void }) {
  const { t } = useTranslation();
  const [destination, setDestination] = useState<PickedPlace | null>(null);
  const [refused, setRefused] = useState<Refused[]>([]);
  const move = useMutation({
    mutationFn: async (to: PickedPlace) => {
      const moved: { row: Topic; from: string; revision: number }[] = [], failed: Refused[] = [];
      for (const row of rows) {
        try { const after = await moveNode('topic', row.id, to.id, row.revision, undefined, newActKey()); if (row.parent_id !== null) moved.push({ row, from: row.parent_id, revision: after.revision }); }
        catch (error: unknown) { failed.push(refusal(row, error)); }
      }
      return { moved, failed, to };
    },
    onSuccess: ({ moved, failed, to }) => {
      if (moved.length > 0) {
        onApplied();
        toast({ tone: 'success', title: t('day100.batch.moved', { count: moved.length, where: to.path }), action: { label: t('places.toast.undo'), run: () => void (async () => {
          let undone = 0;
          for (const { row, from, revision } of moved) {
            try { await moveNode('topic', row.id, from, revision, undefined, newActKey()); undone += 1; } catch { /* the notice below says that not all came back */ }
          }
          toast(undone === moved.length ? { tone: 'success', title: t('day100.batch.undone', { count: undone }) } : { tone: 'danger', title: t('placeReview.undoChanged') });
        })() } });
      }
      if (failed.length === 0) onClose();
      else setRefused(failed);
    },
  });
  if (move.isSuccess && refused.length > 0) return <Results count={rows.length} rows={refused} onClose={onClose} />;
  return <Dialog open dismissible={!move.isPending} onOpenChange={(open) => { if (!open && !move.isPending) onClose(); }} title={t('day100.batch.moveTitle', { count: rows.length })} footer={<>
    <Button disabled={move.isPending} onClick={onClose}>{t('common.cancel')}</Button>
    <Button variant="primary" blocked={destination === null} pending={move.isPending} onClick={() => { if (destination !== null) { setRefused([]); move.mutate(destination); } }}>{t('places.move.submit')}</Button>
  </>}>
    <div className="flex flex-col gap-4">
      <PlacePicker label={t('placeReview.chooseDestination')} value={destination} onChange={setDestination} only="project" rootOrgId={rows[0]?.org_id} />
      {move.isError && <ErrorAlert error={move.error} />}
    </div>
  </Dialog>;
}

/**
 * Archive or unarchive the selected places as soon as their plans are read. Only work an archive stops is said first, in one
 * line with the threads on Show; a refusal holds its own row back and the others still go.
 */
export function ArchivePlaces({ rows, archiving, onClose, onApplied }: { rows: readonly Topic[]; archiving: boolean; onClose: () => void; onApplied: () => void }) {
  const { t } = useTranslation();
  const [refused, setRefused] = useState<Refused[]>([]);
  const reads = useQueries({ queries: rows.map((row) => ({ queryKey: ['place-archive-plan', row.id, archiving], queryFn: () => previewArchive(row.id, archiving), retry: false, gcTime: 0 })) });
  const loaded = reads.every((read) => !read.isPending && !read.isFetching);
  const plans = reads.flatMap((read) => read.data === undefined || read.data.blockers.length > 0 ? [] : [read.data]);
  const blocked = reads.flatMap((read, index) => { const row = rows[index]; return row === undefined || read.data === undefined || read.data.blockers.length === 0 ? [] : [{ row, plan: read.data }]; });
  const unread = reads.flatMap((read, index) => { const row = rows[index]; return row !== undefined && read.isError ? [{ row, error: read.error }] : []; });
  const work = stoppedWork(plans);
  const apply = useMutation({
    mutationFn: async (ready: readonly NodeArchivePlan[]) => {
      const undo: NodeArchivePlan[] = [], failed: Refused[] = [];
      for (const plan of ready) {
        const row = rows.find((one) => one.id === plan.node.id);
        try { const result = await applyArchivePlan(plan, newActKey()); if (result.undo !== null) undo.push(result.undo); }
        catch (error: unknown) { if (row !== undefined) failed.push(refusal(row, error)); }
      }
      return { done: ready.length - failed.length, undo, failed };
    },
    onSuccess: ({ done, undo, failed }) => {
      if (done > 0) {
        onApplied();
        toast({ tone: 'success', title: t(archiving ? 'day100.batch.archivedMany' : 'day100.batch.unarchivedMany', { count: done }),
          ...(undo.length === 0 ? {} : { action: { label: t('places.toast.undo'), run: () => void Promise.allSettled(undo.map((plan) => applyArchivePlan(plan))).then((all) => {
            if (all.some((one) => one.status === 'rejected')) toast({ tone: 'danger', title: t('placeReview.undoChanged') });
          }) } }) });
      }
      if (failed.length === 0 && unread.length === 0 && blocked.length === 0) onClose();
      else setRefused([...failed, ...unread, ...blocked.map(({ row, plan }) => ({ row, error: plan.blockers[0] }))]);
    },
  });
  const started = useRef(false);
  const { mutate } = apply;
  const atOnce = loaded && work.length === 0 && plans.length > 0 && blocked.length === 0 && unread.length === 0;
  useEffect(() => {
    if (!atOnce || started.current) return;
    started.current = true;
    mutate(plans);
  }, [atOnce, mutate, plans]);
  const nothing = loaded && plans.length === 0 && blocked.length === 0 && unread.length === 0;
  useEffect(() => { if (nothing) onClose(); }, [nothing, onClose]);
  // Nothing is drawn while the plans are read or while an archive that stops nothing is made.
  if (apply.isSuccess) return refused.length > 0 ? <Results count={rows.length} rows={refused} onClose={onClose} /> : null;
  if (!loaded || atOnce && !apply.isError) return null;
  return <ConfirmDialog open onOpenChange={(open) => { if (!open && !apply.isPending) onClose(); }}
    title={t(archiving ? 'day100.batch.archiveTitle' : 'day100.batch.unarchiveTitle', { count: rows.length })}
    confirmLabel={t(archiving ? 'day100.batch.archiveConfirm' : 'day100.batch.unarchiveConfirm', { count: plans.length })}
    pending={apply.isPending} disabled={plans.length === 0} error={apply.error ?? undefined} onConfirm={() => apply.mutate(plans)}>
    <div className="flex flex-col gap-3">
      <StopLine work={work} />
      {blocked.map(({ row, plan }) => <div key={row.id} className="flex flex-col gap-1"><span className="font-medium wrap-anywhere">{row.name}</span><ReviewBlockers blockers={plan.blockers} /></div>)}
      <RefusedRows rows={unread} />
    </div>
  </ConfirmDialog>;
}
