/** The same read-only, permission-cut account of agent reach on an environment's page and beside a thread. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { environmentReachQuery } from '@/api/environments';
import { PlaceLink } from '@/features/navigation/places';
import { useNodeLabel } from '@/features/navigation/placeLabels';
import { EnvironmentNetwork } from '@/features/resources/environments/Network';
import { Facts } from '@/features/resources/Facts';
import type { AgentReachOverview, Environment, NodeRef } from '@/generated/api-types';
import { holds, permissionLabel } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { HelpHint } from '@/ui/HelpHint';

type ReachEnvironment = Pick<Environment, 'id' | 'name' | 'access' | 'job'>;

function ReachPlace({ place }: { place: NodeRef }) {
  const { t } = useTranslation();
  const label = useNodeLabel(place.id, place.redacted ? undefined : place.name ?? undefined);
  return label === undefined ? <span>{t('agentReach.privatePlace')}</span> : <PlaceLink place={place} className="text-accent-text underline underline-offset-2">{label}</PlaceLink>;
}

/** An unfamiliar arrangement stays unknown; it is never treated as a separated key. */
function placement(t: TFunction, value: string): string {
  switch (value) {
    case 'separated': return t('agentReach.placement.separated');
    case 'key_separated': return t('agentReach.placement.key_separated');
    case 'credential_shared': return t('agentReach.placement.credential_shared');
    case 'handed_over': return t('agentReach.placement.handed_over');
    case 'together': return t('agentReach.placement.together');
    case 'unbound': return t('agentReach.placement.unbound');
    default: return t('agentReach.notReported');
  }
}

function mountMode(t: TFunction, value: string): string {
  switch (value) {
    case 'read_only': return t('agentReach.mountMode.read_only');
    case 'shared': return t('agentReach.mountMode.shared');
    case 'private': return t('agentReach.mountMode.private');
    default: return t('agentReach.notReported');
  }
}

function placementName(t: TFunction, value: string): string {
  const known = (['separated', 'key_separated', 'credential_shared', 'handed_over', 'together', 'unbound'] as const).find((name) => name === value);
  return known === undefined ? t('agentReach.notReported') : t(`agentReach.arrangements.${known}.name`);
}

function ReachFacts({ reach, environment, compact }: { reach: AgentReachOverview; environment: ReachEnvironment; compact: boolean }) {
  const { t } = useTranslation();
  const explanations = [...new Set(reach.credentials.map((item) => placement(t, item.placement)))];
  const unavailable = (permission: string) => <span className="text-text-muted">{t('agentReach.needs', { permission: permissionLabel(permission) })}</span>;
  return <>
    <EnvironmentNetwork environment={{ ...environment, network: reach.network }} manage={holds(environment.access, 'environment.manage')} compact={compact} />
    <Facts compact={compact} label={t('agentReach.title')} rows={[
      { name: t('agentReach.secrets'), value: reach.secrets.names === null ? unavailable('environment.manage') : <div className="flex flex-col gap-2">
        {reach.secrets.names.length === 0 ? <EmptyState compact title={t('agentReach.noSecrets')} /> : <ul className="flex flex-wrap gap-x-3 gap-y-1">{reach.secrets.names.map((name) => <li key={name} className="font-mono text-sm wrap-anywhere">{name}</li>)}</ul>}
        {reach.secrets.source !== null && <p className="text-sm text-text-muted">{t('agentReach.source')}{' '}<ReachPlace place={reach.secrets.source} /></p>}
        <Link to="/resources/environments/$id" params={{ id: environment.id }} search={{ tab: 'settings' }} className="text-sm text-accent-text underline">{t('agentReach.openSettings')}</Link>
      </div> },
      { name: t('agentReach.mounts'), value: reach.mounts === null ? unavailable('files.view') : reach.mounts.length === 0 ? <EmptyState compact title={t('agentReach.noMounts')} /> : <ul className="flex flex-col gap-2">{reach.mounts.map((item) => <li key={`${item.mount.id}:${item.path}`} className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
        <ResourceChip subject={{ kind: 'mount', mount: item.mount }} asChild hint={false}><Link to="/resources/mounts/$id" params={{ id: item.mount.id }} className="text-accent-text underline">{item.mount.name}</Link></ResourceChip>
        <code className="text-sm wrap-anywhere">{item.path}</code><span className="text-sm text-text-muted">{mountMode(t, item.mode)}</span>
      </li>)}</ul> },
      { name: <span className="inline-flex items-center gap-1">{t('agentReach.credentials')}{compact && explanations.length > 0 && <HelpHint term={t('agentReach.credentials')} what={<ul className="flex flex-col gap-2">{explanations.map((text) => <li key={text}>{text}</li>)}</ul>} />}</span>, value: reach.credentials.length === 0 ? <EmptyState compact title={t('agentReach.noCredentials')} /> : <ul className={compact ? 'flex flex-col gap-1' : 'flex flex-col gap-3'}>{reach.credentials.map((item) => <li key={item.thread.id} className={compact ? 'flex flex-wrap items-baseline gap-x-2 gap-y-1' : 'flex flex-col gap-1'}>
        <ReachPlace place={item.thread} /><span className="text-sm text-text-muted">{compact ? placementName(t, item.placement) : placement(t, item.placement)}</span>
        {item.placement === 'credential_shared' && item.expires_at !== null && <p className="text-sm text-text-muted">{t('agentReach.expires')}{' '}<RelativeTime at={item.expires_at} /></p>}
      </li>)}</ul> },
      { name: t('agentReach.profiles'), value: reach.browser_profiles === null ? unavailable('browser.view') : <div className="flex flex-col gap-2">
        {reach.browser_profiles.length === 0 ? <EmptyState compact title={t('agentReach.noProfiles')} /> : <ul className="flex flex-col gap-1">{reach.browser_profiles.map((profile) => <li key={profile.name} className="text-sm wrap-anywhere">{profile.name}</li>)}</ul>}
        <Link to="/resources/environments/$id" params={{ id: environment.id }} search={{ tab: 'browsers' }} className="text-sm text-accent-text underline">{t('agentReach.openBrowsers')}</Link>
      </div> },
    ]} />
  </>;
}

export function AgentReach({ environment, compact = false }: { environment: ReachEnvironment; compact?: boolean }) {
  const { t } = useTranslation();
  const query = useQuery(environmentReachQuery(environment.id));
  return <section aria-label={t('agentReach.title')} aria-busy={query.isFetching} className={compact ? 'flex min-w-0 flex-col gap-3 border-b border-border px-3 py-3' : 'flex min-w-0 flex-col gap-3'}>
    <h3 className="text-sm font-medium text-text">{t('agentReach.title')}</h3>
    {query.isPending ? <SkeletonRows rows={5} /> : query.isError ? <div className="flex flex-col gap-2">
      {query.dataUpdatedAt > 0 && <p className="text-sm text-text-muted">{t('agentReach.lastRead')}{' '}<RelativeTime at={new Date(query.dataUpdatedAt).toISOString()} /></p>}
      <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} action={<Button size="sm" pending={query.isRefetching} onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />
    </div> : <>
      {query.isFetching && <InlineAlert tone="info">{t('agentReach.checking')}</InlineAlert>}
      <ReachFacts reach={query.data} environment={environment} compact={compact} />
      <Button size="sm" variant="ghost" pending={query.isFetching} onClick={() => void query.refetch()}>{t('actions.refresh.label')}</Button>
    </>}
  </section>;
}
