/** Put one block back using the shown after-state and the current snapshot, keeping a lost reply repeatable. */
import { Check, Undo2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';

import { ApiError } from '@/api/client';
import { putBackBlocks } from '@/api/people';
import { inverseOf } from '@/features/people/inverse';
import type { GuardedOpsBody } from '@/features/people/pending';
import type { Document, DocumentChanges } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { documentWriter, useDocumentStore } from '@/realtime/stores/document';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export function PutBack({ doc, change, changes, onStale }: { doc: Document; change: DocumentChanges['blocks'][number]; changes: DocumentChanges; onStale: () => void }) {
  const { t } = useTranslation();
  const [pending, setPending] = useState(false);
  const [done, setDone] = useState(false);
  const [stale, setStale] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const batch = useRef<GuardedOpsBody | null>(null);
  if (!holds(doc.access, 'doc.edit') || doc.archived || doc.deleted) return null;
  const apply = async () => {
    if (pending) return;
    const current = useDocumentStore.getState().byId.get(doc.id);
    if (!current?.document || !holds(current.document.access, 'doc.edit')) { setStale(true); onStale(); return; }
    if (batch.current === null) {
      const inverse = inverseOf(change, changes, current.blocks.values(), newActKey);
      if (inverse.kind === 'stale') { setStale(true); onStale(); return; }
      batch.current = { client_id: newActKey(), expected_seq: current.document.seq, ops: inverse.ops };
    }
    setPending(true); setError(null);
    try {
      const answer = await putBackBlocks(doc.id, batch.current);
      documentWriter.apply(doc.id, { document: answer.document, blocks: answer.blocks }, answer.seq);
      setDone(true);
    } catch (failure) {
      if (failure instanceof ApiError && failure.status === 409) { batch.current = null; setStale(true); onStale(); }
      else setError(failure);
    } finally { setPending(false); }
  };
  const explanation = change.before === null ? 'people.putBackAdded' : change.after === null ? 'people.putBackRemoved' : 'people.putBackUpdate';
  return <div className="mt-2 flex flex-col items-start gap-1">
    {done ? <span role="status" className="flex items-center gap-1 text-xs text-success"><Check aria-hidden className="size-4" />{t('people.putBackDone')}</span> : <Button size="sm" variant="ghost" icon={<Undo2 aria-hidden />} pending={pending} disabled={stale} title={t(explanation)} onClick={() => void apply()}>{t('people.putBack')}</Button>}
    {stale && <StateLine role="alert" tone="warning" compact message={t('people.laterChange')} />}
    {error !== null && <ErrorAlert error={error} />}
  </div>;
}
