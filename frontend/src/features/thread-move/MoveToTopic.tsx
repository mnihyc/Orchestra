/** Moving the same conversation to a reviewed topic through the shared tree and current topic authority. */
import { useMutation, useQueries, useQuery } from '@tanstack/react-query';
import { useCallback, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { moveThread, previewThreadMove } from '@/api/thread-move';
import type { Thread, ThreadMovePlan, PreviewPerson } from '@/generated/api-types';
import { holds, permissionLabel } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { useOrgTree } from '@/realtime/stores/tree';
import { threadWriter } from '@/realtime/stores/thread';
import { SettingReadout } from '@/features/settings/SettingReadout';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { NodeIcon } from '@/ui/NodeIcon';
import { Tree, type TreeRow } from '@/ui/Tree';
import { topicTree, type MovePlace } from './tree';

function PersonChange({ entry, gained }: { entry: PreviewPerson; gained: boolean }) {
  const { t } = useTranslation();
  return <li>{t(gained ? 'threadMove.gains' : 'threadMove.loses', { name: entry.user.display_name, permissions: entry.permissions.map(permissionLabel).join(', '), count: entry.pages })}</li>;
}

export function MoveToTopic({ thread, close }: { thread: Thread; close: () => void }) {
  const { t } = useTranslation(); const tree = useOrgTree(thread.org_id);
  const [expanded, setExpanded] = useState(() => new Set(thread.path.map((place) => place.id)));
  const [target, setTarget] = useState<string | null>(null);
  const key = useRef(newActKey());
  const rows = useMemo(() => topicTree(tree.nodes, thread.org_id, expanded), [tree.nodes, thread.org_id, expanded]);
  // Only visible topic leaves are probed. Collapsed folders never create a request for every descendant.
  const topics = useMemo(() => rows.filter((row) => row.item.kind === 'topic' && row.id !== thread.topic_id), [rows, thread.topic_id]);
  const checks = useQueries({ queries: topics.map((row) => nodeQuery(row.id)) });
  const permissions = useMemo(() => new Map(topics.map((row, index) => [row.id, checks[index]])), [topics, checks]);
  const review = useQuery({ queryKey: ['thread-move', thread.id, target], queryFn: ({ signal }) => {
    if (target === null) throw new Error('Choose a topic before reviewing this move.');
    return previewThreadMove(thread.id, target, signal);
  }, enabled: target !== null, staleTime: 0, retry: false });
  const move = useMutation({ mutationFn: (plan: ThreadMovePlan) => moveThread(thread.id, plan, key.current), onSuccess: (moved) => {
    threadWriter.apply(thread.id, { thread: moved });
    close();
  } });
  const renderRow = useCallback((row: TreeRow<MovePlace>) => {
    const check = permissions.get(row.id);
    const topic = check?.data?.kind === 'topic' ? check.data : null;
    const mayChoose = row.item.kind === 'topic' && row.id !== thread.topic_id && topic !== null && holds(topic.access, 'topic.interact');
    const reason = row.id === thread.topic_id ? t('threadMove.here') : row.item.kind !== 'topic' ? null : check?.isPending === true ? t('threadMove.checking') : !mayChoose ? t('threadMove.needsInteract') : null;
    return <a href={`#move-topic-${row.id}`} aria-disabled={!mayChoose} className={`flex min-w-0 flex-1 items-center gap-2 py-1 ${mayChoose ? 'text-text' : 'text-text-muted'}`} onClick={(event) => { event.preventDefault(); if (mayChoose && !move.isPending) { key.current = newActKey(); move.reset(); setTarget(row.id); } }}>
      <NodeIcon kind={row.item.kind === 'project' && tree.nodes.get(row.item.parent_id ?? '')?.kind === 'project' ? 'folder' : row.item.kind} color={row.item.color} />
      <span className="truncate">{row.item.name}</span>{reason !== null && <span className="ml-auto truncate text-xs">{reason}</span>}
    </a>;
  }, [permissions, thread.topic_id, tree.nodes, move, t]);
  const plan = review.data;
  const footer = <><Button variant="ghost" onClick={close} disabled={move.isPending}>{t('common.cancel')}</Button>
    {target !== null && <Button variant="secondary" onClick={() => { key.current = newActKey(); move.reset(); void review.refetch(); }} disabled={move.isPending || review.isFetching}>{t('threadMove.reviewAgain')}</Button>}
    <Button pending={move.isPending} disabled={plan === undefined || review.isFetching || plan.blockers.length > 0} onClick={() => { if (plan !== undefined) move.mutate(plan); }}>{t('threadMove.confirm', { topic: plan?.to.name ?? '' })}</Button></>;
  return <Dialog open onOpenChange={(open) => { if (!open && !move.isPending) close(); }} title={t('threadMove.title')} size="lg" dismissible={!move.isPending} footer={footer}>
    <div className="flex flex-col gap-4">
      {tree.error !== null && <ErrorAlert error={tree.error} />}
      {!tree.loaded ? <p role="status">{t('threadMove.loading')}</p> : <Tree label={t('threadMove.choose')} rows={rows} currentId={target} nameOf={(place) => place.name} renderRow={renderRow} onExpandedChange={(id, open) => setExpanded((current) => { const next = new Set(current); if (open) next.add(id); else next.delete(id); return next; })} />}
      {review.isFetching && <p role="status" className="text-sm text-text-muted">{t('threadMove.reviewing')}</p>}
      {review.error !== null && <ErrorAlert error={review.error} />}{move.error !== null && <ErrorAlert error={move.error} />}
      {plan !== undefined && <section className="flex flex-col gap-2 text-sm" aria-label={t('threadMove.review')}>
        <p>{t('threadMove.unchanged', { owner: plan.unchanged.owner.display_name, environment: plan.unchanged.environment?.name ?? t('threadMove.noEnvironment'), directory: plan.unchanged.directory || '/' })}</p>
        {plan.blockers.map((blocker) => <p key={blocker.code} className="text-danger-text">{blocker.message}</p>)}
        <ul className="list-disc pl-6">{plan.access.gaining.map((entry) => <PersonChange key={`gain-${entry.user.id}`} entry={entry} gained />)}{plan.access.losing.map((entry) => <PersonChange key={`lose-${entry.user.id}`} entry={entry} gained={false} />)}{plan.access.more > 0 && <li>{t('threadMove.more', { count: plan.access.more })}</li>}</ul>
        {plan.settings.length > 0 && <><p className="text-text-muted">{t('threadMove.future')}</p><ul className="flex flex-col gap-3">{plan.settings.map((setting) => <li key={setting.key}><SettingReadout settingKey={setting.key} from={{ value: setting.from }} value={setting.to} /></li>)}</ul></>}
      </section>}
    </div>
  </Dialog>;
}
