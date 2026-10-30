/** An environment as a resource: owners, home and address, place, the threads in it and where each runs, its page of two levels and who reaches into it, its Tools, pending updates, its own settings, and removal with its reason; for a directory on a person's machine, its switches.
 *
 * The page says in one sentence what the environment is now ("~/work on Alice's laptop · 3 threads of
 * Fix login · online"), and under it the facts: its owner, where it comes from, where it runs, what it
 * was made from, how far behind it is, when it is to be removed and why. The threads in it that the
 * reader may see, each with its topic, its owner and its directory, are a tab of their own; so are the
 * limits of a person's machine that bound it, its settings and who holds it. Whoever manages it
 * switches files written from outside and shells on or off, renames it, starts and stops one that Orch
 * made, switches it off and removes it, naming it to be sure. Updating, moving and its Tools are not
 * served yet and are not offered.
 */
import { environmentRuntimeRows, namesRuntime, RuntimePair } from '@/features/resources/isolation/RuntimeInfo';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ENVIRONMENT_TABS, type EnvironmentTab } from '@/features/resources/tabs';
import { Link, useNavigate } from '@tanstack/react-router';
import { ArrowRightLeft, Ban, Boxes, CirclePlay, Hourglass } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { environmentKey, environmentQuery, updateEnvironment } from '@/api/environments';
import { ApiError } from '@/api/client';
import { DetailPages, DetailSections, ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { PortsPanel } from '@/features/ports/Ports';
import { BrowsersPanel } from '@/features/browser/Browsers';
import { portsBrowser } from '@/features/browser/View';
import { DisplaysTab } from '@/features/vnc/View';
import { PortsExposure } from '@/features/ports/Exposure';
import { TopicPlace } from '@/features/navigation/PlaceChain';
import { adminSpec } from '@/ui/AdminAct';
import { useEnvironmentActs } from '@/features/resources/environments/acts';
import { openChange } from '@/features/change/target';
import { EnvironmentSettings } from '@/features/resources/environments/Settings';
import { UnitCommandFailure } from '@/features/resources/UnitConfiguration';
import { EnvironmentDoor } from '@/features/resources/environments/Doors';
import { HarnessTrustSection } from '@/features/resources/environments/HarnessTrust';
import { AgentReach } from '@/features/resources/environments/AgentReach';
import { DataSourcesSummary } from '@/features/resources/environments/DataSources';
import { TemporaryStatus } from '@/features/resources/environments/Temporary';
import { McpServers } from '@/features/resources/environments/McpServers';
import { EnvironmentStatusText } from '@/features/resources/environments/EnvironmentList';
import { environmentChangesVisible, environmentDisplayStatus, environmentRemovalPhase, environmentStatusDetail, holding, isBusy } from '@/features/resources/environments/words';
import { Facts } from '@/features/resources/Facts';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { PathValue, RuntimeChange } from '@/features/resources/TechnicalValue';
import { EnvironmentPrograms } from '@/features/resources/agents/ProgramBuild';
import { Hint } from '@/features/help/Hint';
import { MachineLimits } from '@/features/resources/machines/Policy';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import type { Environment as EnvironmentRow } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { formatDateTime, useNow } from '@/lib/time';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';
import { type Column, Table } from '@/ui/Table';
import { SettingRow } from '@/ui/SettingRow';
import { UserChip } from '@/ui/UserChip';

export { ENVIRONMENT_TABS, type EnvironmentTab };

/** Direct doors fill the remaining pane, leaving scrolling to their tree or terminal alone. */
function DoorPage({ children }: { children: ReactNode }) {
  return <div className="mx-auto flex h-full min-h-0 w-full max-w-(--container-page) flex-col gap-4 overflow-hidden px-4 py-4 sm:px-6">{children}</div>;
}

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

function Switches({ environment, manage }: { environment: EnvironmentRow; manage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const change = useMutation({
    mutationFn: (body: { remote_write?: boolean; terminal?: boolean }) => updateEnvironment(environment.id, { expected_revision: environment.revision, ...body }),
    onSuccess: (updated) => queryClient.setQueryData(environmentKey(updated.id), updated),
    // Somebody changed it first: it is read again before the next try.
    onError: (error) => { if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: environmentKey(environment.id) }); },
  });
  // What was asked is shown as asked, and as waiting, until the server has answered. A switch that has its answer coming stays where it is, so that the keyboard stays on it.
  const asked = change.isPending ? change.variables : {};
  const write = asked.remote_write ?? environment.remote_write;
  const shell = asked.terminal ?? environment.terminal;
  return (
    <ul className="min-w-0 divide-y divide-border rounded-lg border border-border">
      <SettingRow name={t('environments.overview.remoteWrite')} help={<Hint id="env-remote-write" />} readOnly={!manage} value={
      <Switch
        label={<span className="sr-only">{t('environments.overview.remoteWrite')}</span>}
        checked={write}
        disabled={!manage}
        onCheckedChange={(next) => !change.isPending && change.mutate({ remote_write: next })}
      />
      } />
      <SettingRow name={t('environments.overview.terminal')} help={<Hint id="env-terminal" />} readOnly={!manage} value={
      <Switch
        label={<span className="sr-only">{t('environments.overview.terminal')}</span>}
        checked={shell}
        disabled={!manage}
        onCheckedChange={(next) => !change.isPending && change.mutate({ terminal: next })}
      />
      } />
      {change.error !== null && <li><ErrorAlert error={change.error} /></li>}
    </ul>
  );
}

