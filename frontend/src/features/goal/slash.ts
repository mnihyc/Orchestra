/** `/goal` in the slash menu: the words after it are the goal, none opens the editor.
 *
 * What is written is both the objective and "Done when", as plain words a person gives; the editor is the place to
 * say them apart or to add limits. Where a goal is already under way or stopped, the command opens that goal's
 * editor instead of trying to set another one. A reader who may not drive the thread is told that.
 */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { setGoal } from '@/api/goals';
import { openGoalEditor } from '@/features/goal/editor';
import { isLive } from '@/features/goal/model';
import { useGoalPlans } from '@/features/goal/plan';
import type { SlashCommand } from '@/features/composer/SlashMenu';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { toast } from '@/ui/Toast';

/** The `/goal` command of a thread's composer; nothing on a thread that cannot hold a goal. */
export function useGoalCommand(thread: Thread): SlashCommand[] {
  const { t } = useTranslation();
  const read = useThreadReadRef(thread.id);
  const plan = useGoalPlans(thread);
  const set = useMutation({
    mutationFn: (words: string) => setGoal(read, { objective: words, condition: words, notify_on_end: false, plan }),
    onSuccess: (goal) => toast({
      tone: 'success',
      title: t('goals.slash.set', { condition: goal.condition }),
      action: { label: t('goals.slash.edit'), run: () => openGoalEditor(thread.id) },
    }),
    onError: () => toast({ tone: 'danger', title: t('goals.act.failed') }),
  });
  if (thread.archived || thread.side_of !== null) return [];
  const goal = thread.goal ?? null;
  const existing = goal !== null && (isLive(goal) || goal.status === 'stopped');
  const off = !holds(thread.access, 'thread.interact') ? t('goals.act.driveOnly') : null;
  return [{
    name: 'goal',
    about: t('goals.slash.about'),
    off,
    run: (argument) => {
      if (existing || argument === '') {
        openGoalEditor(thread.id);
        return null;
      }
      set.mutate(argument);
      return null;
    },
  }];
}
