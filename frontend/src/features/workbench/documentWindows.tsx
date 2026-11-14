/** An open document retains its model independently of the effects of its visible editor. */
import { useEffect } from 'react';
import { retainDocument } from '@/realtime/live';
import { contentTarget } from './contentAddress';
import type { Tab } from './layout';

export function DocumentModelLease({ tab }: { tab: Tab }) {
  const target = tab.scope?.mode === 'fixed' ? tab.scope.target : contentTarget(tab);
  const id = target?.kind === 'document' ? target.id : null;
  useEffect(() => id === null ? undefined : retainDocument(id), [id]);
  return null;
}
