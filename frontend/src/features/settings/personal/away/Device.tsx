/** Current-device readiness, explicit registration/test/removal, and separately named other devices. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink, FlaskConical, Plus, RefreshCw, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery } from '@/api/auth';
import { devicesKey, devicesQuery, outsideKey, removeDevice, routesKey, testDevice } from '@/api/away';
import { notificationChannelsKey } from '@/api/notifications';
import { currentSubscription, enablePush } from '@/features/settings/personal/push';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { deviceState, inspectDevice } from './device';

const hereKey = ['away', 'this-device'] as const;
/** Browser guides explain the actual web features without guessing the person's platform or installed-app requirements. */
const GUIDES = {
  insecure: 'https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Secure_Contexts',
  unavailable: 'https://developer.mozilla.org/en-US/docs/Web/API/Push_API',
  blocked: 'https://developer.mozilla.org/en-US/docs/Web/API/Notifications_API/Using_the_Notifications_API#requesting_permission',
};

export function DeviceReadiness({ channelId }: { channelId: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const devices = useQuery(devicesQuery);
  const here = useQuery({ queryKey: hereKey, queryFn: inspectDevice, retry: false, staleTime: 0 });
  const key = useQuery(metaQuery).data?.push_public_key;
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [errorAct, setErrorAct] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; error: string | null } | null>(null);
  const facts = here.data?.facts;
  const hash = here.data?.hash ?? null;
  const own = devices.data?.find((device) => device.endpoint_hash === hash);
  const state = here.isError || devices.isError || errorAct === 'enable' ? 'failed' : facts === undefined || devices.data === undefined ? 'checking' : deviceState(facts, hash, devices.data);
  const refresh = async () => {
    for (const queryKey of [hereKey, devicesKey, routesKey, notificationChannelsKey, outsideKey]) await client.invalidateQueries({ queryKey });
  };
  const run = async (id: string, act: () => Promise<unknown>) => {
    if (busy !== null) return;
    setBusy(id); setError(null); setErrorAct(null);
    try {
      await act();
      // Removing this device unsubscribes the browser after the server act;
      // its local subscription is read again, apart from the server views.
      if (id === 'remove') await client.invalidateQueries({ queryKey: hereKey });
    } catch (failure) { setError(failure); setErrorAct(id); } finally { setBusy(null); }
  };
  const guide = state === 'insecure' || state === 'unavailable' || state === 'blocked' ? GUIDES[state] : null;
  return (
    <section aria-label={t('away.device.heading')} className="flex flex-col gap-2 rounded-lg border border-border bg-bg-subtle p-3" data-device-state={state}>
      <h3 className="text-sm font-medium text-text">{t('away.device.heading')}</h3>
      <p role="status" className="text-sm text-text">{t(`away.device.${state}`)}</p>
      <div className="flex flex-wrap gap-2">
        {guide !== null && <Button asChild size="sm"><a href={guide} target="_blank" rel="noopener noreferrer"><ExternalLink aria-hidden />{t(state === 'blocked' ? 'away.device.settings' : 'away.device.guide')}</a></Button>}
        {(state === 'default' || state === 'missing') && (key === undefined || key === null ? <p className="text-sm text-text-muted">{t('away.device.noKey')}</p> : <Button size="sm" icon={<Plus aria-hidden />} pending={busy === 'enable'} onClick={() => void run('enable', () => enablePush(key))}>{t('away.device.enable')}</Button>)}
        {state === 'registered' && own !== undefined && <>
          <Button size="sm" icon={<FlaskConical aria-hidden />} pending={busy === 'test'} onClick={() => { setResult(null); void run('test', async () => setResult(await testDevice(channelId, own.id))); }}>{t('away.device.test')}</Button>
          <Button size="sm" icon={<Trash2 aria-hidden />} pending={busy === 'remove'} onClick={() => void run('remove', async () => { await removeDevice(own.id); const subscription = await currentSubscription(); await subscription?.unsubscribe(); })}>{t('away.device.remove')}</Button>
        </>}
        {(state === 'failed' || error !== null) && <Button size="sm" icon={<RefreshCw aria-hidden />} pending={busy !== null} onClick={() => void (errorAct === 'enable' && key !== undefined && key !== null ? run('enable', () => enablePush(key)) : run('retry', refresh))}>{t('away.retry')}</Button>}
      </div>
      {own !== undefined && <p className="text-xs text-text-muted">{t('away.outcome.lastAccepted')} {own.last_ok_at === null ? t('away.notTested') : <RelativeTime at={own.last_ok_at} />}</p>}
      {result !== null && <InlineAlert tone={result.ok ? 'success' : 'danger'}>{result.ok ? t('away.outcome.accepted') : t('away.outcome.testFailed', { reason: result.error ?? '' })}</InlineAlert>}
      {error !== null && <ErrorAlert error={error} message={errorAct === 'test' ? t('away.outcome.notConfirmed') : undefined} />}
      {(here.error !== null || devices.error !== null) && <ErrorAlert error={here.error ?? devices.error} />}
      {(devices.data ?? []).filter((device) => device.id !== own?.id).length > 0 && <div className="flex flex-col gap-1 border-t border-border pt-2"><h4 className="text-xs font-medium text-text-muted">{t('away.device.other')}</h4><ul className="flex flex-col gap-2">{(devices.data ?? []).filter((device) => device.id !== own?.id).map((device) => <li key={device.id} className="flex flex-wrap items-center justify-between gap-2 text-sm text-text"><span className="min-w-0 wrap-anywhere">{device.label}</span><Button size="sm" variant="ghost" icon={<Trash2 aria-hidden />} pending={busy === device.id} onClick={() => void run(device.id, () => removeDevice(device.id))}>{t('away.outcome.remove')}</Button></li>)}</ul></div>}
      <p className="text-xs text-text-muted">{t('away.device.proof')}</p>
    </section>
  );
}
