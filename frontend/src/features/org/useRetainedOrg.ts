/** Keep one revealed view's React state while its current org read is pending.
 * Callers hide the retained subtree; it never substitutes for current authority.
 */
import { useState } from 'react';
import type { Org } from '@/generated/api-types';

export function useRetainedOrg(scope: string, current: Org | undefined, pending: boolean): Org | undefined {
  const [held, setHeld] = useState({ scope, data: current });
  const data = current ?? (pending && held.scope === scope ? held.data : undefined);
  // A refusal, new user or new address retires the former target and its local intent.
  if (held.scope !== scope || held.data !== data) setHeld({ scope, data });
  return data;
}
