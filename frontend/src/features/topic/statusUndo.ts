/** A status change exposes one inverse using the returned revision and current topic rights. */
import { RotateCcw } from 'lucide-react';
import { createElement } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { usePendingPlaces } from '@/features/navigation/NodeMenu';
import { changeTopicStatus } from '@/api/day100';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

/** The inverse never guesses a new revision or calls a turn, queue or runtime operation. */
export function useStatusChange(): (before: Topic, status: TopicStatus) => Promise<Topic> {
  const { t } = useTranslation();
  const describe = useErrorText();
  return async (before, status) => {
    const pending = usePendingPlaces.getState().ids;
    if (pending.has(before.id)) throw new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Already changing', detail: 'Wait for the current change before choosing another status.', status: 409, request_id: '' }, null);
    usePendingPlaces.setState({ ids: new Set(pending).add(before.id) });
    let next: Topic;
    try { next = await changeTopicStatus(before.id, status, before.revision); }
    finally { usePendingPlaces.setState((state) => ({ ids: new Set([...state.ids].filter((id) => id !== before.id)) })); }
    let used = false;
    const dismiss = toast({ tone: 'success', title: t(status === 'done' ? 'places.toast.markedDone' : 'places.toast.reopened', { name: next.name }), ...(holds(next.access, 'topic.interact') ? { action: { label: t('day100.batch.undo'), icon: createElement(RotateCcw, { 'aria-hidden': true }), run: () => {
      if (used) return;
      used = true;
      dismiss();
      const pendingUndo = toast({ tone: 'info', persistent: true, title: t('day100.batch.undoing', { name: next.name }) });
      void changeTopicStatus(next.id, before.status, next.revision).then((restored) => { toast({ tone: 'success', title: t('day100.batch.statusRestored', { name: restored.name }) }); }).catch((error: unknown) => toast({ tone: 'danger', title: t('day100.batch.undoFailed', { name: next.name }), description: describe(error).message })).finally(pendingUndo);
    } } } : {}) });
    return next;
  };
}
