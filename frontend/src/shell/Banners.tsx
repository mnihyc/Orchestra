/** System and connection notices.
 *
 * What stands above the work and concerns all of it: the notice the system's administrators put up,
 * an org of the person's that waits for a second step they have not set up (with the way to Security),
 * a live connection that has been gone for more than a moment, a newer build that was deployed
 * while the page was open, and the one line that says an arrangement could not be read and was
 * reset. None of them moves, and a connection that comes and goes within two seconds says nothing.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { meQuery, metaQuery } from '@/api/auth';
import { LOST_SAID_AFTER_MS } from '@/features/workbench/StatusParts';
import { InstallationProgress } from '@/features/admin/InstallationProgress';
import { useWorkbench } from '@/features/workbench/store';
import { useSocketStatus } from '@/realtime/live';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { useFrameReady } from '@/shell/FrameReadiness';

/** Whether the connection has been gone for longer than a moment. */
function useLongLost(lostAt: number | null): boolean {
  /** The loss for which the moment has passed. */
  const [saidFor, setSaidFor] = useState<number | null>(null);
  useEffect(() => {
    if (lostAt === null) return;
    const timer = setTimeout(() => setSaidFor(lostAt), Math.max(0, lostAt + LOST_SAID_AFTER_MS - Date.now()));
    return () => clearTimeout(timer);
  }, [lostAt]);
  return lostAt !== null && saidFor === lostAt;
}

export function Banners() {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const meta = useQuery({ ...metaQuery, enabled: ready }).data;
  const status = useSocketStatus();
  const dropped = useWorkbench((state) => state.dropped);
  const acknowledge = useWorkbench((state) => state.acknowledgeDropped);
  const lost = useLongLost(status.lostAt);
  const notice = meta?.banner ?? null;
  // An org of the person's that requires a second step they have not set up: its work waits, and the way to it is said here.
  const me = useQuery({ ...meQuery, enabled: ready }).data;
  const setupNeeded = me !== undefined && me.second_step_required && !me.totp_enabled && !me.email_code_second_step;
  const requiring = (me?.second_step_sources ?? []).map((source) => (typeof source === 'string' ? t('secondStep.system') : (source.name ?? t('secondStep.system')))).join(', ');
  const [dismissed, setDismissed] = useState<string | null>(null);
  const noticeKey = notice === null ? null : `${notice.level}:${notice.text}`;

  return (
    <>
      {notice !== null && noticeKey !== dismissed && (
        <Banner tone={notice.level} onDismiss={() => setDismissed(noticeKey)}>
          {notice.text}
        </Banner>
      )}
      {status.outdated ? (
        <Banner
          tone="info"
          action={
            <Button size="sm" onClick={() => location.reload()}>
              {t('shell.banner.reload')}
            </Button>
          }
        >
          {t('shell.banner.newVersion')}
        </Banner>
      ) : (
        lost && <Banner tone="warning"><InstallationProgress fallback={t('shell.banner.reconnecting')} /></Banner>
      )}
      {setupNeeded && (
        <Banner
          tone="warning"
          action={
            <Button asChild size="sm">
              <Link to="/settings/security">{t('secondStep.setupAction')}</Link>
            </Button>
          }
        >
          {t('secondStep.setupLead', { places: requiring })}
        </Banner>
      )}
      {dropped && (
        <Banner tone="info" onDismiss={acknowledge}>
          {t('shell.banner.arrangementReset')}
        </Banner>
      )}
    </>
  );
}
