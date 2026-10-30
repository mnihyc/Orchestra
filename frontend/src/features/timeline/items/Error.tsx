/** Actionable failure and retry.
 *
 * What went wrong in the thread, in a row of its own: the message. The way to try again is the one of
 * the turn that failed, in its footer under this row, and there once: a second button here would do the
 * same thing.
 */
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { FailureAlert } from '../FailureAlert';
import { failureKey } from '../failures';
import { useTimelineContext } from '../context';
import { useThreadPart } from '@/realtime/stores/thread';

type ErrorItem = Extract<Item, { kind: 'error' }>;

export function ErrorRow({ item }: { item: ErrorItem }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const attempts = useThreadPart(threadId, (view) => item.turn_id === null ? 1 : [...view.items.values()].filter((other) => other.kind === 'error' && other.turn_id === item.turn_id && failureKey(other.payload) === failureKey(item.payload)).length);
  return (
    <ItemFrame item={item} label={t('timeline.error.menu')}>
      <FailureAlert error={item.payload} attempts={attempts} />
    </ItemFrame>
  );
}
