/** The heading of what an adopted session held before Orch took it in: the earlier turns as its harness recorded them, read-only, and never sent to the agent again. */
import { History } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { adoptedHistoryQuery } from '@/api/adoption';
import { RetryAdoptedHistory } from '@/features/resources/adopt/RetryHistory';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { useTimelineContext } from '../context';

export function BeforeOrchHeading() {
  const { t } = useTranslation();
  const timeline = useTimelineContext();
  const query = useQuery(adoptedHistoryQuery(timeline.threadId));
  const status = query.data;
  return (
    <div role="separator" aria-label={t('adopt.band')} className="flex flex-col items-center gap-1 border-b border-border py-2 text-center">
      <span className="flex items-center gap-2 text-sm font-medium text-text-muted">
        <History aria-hidden /> {t('adopt.band')}
      </span>
      <span className="text-xs text-text-subtle">{t('adopt.bandLead')}</span>
      {status === undefined && query.error === null && <SkeletonRows rows={1} />}
      {query.error !== null && <ErrorAlert error={query.error} />}
      {status?.state === 'unreadable' && <p className="text-sm text-text-muted">{t('adopt.history.unreadable')}</p>}
      {status?.state === 'unsupported' && <p className="text-sm text-text-muted">{t('adopt.history.unsupported')}</p>}
      {status?.state === 'offline' && <p className="text-sm text-text-muted">{t('adopt.history.offline')}</p>}
      {status?.state === 'unavailable' && <p className="text-sm text-text-muted">{t('adopt.history.unavailable')}</p>}
      {status !== undefined && status !== null && status.state !== 'read' && typeof status.reason === 'string' && <p className="whitespace-pre-wrap break-words text-sm">{status.reason}</p>}
      {status?.can_read_again && timeline.inspection === undefined && <RetryAdoptedHistory threadId={timeline.threadId} />}
    </div>
  );
}
