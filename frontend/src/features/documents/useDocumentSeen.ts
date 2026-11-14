/** A document counts as read only after one uninterrupted interval in a visible view and browser tab. */
import { useEffect } from 'react';
import { markSeen } from '@/api/documents';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { Document } from '@/generated/api-types';

/** The existing reading interval, restarted whenever the reader leaves the document. */
const SEEN_AFTER_MS = 3_000;

export function useDocumentSeen(doc: Pick<Document, 'id' | 'seq' | 'deleted'> | null, version: number | undefined): void {
  const shown = useViewShown();
  const id = doc !== null && !doc.deleted && version === undefined ? doc.id : null;
  const seq = doc?.seq ?? 0;
  useEffect(() => {
    if (id === null || !shown) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let acknowledged = false;
    let stopped = false;
    const reading = () => {
      clearTimeout(timer);
      if (acknowledged || document.visibilityState !== 'visible') return;
      timer = setTimeout(() => {
        if (stopped || document.visibilityState !== 'visible') return;
        void markSeen(id, seq).then(() => { acknowledged = true; }).catch(() => undefined);
      }, SEEN_AFTER_MS);
    };
    document.addEventListener('visibilitychange', reading);
    reading();
    return () => {
      stopped = true;
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', reading);
    };
    // Incoming edits do not restart a reader's wait or mark unseen newer text as read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, shown]);
}
