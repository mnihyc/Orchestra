/** Connection freshness, a slow link said as such, and new-build notice.
 *
 * Whether what the thread shows can be trusted to be current, as far as this thread goes: a thread that
 * could not be brought up to date says so, with the way to try again. The connection itself (gone for
 * more than a moment, a new build) is said once for the whole application above the work; a slow line is
 * said by the status bar and by no banner, because slowness is no event.
 */
import { useTranslation } from 'react-i18next';

import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadPart } from '@/realtime/stores/thread';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { useErrorText } from '@/ui/errorText';
import { isReadCancelled } from '@/api/reads';

export function ConnectionBanner({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const error = useThreadPart(read, (view) => view.closed === null && view.loaded !== 'none' ? view.error : null);
  const explain = useErrorText();
  if (error === null || isReadCancelled(error)) return null;
  const reason = explain(error).message;
  const capacity = typeof error === 'object' && 'code' in error && error.code === 'too_many_subscriptions';
  return (
    <Banner
      tone="warning"
      className="rounded-md border"
      action={
        <Button size="sm" onClick={() => reloadThread(read, true)}>
          {t('thread.connection.retry')}
        </Button>
      }
    >
      <FactDisclosure fragment={capacity ? reason : t('thread.connection.stale')}
        groups={[{ key: 'failure', facts: [{ key: 'reason', label: t('common.reason'), value: reason }] }]} />
    </Banner>
  );
}
