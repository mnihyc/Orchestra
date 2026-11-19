/** Unread summary and jump-to-latest behavior.
 *
 * Where what the reader has not seen begins: a "New" line in the conversation, and, above the reader's threshold,
 * items came since they last read, a card that counts what happened: turns completed, milestones,
 * commands run, files changed, approvals waiting and errors. The newest end is reached by "Jump to latest".
 */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import {Link} from '@tanstack/react-router';

import { catchupQuery } from '@/api/threads';
import { useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import type { Catchup } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { Skeleton } from '@/ui/Skeleton';
import {FactDisclosure} from '@/ui/FactDisclosure';
import {useInterfaceReading} from '@/lib/interfaceReading';
import {useFormatContext} from '@/lib/appearance';
import {formatNumber} from '@/lib/format';

export function NewLine({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const reading=useInterfaceReading();
  const {locale}=useFormatContext();
  const threshold=reading.catch_up_above_items;
  const read = useThreadReadRef(threadId);
  const since = useThreadPart(threadId, (view) => view.openedReadSeq);
  const until = useThreadPart(threadId, (view) => view.openedSeq);
  const many = threshold!==undefined&&since !== null && until !== null && until - since > threshold;
  const options = catchupQuery(read, since ?? 0);
  const catchup = useQuery({ ...options, enabled: many && options.enabled });
  const data = catchup.data;
  return (
    <div className="flex flex-col gap-2 py-1" data-new-line>
      <div className="flex items-center gap-3 text-xs font-medium text-accent-text">
        <span className="h-px flex-1 bg-accent/40" />
        <span>{t('timeline.newLine')}</span>
        <FactDisclosure groups={[{key:'catchup',kind:'current_setting',facts:[{key:'threshold',label:t('timeline.catchup.automatic'),
          value:threshold===undefined?undefined:t('timeline.catchup.automaticAfter',{count:threshold,number:formatNumber(threshold,locale)}),
          source:reading.catch_up_source===null?undefined:<Link to="/settings/interface" hash="interface-reading" className="text-accent-text underline underline-offset-2">{t(reading.catch_up_source==='personal'?'timeline.catchup.personalSource':'timeline.catchup.installationSource')}</Link>}]}]}/>
        <span className="h-px flex-1 bg-accent/40" />
      </div>
      {threshold!==undefined&&many&&data !== undefined && data.items > threshold && (
        <CatchupContents data={data} />
      )}
    </div>
  );
}


/** The same counted card, explicitly requested independently of the automatic threshold. */
export function RequestedCatchupCard({ threadId }: { threadId: string }) {
  const read = useThreadReadRef(threadId);
  const since = useThreadPart(threadId, (view) => view.openedReadSeq);
  const query = useQuery({ ...catchupQuery(read, since ?? 0) });
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
