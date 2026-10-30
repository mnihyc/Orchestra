/** Side-thread availability comes from the thread's verified reading mode and current authority. */
import type { TFunction } from 'i18next';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';

/** Only programs with a verified reading mode offer a new read-only question. */
export function sideReadingAvailable(thread: Pick<Thread, 'sides'> | null | undefined): boolean {
  const mode = thread?.sides?.mode;
  return mode !== null && mode !== undefined;
}

/** A refusal for an existing draft or an offered action, or false when this reader may ask here. */
export function sideRefusal(thread: Thread | null | undefined, t: TFunction): string | false {
  if (thread === null || thread === undefined) return t('side.unavailable.loading');
  if (!holds(thread.access, 'thread.interact')) return t('side.unavailable.permission');
  if (thread.archived) return t('side.unavailable.archived');
  if (thread.side_of !== null) return t('side.unavailable.side');
  if (thread.lock !== null) return t('side.unavailable.busy');
  if (!thread.sides) return t('side.unavailable.notReported');
  if (!thread.sides.allowed) return t('side.unavailable.policy');
  if (!thread.sides.mode) {
    const reason = thread.sides.reason?.trim() ?? '';
    if (reason.includes('no verified way to read without acting') || reason === 'This agent cannot answer without being able to change things.') return t('side.unavailable.readOnly');
    if (reason.includes('has not said yet what it can do') || reason === 'This agent has not reported whether it can answer without changing things yet.') return t('side.unavailable.notReported');
    if (reason === "the thread's agent profile is gone" || reason === 'This thread’s agent profile is no longer available.') return t('side.unavailable.agentGone');
    return t('side.unavailable.readOnly');
  }
  return false;
}