function Overview({ environment }: { environment: EnvironmentRow }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const context = { locale, timeZone };
  return (
    <div className="flex flex-col gap-6">
      {environment.status_detail !== null && environmentRemovalPhase(environment) === undefined && <InlineAlert tone={environment.status === 'failed' ? 'danger' : 'info'}>{environmentStatusDetail(environment, t)}</InlineAlert>}
      {environment.remove_at !== null && environmentRemovalPhase(environment) === undefined && (environment.temporary === null || environment.temporary === undefined) && (
        <InlineAlert tone="warning" title={t('environments.overview.removalScheduled', { time: formatDateTime(environment.remove_at, context, now) })}>
          {environment.remove_reason === null ? undefined : t(`environments.overview.removeReason.${environment.remove_reason}`)}
        </InlineAlert>
      )}
      {environmentChangesVisible(environment) && (environment.updates.length > 0 || environment.update_required || environment.connector_update?.behind === true) && (
        <InlineAlert tone="warning" title={environment.update_required ? t('environments.list.updateRequired') : t('environments.overview.behind')} action={holds(environment.access, 'environment.manage') && environment.template !== null ? <Button size="sm" onClick={() => openChange({ kind: 'environment', id: environment.id, name: environment.name }, { kind: 'update' })}>{t('change.env.menu.update')}</Button> : undefined}>
          <span className="flex flex-col gap-1">
            {environment.connector_update?.behind === true && <p>{t('componentUpdates.versions', {
              recommended: `v${environment.connector_update.recommended_version.replace(/^v/, '')}`,
              running: `v${environment.connector_update.running_version.replace(/^v/, '')}`,
            })}</p>}
            <ul className="list-disc pl-4">
              {environment.updates.map((update) => (
                <li key={update.name}><RuntimeChange change={update} /></li>
              ))}
            </ul>
          </span>
        </InlineAlert>
      )}
      <EnvironmentPrograms id={environment.id} generation={environment.generation} />
      <Section id="environment-about" title={t('environments.overview.about')}>
        <Facts compact
          label={t('environments.overview.about')}
          rows={[
            { name: t('environments.overview.home'), value: environment.home.name ?? '' },
            {
              name: t('environments.overview.runs'),
              value:
                environment.machine !== null
                  ? t('environments.overview.onMachine', { machine: environment.machine.name, state: environment.machine.online ? t('machines.state.online') : t('machines.state.offline') })
                  : environment.host !== null
                    ? <ResourceName subject={{ kind: 'host', host: environment.host }} />
                    : t('environments.how.none'),
            },
            ...environmentRuntimeRows(environment.isolation, t),
            { name: t('environments.overview.root'), value: <PathValue path={environment.workspace_root} /> },
            ...(environment.template === null ? [] : [{ name: t('environments.overview.template'), value: <ResourceName subject={{ kind: 'template', template: environment.template }} hint={false} /> }]),
            { name: t('environments.overview.reach'), value: t('environments.overview.reachValue', { count: environment.reach }) },
            ...(environment.last_activity_at === null ? [] : [{ name: t('environments.overview.lastActivity'), value: <RelativeTime at={environment.last_activity_at} /> }]),
            ...(environment.idle_stop_at === null ? [] : [{ name: t('environments.overview.idleStop'), value: formatDateTime(environment.idle_stop_at, context, now) }]),
            { name: t('environments.overview.may'), value: t(`environments.holding.${holding(environment.access.permissions)}`) },
          ]}
        />
      </Section>
      <ResourceDetails><AgentReach environment={environment} /></ResourceDetails>
    </div>
  );
}

