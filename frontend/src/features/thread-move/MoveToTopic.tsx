/** A thread moves to another topic of its tree at once: nothing is reviewed, a refusal is said, the notice offers Undo. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { moveThread, previewThreadMove } from '@/api/thread-move';
import type { Thread } from '@/generated/api-types';
import { canCreateIn } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { openChange } from '@/features/change/target';
import { CopyPlacePicker } from '@/features/duplicate/CopyPlacePicker';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import { threadWriter } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { toast } from '@/ui/Toast';

/** The server's move takes the plan it stored for this caller; it is read with the act, never shown for acceptance. */
async function moveTo(thread: Thread, topic: string, key: string): Promise<{ moved: Thread; lost: number; to: string }> {
  const plan = await previewThreadMove(thread.id, topic);
  const blocker = plan.blockers[0];
  if (blocker !== undefined) throw new Error(blocker.message);
  return { moved: await moveThread(thread.id, plan, key), lost: plan.access.losing.length, to: plan.to.name ?? '' };
}

export function MoveToTopic({ thread, close }: { thread: Thread; close: () => void }) {
  const { t } = useTranslation();
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const target = place?.id ?? null;
  const key = useRef(newActKey());
  const topic = useQuery({ ...nodeQuery(target ?? ''), enabled: target !== null });
  const mayChoose = topic.data?.kind === 'topic' && canCreateIn('topic', topic.data.access);
  const from = thread.topic_id;
  const move = useMutation({ mutationFn: (to: string) => moveTo(thread, to, key.current), onSuccess: ({ moved, lost, to }) => {
    threadWriter.apply(thread.id, { thread: moved });
    close();
    toast({ tone: 'success', title: t('threadMove.done', { topic: to }),
      ...(lost > 0 ? { description: t('places.move.lost', { count: lost }) } : {}),
      ...(from === null ? {} : { action: { label: t('places.toast.undo'), run: () => void moveTo(moved, from, newActKey()).then(
        (back) => { threadWriter.apply(thread.id, { thread: back.moved }); toast({ tone: 'success', title: t('threadMove.done', { topic: back.to }) }); },
        () => toast({ tone: 'danger', title: t('placeReview.undoChanged') })) } }) });
  } });
  const choose = (next: PickedPlace | null) => {
    if (move.isPending || next !== null && next.rootOrgId === undefined) return;
    if (next !== null && next.rootOrgId !== thread.org_id) {
      close();
      openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'move-out', destination: next });
      return;
    }
    key.current = newActKey();
    move.reset();
    setPlace(next);
  };
  const footer = <><Button variant="ghost" onClick={close} disabled={move.isPending}>{t('common.cancel')}</Button>
    <Button pending={move.isPending} disabled={target === null || !mayChoose || topic.isError} onClick={() => { if (target !== null) move.mutate(target); }}>{t('threadMove.confirm', { topic: topic.data?.name ?? place?.path ?? '' })}</Button></>;
  return <Dialog open onOpenChange={(open) => { if (!open && !move.isPending) close(); }} title={t('threadMove.title')} dismissible={!move.isPending} footer={footer}>
    <div className="flex flex-col gap-4">
      <CopyPlacePicker label={t('threadMove.choose')} only="topic" liveBranchesOnly subject={{ kind: 'thread', id: thread.id }} sourceName={thread.name}
        intent="move" homeMoveRoot={thread.org_id} homeMoveRefusal={(candidate) => candidate.id === thread.topic_id ? t('threadMove.here') : false}
        refusal={(candidate) => candidate.rootOrgId === undefined ? t('duplicate.homeUnknown') : false}
        onStanding={() => undefined} value={place} onChange={choose} disabled={move.isPending} />
      {target !== null && (topic.isError ? <ErrorAlert error={topic.error} /> : topic.isPending ? <p role="status">{t('threadMove.checking')}</p>
        : !mayChoose && <p className="text-sm text-text-muted">{t('threadMove.needsInteract')}</p>)}
      {move.error !== null && <ErrorAlert error={move.error} />}
    </div>
  </Dialog>;
}
