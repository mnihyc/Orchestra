/** The one way to open where a thread works: its environment and its directory.
 *
 * The chip under the message, the `/directory` command and the act that a reason about the environment offers all call
 * this, so the control that answers it is one place to change.
 */
import { openChange } from '@/features/change/target';
import type { Thread } from '@/generated/api-types';

export function openWhere(thread: Pick<Thread, 'id' | 'name'>): void {
  openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'environment' });
}