function Threads({ environment, manage }: { environment: EnvironmentRow; manage: boolean }) {
  const { t } = useTranslation();
  const columns: Column<EnvironmentRow['threads'][number]>[] = [
    {
      id: 'thread',
      header: t('environments.threads.thread'),
      cell: (row, shown) => {
        const withoutTopic = !shown.has('topic');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <Link to="/thread/$threadId" params={{ threadId: row.thread.id }} className="truncate font-medium text-text hover:underline">
              {row.thread.name ?? t('providers.threads.hiddenName')}
            </Link>
            {withoutTopic && (
              <>
                <Link to="/topic/$topicId" params={{ topicId: row.topic.id }} className="truncate text-sm text-text-muted hover:underline">
                  {row.topic.name ?? ''}
                </Link>
                <TopicPlace topic={row.topic} className="text-sm" />
              </>
            )}
          </div>
        );
      },
    },
    {
      id: 'topic',
      header: t('environments.threads.topic'),
      from: 'sm',
      cell: (row) => (
        <span className="flex min-w-0 flex-col">
          <Link to="/topic/$topicId" params={{ topicId: row.topic.id }} className="truncate text-text hover:underline">
            {row.topic.name ?? ''}
          </Link>
          <TopicPlace topic={row.topic} className="text-sm" />
        </span>
      ),
    },
    { id: 'owner', header: t('environments.threads.owner'), from: 'md', cell: (row) => <UserChip user={row.owner} size={24} /> },
    { id: 'directory', header: t('environments.threads.directory'), from: 'sm', cell: (row) => (row.directory === '' ? <span className="text-text-subtle">{t('environments.threads.root')}</span> : <code className="text-sm">{row.directory}</code>) },
  ];
  return (
    <Section
      id="environment-threads"
      title={t('environments.threads.title')}
      lead={t('environments.threads.lead')}
      action={
        manage && (environment.threads.length > 0 || environment.threads_hidden > 0) ? (
          <Button icon={<ArrowRightLeft aria-hidden />} onClick={() => openChange({ kind: 'environment', id: environment.id, name: environment.name }, { kind: 'switch' })}>
            {t('change.env.menu.moveThreads')}
          </Button>
        ) : undefined
      }
    >
      {environment.threads.length === 0 && environment.threads_hidden === 0 ? (
        <EmptyState compact icon={<Boxes aria-hidden />} title={t('environments.threads.none')} />
      ) : (
        <div className="flex flex-col gap-2">
          {environment.threads.length > 0 && <DetailPages items={environment.threads}>{(page) => <Table label={t('environments.threads.title')} columns={columns} rows={page} rowKey={(row) => row.thread.id} />}</DetailPages>}
          {environment.threads_hidden > 0 && <p className="text-sm text-text-muted">{t('environments.threads.hidden', { count: environment.threads_hidden })}</p>}
        </div>
      )}
    </Section>
  );
}

function DangerZone({ environment, onEnable, onDisable }: { environment: EnvironmentRow; onEnable: () => void; onDisable: () => void }) {
  const { t } = useTranslation();
  const disabled = environment.status === 'disabled';
  return (
    <div className="flex flex-col gap-6">
      <Section id="environment-switch" title={disabled ? t('environments.danger.enableSection') : t('environments.danger.disableSection')} lead={disabled ? t('environments.danger.enableLead') : t('environments.danger.disableLead')}>
        <div>
          {disabled ? (
            <Button icon={<CirclePlay aria-hidden />} onClick={onEnable}>
              {t('environments.danger.enable')}
            </Button>
          ) : (
            <Button icon={<Ban aria-hidden />} onClick={onDisable}>
              {t('environments.danger.disable')}
            </Button>
          )}
        </div>
      </Section>
    </div>
  );
}

