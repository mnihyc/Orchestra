/** The topic's state, changed from its chip by whoever may take part, with the ordinary guarded Undo. */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { useIsPending } from '@/features/navigation/NodeMenu';
import { useStatusChange } from '@/features/topic/statusUndo';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { Menu } from '@/ui/Menu';
import { StateChip, type WorkState } from '@/ui/StateChip';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

export function TopicStatusChip({ topic, allowed }: { topic: Topic; allowed: boolean }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const pending = useIsPending(topic.id);
  const changeStatus = useStatusChange();
  const change = useMutation({
    mutationFn: (status: TopicStatus) => changeStatus(topic, status),
    onError: (error) => toast({ tone: 'danger', title: t('places.toast.notChanged', { name: topic.name }), description: describe(error).message }),
  });
  const readonlyState = topic.status === 'done' ? 'done' : 'idle';
  if (!allowed || topic.archived) return <StateChip state={readonlyState} label={t(`places.topic.statuses.${topic.status}`)} />;
  const working = change.isPending || pending;
  const current = change.isPending ? change.variables : topic.status;
  const label = t(`places.topic.statuses.${current}`);
  const state: WorkState = working ? 'starting' : current === 'done' ? 'done' : 'idle';
  const choose = (next: string) => {
    if (!working && next !== current && (next === 'open' || next === 'done')) change.mutate(next);
  };
  return <Menu label={t('places.topic.statusLabel')} entries={[{
    type: 'radio', id: 'status', label: t('places.topic.statusLabel'), value: current, onValueChange: choose,
    options: [
      { value: 'open', name: t('places.topic.statuses.open') },
      { value: 'done', name: t('places.topic.statuses.done') },
    ],
  }]} trigger={
    <button type="button" className="inline-flex h-(--control-sm) max-w-full items-center rounded-full text-left disabled:cursor-wait"
      aria-label={t('places.topic.changeStatus', { status: label })} aria-busy={working || undefined} disabled={working}>
      <StateChip state={state} label={label} menu className="transition-colors hover:border-border-strong hover:bg-bg-muted" />
    </button>
  } />;
}
