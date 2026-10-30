/** Changed since you were here: who changed which blocks since the reader last had the document open, marked in the page.
 *
 * Opening a document that changed since the reader last had it open shows one line under its title,
 * with who changed how many blocks. Review marks those blocks in the page itself, each with who
 * changed it, until the reader dismisses it. An agent's blocks are marked no differently from a person's.
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, Eye } from 'lucide-react';

import { client } from '@/api/client';
import { changesQuery, markSeen, type Document } from '@/api/documents';
import { ChangesView } from '@/features/documents/History';
import { ErrorAlert } from '@/ui/ErrorState';
import { useDocumentMarksStore } from '@/features/documents/marks';
import { authorName } from '@/features/documents/Together';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';

export function SinceYouLooked({ doc }: { doc: Document }) {
  const { t } = useTranslation();
  const marks = useDocumentMarksStore();
  // The sequence the reader last had on screen, as it was when the page opened: this visit's own looking does not move it.
  const [lookedAt] = useState(doc.seen_seq);
  const [dismissed, setDismissed] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  // The line is about what happened before this visit: it asks once, at the sequence the reader last had.
  const ask = lookedAt !== null && doc.seq > lookedAt;
  const changes = useQuery({ ...changesQuery(doc.id, 'seen', null), enabled: ask, staleTime: Number.POSITIVE_INFINITY });
  const me = client.session.getState().userId;
  // What the reader changed themselves is not news to them.
  const blocks = changes.data?.blocks.filter((one) => !(one.by !== null && 'id' in one.by && one.by.id === me));
  const who = useMemo(() => {
    const counts = new Map<string, number>();
    for (const one of blocks ?? []) counts.set(authorName(one.by), (counts.get(authorName(one.by)) ?? 0) + 1);
    return [...counts].map(([name, count]) => t('people.writerBlocks', { name, count }));
  }, [blocks, t]);

  useEffect(() => {
    if (!reviewing || dismissed) {
      marks.setState({ changed: new Map() });
      return;
    }
    marks.setState({ changed: new Map((blocks ?? []).filter((one) => one.after !== null).map((one) => [one.id, authorName(one.by)])) });
    return () => marks.setState({ changed: new Map() });
  }, [reviewing, dismissed, blocks, marks]);

  if (!ask || dismissed || blocks === undefined || blocks.length === 0) return null;
  return (
    <div className="mb-4 flex flex-col gap-2">
    <Banner
      tone="info"
      className="mb-4"
      onDismiss={() => setDismissed(true)}
      action={
        <Button size="sm" icon={<Eye aria-hidden />} aria-pressed={reviewing} onClick={() => setReviewing((on) => !on)}>
          {reviewing ? t('documents.since.hide') : t('documents.since.review')}
        </Button>
      }
    >
      {t('documents.since.line', { who: who.join(', ') })}
    </Banner>
    {reviewing && changes.data !== undefined && <>
      <ChangesView doc={doc} changes={changes.data} columns={1} onStale={() => void changes.refetch()} />
      <Button size="sm" icon={<Check aria-hidden />} pending={pending} onClick={() => { setPending(true); setError(null); void markSeen(doc.id, doc.seq).then(() => setDismissed(true)).catch(setError).finally(() => setPending(false)); }}>{t('people.seen')}</Button>
      {error !== null && <ErrorAlert error={error} />}
    </>}
    </div>
  );
}