export interface EnvironmentScreenProps {
  environmentId: string;
  wantedKey?: string;
  initialPort?: number;
  tab: EnvironmentTab;
  onTab: (tab: EnvironmentTab) => void;
}

export function EnvironmentScreen({ environmentId, initialPort, tab, onTab, wantedKey }: EnvironmentScreenProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery(environmentQuery(environmentId));
  // The canonical environment query shares one native feed with all visible workspace panes.
  const back = useResourceHeading('environments', Boxes, query.data?.name ?? t('common.loading'), query.data?.id);
  const acts = useEnvironmentActs({ onTab: (_environment, next) => onTab(next), afterDelete: () => void navigate({ to: '/resources/environments' }) });

  if (query.isPending) return <Skeletons />;
  if (query.isError) return <ErrorState className="h-full" title={t('environments.readFailed')} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const environment = query.data;
  const manage = holds(environment.access, 'environment.manage');
  const viewPorts = holds(environment.access, 'ports.view');
  const displayStatus = environmentDisplayStatus(environment);
  const busy = isBusy(displayStatus);
  const { edit, primary, menu } = acts.page(environment);
  const keep = acts.keepAwake(environment);
  const sectionHref = (section: string) => `/resources/environments/${encodeURIComponent(environment.id)}?tab=${section}`;
  const toolSections = [
    { value: 'tools' as const, label: t('mcpServers.title'), panel: <McpServers environment={environment} /> },
    { value: 'terminal' as const, label: t('views.terminal.title'), panel: <EnvironmentDoor environment={environment} kind="terminal" /> },
    ...(viewPorts ? [{ value: 'ports' as const, label: t('ports.title'), panel: <PortsPanel environment={environment} initialPort={initialPort} browser={portsBrowser(environment, () => onTab('browsers'))} /> }] : []),
    ...(environment.access.permissions.includes('browser.view') ? [{ value: 'browsers' as const, label: t('browser.tab'), panel: <BrowsersPanel environment={environment} threadId={null} /> }] : []),
    ...(environment.access.permissions.includes('vnc.view') ? [{ value: 'vnc' as const, label: t('vnc.tab'), panel: <DisplaysTab environment={environment} threadId={null} /> }] : []),
  ];
  const settingsSections = [
    { value: 'settings' as const, label: t('resourceDetails.layout.configuration'), panel: <EnvironmentSettings key={wantedKey} environment={environment} wantedKey={wantedKey} /> },
    { value: 'controls' as const, label: t('environments.overview.switches'), panel: <><Switches environment={environment} manage={manage} /><DataSourcesSummary value={environment.data_sources} /></> },
    { value: 'trust' as const, label: t('resourceDetails.layout.trust'), panel: <HarnessTrustSection environment={environment} /> },
    ...(environment.machine !== null && manage ? [{ value: 'limits' as const, label: t('environments.tabs.limits'), panel: <MachineLimits compact machineId={environment.machine.id} bounds="environment" id="environment-limits" title={t('environments.limits.title')} lead={t('environments.limits.lead', { machine: environment.machine.name })} /> }] : []),
    ...(manage ? [{ value: 'danger' as const, label: t('resourceDetails.layout.lifecycle'), panel: <DangerZone environment={environment} onEnable={() => acts.enable(environment)} onDisable={() => acts.ask('disable', environment)} /> }] : []),
  ];
  const tool = toolSections.find((section) => section.value === tab)?.value ?? 'tools';
  const settings = settingsSections.find((section) => section.value === tab)?.value ?? 'settings';
  const grouped = toolSections.some((section) => section.value === tab) ? 'tools' : settingsSections.some((section) => section.value === tab) ? 'settings' : tab;

  // Their order is the order of the screen's own parts: what it is, who is in it, what bounds it, who may reach it, what it is set to, and last what ends it.
  const tabs = [
    { value: 'overview' as const, label: t('environments.tabs.overview'), panel: <Overview environment={environment} /> },
    { value: 'threads' as const, label: t('environments.tabs.threads'), count: environment.threads.length + environment.threads_hidden, panel: <Threads environment={environment} manage={manage} /> },
    { value: 'tools' as const, label: t('resourceDetails.layout.tools'), panel: <DetailSections label={t('resourceDetails.layout.tools')} sections={toolSections} value={tool} onValueChange={onTab} hrefOf={sectionHref} className="h-full min-h-0" panelClassName="h-full min-h-0" /> },
    { value: 'files' as const, label: t('views.files.title'), panel: <EnvironmentDoor environment={environment} kind="files" /> },
    { value: 'access' as const, label: t('environments.tabs.access'), panel: <ResourceAccess access={environment.access} target={{ kind: 'environment', id: environment.id, name: environment.name }} /> },
    { value: 'settings' as const, label: t('environments.tabs.settings'), panel: <DetailSections label={t('environments.tabs.settings')} sections={settingsSections} value={settings} onValueChange={onTab} hrefOf={sectionHref} /> },
  ];
  const shown = tabs.some((candidate) => candidate.value === grouped) ? grouped : 'overview';
  const directDoor = tab === 'files' || tab === 'terminal' || tab === 'browsers' || tab === 'vnc';
  const Frame = directDoor ? DoorPage : Page;

  return (
    <Frame>
      <PageHeader
        adminAccess={environment.access.admin_access} ownership={environment} opening={{ kind: 'environment', id: environment.id }}
        back={back}
        mark={
          <PlaceMark>
            <Boxes aria-hidden />
          </PlaceMark>
        }
        title={environment.name}
        facts={[
          ...(!busy ? [<EnvironmentStatusText key="status" environment={environment} />] : []),
          // What it runs on and under, with what that shares with the host as its hint.
          ...(namesRuntime(environment.isolation) ? [<RuntimePair key="runtime" isolation={environment.isolation} />] : []),
          <PortsExposure key="ports" environment={environment} />,
          ...(shown !== 'overview' && environmentChangesVisible(environment) && environment.updates.length > 0 ? [<Badge key="update" tone="warning">{t('environments.list.update')}</Badge>] : []),
        ]}
        actions={
          <HeadingActs
            admin={adminSpec(environment, 'environment.manage')}
            name={environment.name}
            primary={primary}
            edit={edit}
            menu={menu}
            omit={settings === 'danger' && shown === 'settings' ? ['enable', 'disable'] : []}
            before={
              keep === null ? undefined : (
                <Button icon={<Hourglass aria-hidden />} pending={keep.pending === true} onClick={keep.run}>
                  {keep.label}
                </Button>
              )
            }
          />
        }
      />
      <TemporaryStatus environment={environment} />
      {busy && (
        <InlineAlert
          tone="info"
          title={t(`environments.status.${displayStatus}`)}
          {...(environment.job === null
            ? {}
            : {
                action: (
                  <Button size="sm" onClick={() => openChange({ kind: 'environment', id: environment.id, name: environment.name }, { kind: 'progress', job: environment.job?.id ?? '' })}>
                    {t('change.banner.progress')}
                  </Button>
                ),
              })}
        />
      )}
      {shown !== 'settings' && <UnitCommandFailure jobId={environment.job?.id} detail={environment.status_detail} href={`/resources/environments/${environment.id}?tab=settings&setting=env.after_up#unit-after-up`} />}
      {tab === 'ports' && !viewPorts ? <PortsPanel environment={environment} initialPort={initialPort} browser={portsBrowser(environment, () => onTab('browsers'))} /> : <Tabs label={t('environments.tabs.label')} value={shown} onValueChange={onTab} tabs={tabs} className={directDoor ? 'flex min-h-0 flex-1 flex-col' : undefined} panelClassName={directDoor ? 'min-h-0 flex-1 overflow-hidden pt-3' : 'pt-6'} />}
      {acts.dialogs}
    </Frame>
  );
}
