/** Connection freshness, a slow link said as such, and new-build notice.
 *
 * Whether what the thread shows can be trusted to be current, as far as this thread goes: a thread that
 * could not be brought up to date says so, with the way to try again. The connection itself (gone for
 * more than a moment, a new build) is said once for the whole application above the work; a slow line is
 * said by the status bar and by no banner, because slowness is no event.
 */
import { useTranslation } from 'react-i18next';

import { reloadThread } from '@/realtime/live';
import { useThreadPart } from '@/realtime/stores/thread';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';

export function ConnectionBanner({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const failed = useThreadPart(threadId, (view) => view.loaded !== 'none' && view.error !== null);
  if (!failed) return null;
  return (
    <Banner
      tone="warning"
      className="rounded-md border"
      action={
        <Button size="sm" onClick={() => reloadThread(threadId)}>
          {t('thread.connection.retry')}
        </Button>
      }
    >
      {t('thread.connection.stale')}
    </Banner>
  );
}
