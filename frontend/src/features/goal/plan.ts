/** Whether the first turn of a goal plans: the composer's Plan switch, as it stands for the thread now. */
import { useQuery } from '@tanstack/react-query';

import { settingsQuery } from '@/api/settings';
import { useComposerStore } from '@/features/composer/Draft';
import type { Thread } from '@/generated/api-types';

/** The Plan switch decides, where the thread's program has plan mode; a place may start it on for a first message. */
export function useGoalPlans(thread: Thread): boolean {
  const offered = thread.binding?.plan_mode === true;
  const chosen = useComposerStore((held) => held.planning[thread.id]);
  const settings = useQuery({ ...settingsQuery(thread.id), enabled: offered });
  const planFirst = settings.data?.find((setting) => setting.key === 'thread.plan_first')?.value === true;
  return offered && (chosen ?? (planFirst && thread.usage === null && thread.current_turn_id === null));
}
