/** The same held-input reason appears in the queue and beside a recorded background wake cause. */
import type { TFunction } from 'i18next';
import type { Thread, ThreadInput } from '@/generated/api-types';
import { modelDisplayName } from '@/features/models/names';

const COMMAND_HOLDS = {
  'native_action.stale': 'composer.queue.heldCommandStale',
  'native_action.permission': 'composer.queue.heldCommandPermission',
  'native_action.arguments': 'composer.queue.heldCommandArguments',
  'native_action.unavailable': 'composer.queue.heldCommandUnavailable',
} as const;

export function heldInputWords(input: ThreadInput, thread: Thread | null, t: TFunction): string | null {
  const sender = input.author?.display_name ?? t('composer.queue.theSender');
  const background = thread?.background;
  if (input.state_detail === 'background_stopped') return (background?.held_completions ?? 0) > 0 ? t('backgroundWake.held', { count: background?.held_completions }) : t('composer.queue.heldStop');
  if (input.state_detail === 'background_refresh') return t('backgroundWake.refreshHold');
  if (input.state_detail === 'background_wake_off') return t('backgroundWake.wakeOffHold');
  if (input.state_detail === 'author_lost_access') return t('backgroundWake.authorLostHold', { name: sender });
  if (input.queue_held) return t('composer.queue.heldStop');
  if (input.state_detail === 'cap_reached') return t('composer.queue.heldCap', { name: sender });
  if (input.state_detail === 'owner_lacks_access') return t('composer.queue.heldOwner', { owner: thread?.owner.display_name ?? '', provider: thread?.binding?.provider.name ?? '' });
  if (input.state_detail === 'second_step_required') return t('composer.queue.heldSecondStep', { name: sender });
  if (input.state_detail === 'model_use_required') {
    const selectedModel = thread?.model;
    const model = selectedModel === null || selectedModel === undefined ? '' : modelDisplayName(thread?.model_display);
    return thread?.environment !== null && thread?.environment !== undefined ? t('composer.queue.heldUse', { model, environment: thread.environment.name }) : t('composer.queue.heldModel', { model });
  }
  if (input.state_detail === 'files') return t('composer.queue.preparing');
  return input.state_detail !== null && input.state_detail in COMMAND_HOLDS ? t(COMMAND_HOLDS[input.state_detail as keyof typeof COMMAND_HOLDS], { name: sender }) : null;
}
