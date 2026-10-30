/** Checked scoped ways and one editor link without changing the ordinary subscription bell. */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Clock, Pause, Play, RefreshCw, Settings } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { awayPrefsQuery, outsideQuery } from '@/api/away';
import { ApiError } from '@/api/client';
import { useAppearance } from '@/lib/appearance';
import { DropdownEntries } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import type { AwayPrefs } from './types';
import { useAwayLive } from './live';
import { writePause } from './time';

export function OutsideMenu({ nodeId, onClose, onPause }: { nodeId: string; onClose: () => void; onPause: (mode: 'pause' | 'resume', prefs: AwayPrefs) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  useAwayLive();
  const query = useQuery(outsideQuery(nodeId));
  const prefs = useQuery(awayPrefsQuery);
  const zone = useAppearance((state) => state.timeZone);
  const locale = useAppearance((state) => state.locale);
  const refused = query.error instanceof ApiError && (query.error.status === 403 || query.error.status === 404);
  const outside = refused ? undefined : query.data?.outside;
  const label = outside === undefined ? t(refused ? 'away.refused' : query.isPending ? 'away.checking' : 'away.failed') : outside.pause_until !== null ? t('away.status.paused', { time: writePause(outside.pause_until, zone, locale) }) : outside.ways.length === 0 ? t('away.status.off') : t('away.status.on', { ways: new Intl.ListFormat(locale).format(outside.ways.map((way) => way.name)) });
  return (
    <>
      <div className="my-1 border-t border-border px-2 pt-2 pb-1" role="status">
        <p className="text-sm font-medium text-text">{label}</p>
        {outside?.ways.some((way) => way.state === 'needs_attention') === true && <p className="text-xs text-warning-text">{t('away.attention')}</p>}
        {query.isError && outside !== undefined && <p className="text-xs text-warning-text">{t('away.stale')}</p>}
        {outside !== undefined && <p className="text-xs text-text-muted">{t('away.checked')} <RelativeTime at={new Date(query.dataUpdatedAt).toISOString()} /></p>}
      </div>
      <DropdownEntries entries={[
        ...(query.isError || outside === undefined && !query.isPending && !refused ? [{ id: 'outside-retry', label: t('away.retry'), icon: <RefreshCw aria-hidden />, onSelect: () => void query.refetch() }] : []),
        { id: 'outside-edit', label: t('away.edit'), icon: <Settings aria-hidden />, onSelect: () => { onClose(); void navigate({ href: '/settings/notifications#outside' }); } },
        ...(prefs.data?.revision === undefined ? [] : [
          { id: 'outside-pause', label: t('away.pause.button'), icon: <Pause aria-hidden />, onSelect: () => { if (prefs.data !== undefined) onPause('pause', prefs.data); } },
          ...(prefs.data.pause_until === null ? [] : [{ id: 'outside-resume', label: t('away.pause.resume'), icon: <Play aria-hidden />, onSelect: () => { if (prefs.data !== undefined) onPause('resume', prefs.data); } }]),
        ]),
      ]} />
    </>
  );
}

/** A first-work list consumes its completed org projection, not a route inferred from a device or role. */
export function ConfiguredAway({ configured, nodeId }: { configured: boolean; nodeId: string }) {
  const { t } = useTranslation();
  const query = useQuery({ ...outsideQuery(nodeId), enabled: configured });
  const outside = query.data?.outside;
  return <div className="flex flex-col gap-1 text-xs text-text-muted"><p>{t(configured ? 'away.configured' : 'away.status.off')}</p>{configured && <>{outside?.pause_until !== null && outside?.pause_until !== undefined && <p className="flex items-center gap-1"><Clock aria-hidden className="size-4" />{t('away.pause.active')}</p>}{outside?.ways.some((way) => way.state === 'needs_attention') === true && <p>{t('away.attention')}</p>}{outside?.ways.some((way) => way.last_ok_at === null) === true && <p>{t('away.notTested')}</p>}</>}</div>;
}
