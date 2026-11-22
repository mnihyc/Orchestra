/** The selected topic determines which native move is reviewed. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { moveThread, previewThreadMove } from '@/api/thread-move';
import type { Thread, ThreadMovePlan, PreviewPerson } from '@/generated/api-types';
import { canCreateIn, permissionLabel } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { openChange } from '@/features/change/target';
import { CopyPlacePicker } from '@/features/duplicate/CopyPlacePicker';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import { threadWriter } from '@/realtime/stores/thread';
import { SettingReadout } from '@/features/settings/SettingReadout';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';

function PersonChange({ entry, gained }: { entry: PreviewPerson; gained: boolean }) {
  const { t } = useTranslation();
  return <li>{t(gained ? 'threadMove.gains' : 'threadMove.loses', { name: entry.user.display_name, permissions: entry.permissions.map(permissionLabel).join(', '), count: entry.pages })}</li>;
}

export function MoveToTopic({ thread, close }: { thread: Thread; close: () => void }) {
  const { t } = useTranslation();
  const [place, setPlace] = useState<PickedPlace | null>(null);
  const target = place?.id ?? null;
  const key = useRef(newActKey());
  const topic = useQuery({ ...nodeQuery(target ?? ''), enabled: target !== null });
  const mayChoose = topic.data?.kind === 'topic' && canCreateIn('topic', topic.data.access);
  const review = useQuery({ queryKey: ['thread-move', thread.id, target], queryFn: ({ signal }) => {
    if (target === null) throw new Error('Choose a topic before reviewing this move.');
    return previewThreadMove(thread.id, target, signal);
  }, enabled: target !== null && mayChoose, retry: false });
  const move = useMutation({ mutationFn: (plan: ThreadMovePlan) => moveThread(thread.id, plan, key.current), onSuccess: (moved) => {
    threadWriter.apply(thread.id, { thread: moved });
    close();
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
  const plan = review.data;
  const footer = <><Button variant="ghost" onClick={close} disabled={move.isPending}>{t('common.cancel')}</Button>
    {target !== null && mayChoose && <Button variant="secondary" onClick={() => { key.current = newActKey(); move.reset(); void review.refetch(); }} disabled={move.isPending || review.isFetching}>{t('threadMove.reviewAgain')}</Button>}
    <Button pending={move.isPending} disabled={!mayChoose || topic.isError || review.isError || plan === undefined || review.isFetching || plan.blockers.length > 0} onClick={() => { if (plan !== undefined) move.mutate(plan); }}>{t('threadMove.confirm', { topic: plan?.to.name ?? '' })}</Button></>;
  return <Dialog open onOpenChange={(open) => { if (!open && !move.isPending) close(); }} title={t('threadMove.title')} size="lg" dismissible={!move.isPending} footer={footer}>
    <div className="flex flex-col gap-4">
      <CopyPlacePicker label={t('threadMove.choose')} only="topic" liveBranchesOnly subject={{ kind: 'thread', id: thread.id }} sourceName={thread.name}
        intent="move" homeMoveRoot={thread.org_id} homeMoveRefusal={(candidate) => candidate.id === thread.topic_id ? t('threadMove.here') : false}
        refusal={(candidate) => candidate.rootOrgId === undefined ? t('duplicate.homeUnknown') : false}
        onStanding={() => undefined} value={place} onChange={choose} disabled={move.isPending} />
      {target !== null && (topic.isError ? <ErrorAlert error={topic.error} /> : topic.isPending ? <p role="status">{t('threadMove.checking')}</p>
        : !mayChoose && <p className="text-sm text-text-muted">{t('threadMove.needsInteract')}</p>)}
      {review.isFetching && <p role="status" className="text-sm text-text-muted">{t('threadMove.reviewing')}</p>}
      {review.error !== null && <ErrorAlert error={review.error} />}{move.error !== null && <ErrorAlert error={move.error} />}
      {mayChoose && plan !== undefined && <section className="flex flex-col gap-2 text-sm" aria-label={t('threadMove.review')}>
        <p>{t('threadMove.unchanged', { owner: plan.unchanged.owner.display_name, environment: plan.unchanged.environment?.name ?? t('threadMove.noEnvironment'), directory: plan.unchanged.directory || '/' })}</p>
        {plan.blockers.map((blocker) => <p key={blocker.code} className="text-danger-text">{blocker.message}</p>)}
        <ul className="list-disc pl-6">{plan.access.gaining.map((entry) => <PersonChange key={`gain-${entry.user.id}`} entry={entry} gained />)}{plan.access.losing.map((entry) => <PersonChange key={`lose-${entry.user.id}`} entry={entry} gained={false} />)}{plan.access.more > 0 && <li>{t('threadMove.more', { count: plan.access.more })}</li>}</ul>
        {plan.settings.length > 0 && <><p className="text-text-muted">{t('threadMove.future')}</p><ul className="flex flex-col gap-3">{plan.settings.map((setting) => <li key={setting.key}><SettingReadout settingKey={setting.key} from={{ value: setting.from }} value={setting.to} /></li>)}</ul></>}
      </section>}
    </div>
  </Dialog>;
}
