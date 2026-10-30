/** Safe durable outcomes and retained removed routes, with only the repair the current route allows. */
import { ArrowDown, RefreshCw, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { changeRoute, removeRetainedRoute } from '@/api/away';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import type { AwayRoute } from './types';

export function RouteOutcome({ route }: { route: AwayRoute }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const name = route.channel?.name ?? t('away.outcome.removedName');
  const failed = route.health === 'failed' || route.health === 'partial';
  const removed = route.health === 'channel_removed';
  const rejected = route.health === 'target_gone';
  const retry = async () => {
    if (route.channel_id === null || busy) return;
    setBusy(true); setError(null);
    try { await changeRoute(route.channel_id, { retry_failed: true }); } catch (failure) { setError(failure); } finally { setBusy(false); }
  };
  return (
    <div className="flex flex-col gap-2" id={route.id === null ? undefined : `route-${route.id}`}>
      <p className="text-sm text-text-muted">{t('away.outcome.lastAccepted')} {route.last_ok_at === null ? <>{t('away.outcome.never')}{route.health === 'never' && <> · {t('away.notTested')}</>}</> : <RelativeTime at={route.last_ok_at} />}</p>
      {(failed || rejected || removed) && <InlineAlert tone="warning" title={removed ? t('away.outcome.removed', { name }) : rejected ? t('away.outcome.rejected', { reason: route.failure?.message ?? '' }) : t('away.outcome.failed', { way: name })}>
        {!rejected && route.failure !== null && <p className="wrap-anywhere">{route.failure.message}</p>}
        {route.health === 'partial' && (route.failure?.affected_devices.length ?? 0) > 0 && <p>{t('away.outcome.partial', { devices: route.failure?.affected_devices.join(t('startFlow.separator')) ?? '' })}</p>}
        <p>{t('away.outcome.waiting', { count: route.waiting_count })}</p>
        {failed && <div className="mt-2 flex flex-col items-start gap-1"><Button size="sm" icon={<RefreshCw aria-hidden />} pending={busy} disabled={!route.enabled || route.needs !== null} onClick={() => void retry()}>{t('away.outcome.retryWaiting')}</Button>{(!route.enabled || route.needs !== null) && <p className="text-xs">{t('away.outcome.retryRequires')}</p>}</div>}
      </InlineAlert>}
      {error !== null && <ErrorAlert error={error} />}
    </div>
  );
}

export function RetainedRoute({ route }: { route: AwayRoute }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const remove = async () => {
    if (route.id === null || busy) return;
    setBusy(true); setError(null);
    try { await removeRetainedRoute(route.id); } catch (failure) { setError(failure); } finally { setBusy(false); }
  };
  return <li><section aria-label={t('away.outcome.removed', { name: route.channel?.name ?? t('away.outcome.removedName') })} className="flex flex-col gap-3 rounded-lg border border-border bg-surface p-4" data-retained-route><RouteOutcome route={route} /><div className="flex flex-wrap gap-2"><Button size="sm" icon={<ArrowDown aria-hidden />} onClick={() => document.getElementById('outside')?.scrollIntoView({ behavior: 'smooth' })}>{t('away.outcome.choose')}</Button><Button size="sm" icon={<Trash2 aria-hidden />} pending={busy} onClick={() => void remove()}>{t('away.outcome.remove')}</Button></div>{error !== null && <ErrorAlert error={error} />}</section></li>;
}
