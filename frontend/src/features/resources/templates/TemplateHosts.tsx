/** Template advice distinguishes missing reports from enrolled hosts that are currently offline. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { hostsQuery } from '@/api/resources';
import type { Host, UnitKind } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatExact } from '@/lib/time';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

type AdviceHost = Pick<Host, 'id' | 'name' | 'machine' | 'drivers' | 'offline_since'>;
interface Props { kind: UnitKind; driver: string | null; hasSchema: boolean }

export function HostAdvice({ hosts, kind, driver, hasSchema }: Props & { hosts: readonly AdviceHost[] }) {
  const { t, i18n } = useTranslation();
  const timeZone = useAppearance((state) => state.timeZone);
  const matching = hosts.filter((host) => driver !== null && host.drivers.some((offered) => offered.name === driver && offered.creates.includes(kind)));
  if (hasSchema && matching.some((host) => host.machine.online)) return null;
  const candidates = matching.length > 0 ? matching : hosts;
  const offline = candidates.filter((host) => !host.machine.online);
  const state = hosts.length === 0 ? 'none' : offline.length === candidates.length ? 'offline' : 'unreported';
  const context = { locale: i18n.resolvedLanguage ?? i18n.language, timeZone };
  return <InlineAlert tone="warning" title={t(`templates.hostAdvice.${state}.name`)}>
    <p>{t(`templates.hostAdvice.${state}.description`)}</p>
    {offline.length > 0 && <ul className="mt-2 flex flex-col gap-1">{offline.map((host) => <li key={host.id}>
      {host.offline_since === null || host.offline_since === undefined
        ? t('templates.hostAdvice.offlineUnknown', { name: host.name })
        : <time dateTime={host.offline_since}>{t('templates.hostAdvice.offlineSince', { name: host.name, when: formatExact(host.offline_since, context) })}</time>}
    </li>)}</ul>}
    <p className="mt-2">{t(hasSchema ? 'templates.hostAdvice.cached' : 'templates.hostAdvice.noSchema')}</p>
  </InlineAlert>;
}

export function TemplateHosts(props: Props) {
  const hosts = useQuery(hostsQuery(null));
  if (hosts.isPending) return <SkeletonRows rows={1} />;
  if (hosts.isError) return <ErrorState compact error={hosts.error} onRetry={() => void hosts.refetch()} />;
  return <HostAdvice {...props} hosts={hosts.data} />;
}
