/** The actual and requested network, its safe policy sources, and recovery through the existing change plan. */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { openChange } from '@/features/change/target';
import { SettingHelpLink } from '@/features/help/SettingHint';
import { GuideLink } from '@/features/help/GuideLink';
import { GuideRail } from '@/features/help/GuideRail';
import { PlaceLink } from '@/features/navigation/places';
import type { Environment, NetworkState } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';

/** The retained requirement prevents a generic Start from reopening an old open unit. */
export function keptStopped(network: NetworkState): boolean {
  return network.required_mode === 'restricted' && network.mode === 'open';
}

/** One sentence shared with the Ports panel about a program's own outbound path. */
export function OpenEgressNotice({ network }: { network: Pick<NetworkState, 'mode'> }) {
  const { t } = useTranslation();
  if (network.mode !== 'open') return null;
  return <InlineAlert tone="info"><span>{t('egress.outboundTunnel')}</span>{' '}<SettingHelpLink settingKey="network.allowed_hosts" /></InlineAlert>;
}

/** Shared facts for the resource page and the thread's Environment panel. */
export function EnvironmentNetwork({ environment, manage, compact = false }: {
  environment: Pick<Environment, 'id' | 'name' | 'network' | 'job'> & { access: Pick<Environment['access'], 'permissions'> };
  manage: boolean;
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const network = environment.network;
  const details = environment.access.permissions.some((permission) => permission !== 'access.manage' && permission !== 'access.external');
  const guarded = keptStopped(network);
  const status = network.state !== 'ready' && environment.job !== null ? 'applying' : guarded ? 'keptStopped' : network.state === 'ready' ? 'ready' : 'unavailable';
  const mode = network.network_mode === 'connected' ? 'connected' : network.network_mode === 'none' ? 'onlyOrch' : network.mode;
  const outbound = network.mode === 'open';
  const about = t(network.network_mode === 'connected' ? 'egress.machineNetwork' : 'egress.openAbout');
  return (
    <section aria-label={t('egress.title')} className="flex min-w-0 flex-col gap-3" data-testid="environment-network">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium text-text">{t('egress.title')}</h3>
        <GuideRail label={t('egress.title')}>
          {compact && <><p>{about}</p>{outbound && <p>{t('egress.outboundTunnel')}</p>}</>}
          <GuideLink guide="let-agents-open-ports" target={{ kind: 'environment', id: environment.id }} /><GuideLink guide="change-a-running-environment" target={{ kind: 'environment', id: environment.id }} />
        </GuideRail>
        <Badge>{t(`egress.${mode}`)}</Badge>
        <span className="text-sm text-text-muted" role="status">{t(`egress.${status}`)}</span>
      </div>
      {!compact && <OpenEgressNotice network={network} />}
      {network.required_mode !== network.mode && <p className="text-sm text-text-muted">{t('egress.requested', { mode: t(`egress.${network.required_mode}`) })}</p>}
      {network.reason !== null && <InlineAlert tone={guarded ? 'warning' : 'info'}>{network.reason}</InlineAlert>}
      {!details ? <p className="text-sm text-text-muted">{t('egress.detailsHidden')}</p> : network.mode === 'restricted' || network.required_mode === 'restricted' ? (
        <>
          <div className="flex flex-col gap-1 text-sm">
            <h4 className="font-medium text-text">{t('egress.always')}</h4>
            {network.always.length === 0 ? <EmptyState compact title={t('egress.transportUnknown')} /> : (
              <ul className="flex flex-col gap-1">{network.always.map((transport) => <li key={`${transport.protocol}:${transport.authority}:${transport.port}:${transport.purpose}`} className="flex flex-wrap gap-x-3 gap-y-1 break-all text-text-muted"><code>{transport.authority}:{transport.port}</code><span>{t(`egress.purpose.${transport.purpose}`)}</span></li>)}</ul>
            )}
          </div>
          <div className="flex flex-col gap-1 text-sm">
            <h4 className="font-medium text-text">{t('egress.otherHosts')}</h4>
            {network.allowed_hosts.length === 0 ? <EmptyState compact title={t('egress.onlyOrch')} /> : <ul className="flex flex-wrap gap-x-3 gap-y-1">{network.allowed_hosts.map((host) => <li key={host} className="break-all font-mono text-text-muted">{host}</li>)}</ul>}
            {network.sources.map((source, index) => <p key={`${source.scope?.id ?? 'own'}:${index}`} className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted">{source.scope === null ? t('egress.unnamedSource') : <PlaceLink place={source.scope} className="text-accent-text underline">{source.scope.name ?? t('ui.nameUnknown')}</PlaceLink>}{source.hosts.length > 0 && <span>{source.hosts.join(', ')}</span>}</p>)}
          </div>
          {network.last_denied !== null && <p className="flex flex-wrap gap-x-3 gap-y-1 break-words text-sm text-text-muted"><span>{t('egress.denied', { host: network.last_denied.host })}</span><span>{network.last_denied.reason}</span><RelativeTime at={network.last_denied.at} /></p>}
          {manage && <div className="flex flex-wrap items-center gap-3 text-sm">
            <a href={`/resources/environments/${environment.id}?tab=settings`} className="text-accent-text underline">{t('egress.editHosts')}</a>
            {network.state !== 'ready' && <Button size="sm" disabled={environment.job !== null} onClick={() => openChange({ kind: 'environment', id: environment.id, name: environment.name }, { kind: 'update' })}>{t('egress.retry')}</Button>}
          </div>}
        </>
      ) : !compact && <p className="text-sm text-text-muted">{about}</p>}
    </section>
  );
}
