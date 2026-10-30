/** Six sections for a provider: observed facts, models, credentials, usage, settings and activity.
 * Runtime, source, arrangement, access and lifecycle controls live in Settings; old bookmarks still
 * reach their content. Only the active section loads, and the setup banner owns its next action.
 */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { PlugZap } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { providerQuery } from '@/api/resources';
import { HeadingActs } from '@/features/resources/acts';
import { adminSpec } from '@/ui/AdminAct';
import { HarnessSessions, harnessesOfProvider, ProviderActivityStatus } from '@/features/resources/adopt/Sessions';
import { useProviderActs } from '@/features/resources/providers/acts';
import { ProviderArrangements } from '@/features/resources/providers/Arrangements';
import { ProviderDangerZone } from '@/features/resources/providers/DangerZone';
import { ProviderKey } from '@/features/resources/providers/Key';
import { ProviderModels } from '@/features/resources/providers/Models';
import { ProviderRuntime } from '@/features/resources/providers/Runtime';
import { ProviderSettings } from '@/features/resources/providers/Settings';
import { providerState } from '@/features/resources/providers/state';
import { ProviderThreads } from '@/features/resources/providers/Threads';
import { ProviderSetupNotice } from './SetupNotice';
import { ProviderReadFailure } from './ReadFailure';
import { PlanPanel } from './PlanPanel';
import { ProviderUsage } from './Usage';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import type { Provider as ProviderRow } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { useNow } from '@/lib/time';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { ErrorAlert } from '@/ui/ErrorState';
import { Button } from '@/ui/Button';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { Tabs } from '@/ui/Tabs';

import { PROVIDER_TABS, providerTab, type ProviderTab } from '@/features/resources/tabs';
import type { UsageSearch } from '@/features/usage/search';
import type { SearchPatch } from '@/features/usage/choices';

export { PROVIDER_TABS, providerTab, type ProviderTab };

function Skeletons() {
  return (
    <Page>
      <SkeletonGroup className="flex flex-col gap-6">
        <div className="flex items-center gap-4">
          <Skeleton className="size-10 rounded-lg" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-6 w-1/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        </div>
        <Skeleton className="h-8 w-full" />
        <SkeletonRows rows={5} />
      </SkeletonGroup>
    </Page>
  );
}

function Overview({ provider }: { provider: ProviderRow }) {
  return <PlanPanel provider={provider} resets />;
}

export interface ProviderScreenProps {
  usageSearch?:UsageSearch | undefined;
  onUsageSearch?:((patch:SearchPatch)=>void) | undefined;
  providerId: string;
  tab: ProviderTab;
  onTab: (tab: ProviderTab) => void;
}

export function ProviderScreen({ providerId, tab, onTab,usageSearch,onUsageSearch }: ProviderScreenProps) {
  const { t } = useTranslation();
  const query = useQuery(providerQuery(providerId));
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const navigate = useNavigate();
  const acts = useProviderActs({ onTab: (_provider, next) => onTab(next), afterDelete: () => void navigate({ to: '/resources/providers' }) });
  const back = useResourceHeading('providers', PlugZap, query.data?.name ?? t('common.loading'), query.data?.id);

  if (query.isPending) return <Skeletons />;
  if (query.data === undefined) return <ProviderReadFailure id={providerId} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const provider = query.data;
  const manage = holds(provider.access, 'provider.manage');
  const canUse = holds(provider.access, 'provider.use');
  const state = providerState(provider, t, { locale, timeZone }, now);
  const page = acts.page(provider);

  const tabs = [
    { value: 'overview' as const, label: t('providers.tabs.overview'), panel: <Overview provider={provider} /> },
    { value: 'models' as const, label: t('providers.tabs.models'), count: provider.models.length, panel: <ProviderModels provider={provider} manage={manage} /> },
    ...(manage ? [{ value: 'key' as const, label: t('providers.tabs.credentials'), panel: <ProviderKey provider={provider} /> }] : []),
    { value: 'usage' as const, label: t('providers.tabs.usage'), panel: <ProviderUsage provider={provider} search={usageSearch} onSearch={onUsageSearch} /> },
    ...(manage ? [{ value: 'settings' as const, label: t('providers.tabs.settings'), panel: <div className="flex min-w-0 flex-col gap-4">
      <ProviderRuntime provider={provider} configuration={false} />
      <ProviderSettings provider={provider} />
      <ProviderArrangements provider={provider} />
      <ResourceAccess access={provider.access} target={{ kind: 'provider', id: provider.id, name: provider.name }} />
      <ProviderDangerZone provider={provider} onDisable={() => acts.ask('disable', provider)} />
    </div> }] : []),
    { value: 'activity' as const, label: t('providers.tabs.activity'), panel: <div className="flex min-w-0 flex-col gap-4">
      <ProviderThreads provider={provider} />
      {(manage || canUse) && harnessesOfProvider(provider).length > 0 && <HarnessSessions showProvider={false} harnesses={harnessesOfProvider(provider)} />}
    </div> },
  ];
  // An address that names a tab this reader has none of shows the overview.
  const requested = providerTab(tab) ?? 'overview';
  const shown = tabs.some((candidate) => candidate.value === requested) ? requested : 'overview';
  const headerStatus = state.next === null && provider.status !== 'needs_setup';

  return (
    <Page>
      <PageHeader
        adminAccess={provider.access.admin_access} ownership={provider} opening={{ kind: 'provider', id: provider.id }}
        back={back}
        mark={
          <PlaceMark>
            <PlugZap aria-hidden />
          </PlaceMark>
        }
        title={provider.name}
        facts={[
          ...(headerStatus ? [shown === 'activity' && provider.status === 'ready'
            ? <ProviderActivityStatus key="status" harnesses={manage || canUse ? harnessesOfProvider(provider) : []} />
            : <StatusDot key="status" tone={state.tone} label={state.text} showLabel />] : []),
        ]}
        actions={<HeadingActs admin={adminSpec(provider, 'provider.manage')} name={provider.name} primary={shown === 'settings' ? null : page.primary} edit={page.edit} menu={page.menu} />}
      />
      <ProviderSetupNotice provider={provider} manage={manage} currentTab={shown} onTab={(next) => onTab(providerTab(next) ?? 'overview')} />
      {query.isError && <ErrorAlert error={query.error} action={<Button size="sm" pending={query.isRefetching} onClick={()=>void query.refetch()}>{t('common.tryAgain')}</Button>}/>}
      <Tabs label={t('providers.tabs.label')} value={shown} onValueChange={onTab} tabs={tabs} panelClassName="pt-3" />
      {acts.dialogs}
    </Page>
  );
}
