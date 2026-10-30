/** Unread summary and jump-to-latest behavior.
 *
 * Where what the reader has not seen begins: a "New" line in the conversation, and, when more than twenty
 * items came since they last read, a card that counts what happened: turns completed, milestones,
 * commands run, files changed, approvals waiting and errors. The newest end is reached by "Jump to latest".
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { catchupQuery } from '@/api/threads';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import type { Catchup } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton } from '@/ui/Skeleton';

/** More than this many unseen items and the card is shown: fewer are just read. */
export const CATCH_UP_ABOVE = 20;

export function NewLine({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const since = useThreadPart(threadId, (view) => view.openedReadSeq);
  const until = useThreadPart(threadId, (view) => view.openedSeq);
  const many = since !== null && until !== null && until - since > CATCH_UP_ABOVE;
  const options = catchupQuery(read, since ?? 0);
  const catchup = useQuery({ ...options, enabled: many && options.enabled });
  const data = catchup.data;
  return (
    <div className="flex flex-col gap-2 py-1" data-new-line>
      <div className="flex items-center gap-3 text-xs font-medium text-accent-text">
        <span className="h-px flex-1 bg-accent/40" />
        <span>{t('timeline.newLine')}</span>
        <span className="h-px flex-1 bg-accent/40" />
      </div>
      {data !== undefined && data.items > CATCH_UP_ABOVE && (
        <CatchupContents data={data} />
      )}
    </div>
  );
}


/** The same counted card, explicitly requested even when fewer than twenty items are new. */
export function RequestedCatchupCard({ threadId }: { threadId: string }) {
  const read = useThreadReadRef(threadId);
  const since = useThreadPart(threadId, (view) => view.openedReadSeq);
  const query = useQuery({ ...catchupQuery(read, since ?? 0), staleTime: 0 });
  if (query.data !== undefined) return <CatchupContents data={query.data} />;
  return query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : <Skeleton className="h-20 w-full" />;
}

function CatchupContents({ data }: { data: Catchup }) {
  const { t } = useTranslation();
  return (
        <section aria-label={t('timeline.catchup.title')} className="rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm text-text-muted">
          <h3 className="font-medium text-text">{t('timeline.catchup.title')}</h3>
          <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
            <li>{t('timeline.catchup.turns', { count: data.turns_completed })}</li>
            <li>{t('timeline.catchup.commands', { count: data.commands })}</li>
            <li>{t('timeline.catchup.files', { count: data.files_changed })}</li>
            {data.approvals_pending > 0 && <li>{t('timeline.catchup.approvals', { count: data.approvals_pending })}</li>}
            {data.turns_failed + data.errors > 0 && <li>{t('timeline.catchup.errors', { count: data.turns_failed + data.errors })}</li>}
            {data.milestones.map((milestone) => (
              <li key={milestone.item_id}>{t('timeline.catchup.milestone', { title: milestone.title })}</li>
            ))}
          </ul>
        </section>
  );
}
