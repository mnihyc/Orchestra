/** Undo a complete recorded text edit through the ordinary observed-hash file writer. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { readFileBytes, statFile, writeFile } from '@/api/workspace';
import { bringFile, invalidateHead } from '@/features/files/content';
import { PREVIEW_BYTES } from '@/features/files/viewers';
import type { WorkspaceAccess } from '@/features/files/door';
import type { EnvironmentChange } from '@/generated/api-types';
import { reverseDiff } from '@/lib/reverseDiff';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { DiffView } from '@/ui/DiffView';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export function undoUnavailable(change: EnvironmentChange): 'structural' | 'incomplete' | null {
  if (change.binary || change.truncated || change.diff === null) return 'incomplete';
  return change.change === 'modify' ? null : 'structural';
}

type UndoReason = 'incomplete' | 'textOnly' | 'unavailable' | 'changed';
interface ReadyUndo { state: 'ready'; text: string; diff: string; hash: string }
type UndoPlan = ReadyUndo | { state: 'unavailable'; reason: UndoReason };
const unavailable = (reason: UndoReason): UndoPlan => ({ state: 'unavailable', reason });

export function UndoChange({ change, access, onClose }: { change: EnvironmentChange; access: WorkspaceAccess; onClose: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const plan = useQuery({
    queryKey: ['undo-file-change', access.door.environmentId, access.door.threadId, change.item_id, change.path, change.diff],
    queryFn: async ({ signal }): Promise<UndoPlan> => {
      if (undoUnavailable(change) !== null || change.diff === null) return unavailable('incomplete');
      const stat = await statFile(access.door, change.path, signal);
      if (!stat.is_text || stat.size > PREVIEW_BYTES) return unavailable('textOnly');
      const brought = await bringFile(access.door, change.path, signal);
      if (brought.state !== 'ready') return unavailable('unavailable');
      const bytes = await readFileBytes(access.door, change.path, brought.hash, PREVIEW_BYTES, signal);
      if (bytes.total !== bytes.bytes.length || bytes.total > PREVIEW_BYTES) return unavailable('textOnly');
      let current: string;
      try { current = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes.bytes); }
      catch { return unavailable('textOnly'); }
      const inverse = reverseDiff(current, change.diff);
      if (inverse === null) return unavailable('changed');
      return { state: 'ready', ...inverse, hash: brought.hash };
    },
    enabled: access.mayView && access.mayManage,
    staleTime: 0,
    retry: false,
  });
  const undo = useMutation({
    mutationFn: (ready: ReadyUndo) => writeFile(access.door, change.path, ready.text, ready.hash),
    onSuccess: () => { void invalidateHead(cache, access.door, change.path); toast({ tone: 'success', title: t('thread.undo.done') }); onClose(); },
  });
  return <Dialog open title={t('thread.undo.title', { path: change.path })} onOpenChange={(open) => { if (!open) onClose(); }} footer={<>
    <Button onClick={onClose}>{t('common.cancel')}</Button>
    {undo.error !== null && <Button onClick={() => { undo.reset(); void plan.refetch(); }}>{t('source.restore.freshPlan')}</Button>}
    <Button variant="primary" pending={undo.isPending} disabled={!access.mayManage || plan.data?.state !== 'ready' || plan.isFetching || plan.isError || undo.error !== null}
      onClick={() => { if (access.mayManage && plan.data?.state === 'ready') undo.mutate(plan.data); }}>{t('thread.undo.confirm')}</Button>
  </>}>
    <p className="text-sm text-text-muted">{t('thread.undo.review')}</p>
    {!access.mayView || !access.mayManage ? <InlineAlert tone="warning">{t('files.permission.manage')}</InlineAlert>
      : plan.isPending ? <SkeletonRows rows={3} /> : plan.isError ? <ErrorState compact error={plan.error} onRetry={() => void plan.refetch()} />
      : plan.data.state === 'unavailable' ? <InlineAlert tone="warning">{t(`thread.undo.${plan.data.reason}`)}</InlineAlert>
      : <DiffView diff={plan.data.diff} label={t('thread.undo.title', { path: change.path })} />}
    {undo.error !== null && <ErrorAlert error={undo.error} />}
  </Dialog>;
}
