/** A host: owner, capacity and storages, what its units declare they need of it, what it does when full, its load over time from stored samples, what runs on it from both pools, its page with the limits each entry carries, and enrollment.
 *
 * What is built: whose it is and which machine it is, what it has and how much of that the environments there
 * declare they need, its storages and drivers, what runs on it from both pools, whether it takes new
 * environments and what it does when it is full, and who may use or manage it. Its load over time from
 * stored samples is not drawn: that waits for the shared time-series chart.
 *
 * Fullness is counted from what environments declare they need, which reserves nothing and can pass the
 * whole; the page says so wherever it shows a figure of it. A host whose machine is away is shown as such.
 * Its units are read only by those who manage it: one is stopped, or removed after naming it.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { Plus, Server } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { applyRow } from '@/api/cache';

import { ApiError } from '@/api/client';
import { hostKey, hostQuery, updateHost } from '@/api/resources';
import { HostPortsDeclaration } from '@/features/ports/HostDeclaration';
import { PortFault } from '@/features/ports/Fault';
import { DetailSections, ResourceDetails, ResourceTabs as Tabs, ResourceHeadingActs as HeadingActs } from '@/features/resources/DetailSections';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { UpdateNotice } from '@/features/resources/ComponentUpdate';
import { adminSpec } from '@/ui/AdminAct';
import { useAmountWords } from '@/features/resources/amounts';
import { Facts } from '@/features/resources/Facts';
import { useHostActs } from '@/features/resources/hosts/acts';
import { HostRuntimes } from '@/features/resources/hosts/Runtimes';
import { KeptLayers } from '@/features/resources/hosts/KeptLayers';
import { DriverFeatures } from '@/features/resources/hosts/DriverFeatures';
import { MountsHere } from '@/features/resources/mounts/Here';
import { MachinePolicyTab } from '@/features/resources/machines/Policy';
import { EnvironmentDirectoriesHere } from '@/features/resources/environments/DataSources';
import { Hint } from '@/features/help/Hint';
import { useHelpOpener } from '@/features/help/open';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { SettingsEditor } from '@/features/settings/Editor';
import { SettingChanges } from '@/ui/SettingChanges';
import { HostKept } from '@/features/trace/HostKept';
import { declaredShare, usedShare } from '@/features/resources/hosts/words';
import { ResourceAccess } from '@/features/resources/ResourceAccess';
import { useResourceHeading } from '@/features/resources/trail';
import type { Host, HostDriver, HostStorage, UnitKind, WhenFull } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Page, PageHeader, PlaceMark, Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { HelpHint } from '@/ui/HelpHint';
import { Meter } from '@/ui/Meter';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { SettingRow } from '@/ui/SettingRow';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { type Column, Table } from '@/ui/Table';
import { linkClasses } from '@/ui/link';

import { HOST_TABS, type HostTab } from '@/features/resources/tabs';

export { HOST_TABS, type HostTab };

/** The sentinel of the "when it is full" choice that follows the system again: a radio group cannot hold nothing. */
const FOLLOW_SYSTEM = 'system';

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

function Capacity({ host }: { host: Host }) {
  const { t } = useTranslation();
  const words = useAmountWords();
  const rows = [
    { id: 'cpu', label: t('hosts.capacity.cpu'), declared: host.declared.cpu_millis, capacity: host.capacity.cpu_millis, say: words.cpu },
    { id: 'memory', label: t('hosts.capacity.memory'), declared: host.declared.memory_mb, capacity: host.capacity.memory_mb, say: words.memory },
    { id: 'disk', label: t('hosts.capacity.disk'), declared: host.declared.disk_gb, capacity: host.capacity.disk_gb, say: words.disk },
  ];
  return (
    <Section id="host-capacity" title={t('hosts.capacity.title')} lead={t('hosts.capacity.lead')}>
      <ul aria-label={t('hosts.capacity.title')} className="grid gap-4 sm:grid-cols-3">
        {rows.map((row) => {
          const share = declaredShare(row.declared, row.capacity);
          return (
            <li key={row.id} className="flex flex-col gap-2 rounded-lg border border-border bg-surface p-3">
              {share === null ? (
                <>
                  <span className="text-sm text-text">{row.label}</span>
                  <span className="text-sm text-text-muted">{t('hosts.capacity.unknown')}</span>
                </>
              ) : (
                <Meter
                  value={share}
                  label={row.label}
                  detail={t('hosts.capacity.declared', { declared: row.say(row.declared), capacity: row.say(row.capacity) })}
                />
              )}
              {share !== null && share > 100 && <span className="text-sm text-warning-text">{t('hosts.capacity.over')}</span>}
            </li>
          );
        })}
      </ul>
    </Section>
  );
}

function Storages({ host }: { host: Host }) {
  const { t } = useTranslation();
  const words = useAmountWords();
  if (host.storages.length === 0) return null;
  const columns: Column<HostStorage>[] = [
    {
      id: 'name',
      header: t('hosts.storages.name'),
      cell: (storage) => (
        <span className="flex flex-wrap items-center gap-2">
          <span className="font-medium text-text">{storage.name}</span>
          {storage.default && <Badge tone="accent">{t('hosts.storages.default')}</Badge>}
        </span>
      ),
    },
    {
      id: 'used',
      header: t('hosts.storages.used'),
      cell: (storage) => {
        const share = usedShare(storage);
        return share === null ? '–' : <Meter look="compact" value={share} label={t('hosts.storages.usedOf', { name: storage.name })} />;
      },
    },
    { id: 'free', header: t('hosts.storages.free'), from: 'sm', className: 'text-text-muted', cell: (storage) => storage.capacity_gb > 0 ? words.disk(storage.free_gb) : '–' },
    { id: 'size', header: t('hosts.storages.size'), from: 'sm', className: 'text-text-muted', cell: (storage) => storage.capacity_gb > 0 ? words.disk(storage.capacity_gb) : '–' },
    {
      id: 'views',
      header: t('hosts.storages.views'),
      from: 'md',
      className: 'text-text-muted',
      cell: (storage) => (storage.private_views ? t('hosts.storages.viewsYes') : t('hosts.storages.viewsNo')),
    },
  ];
  return (
    <Section id="host-storages" title={t('hosts.storages.title')} lead={t('hosts.storages.lead')}>
      <Table label={t('hosts.storages.title')} columns={columns} rows={host.storages} rowKey={(storage) => storage.name} />
    </Section>
  );
}

function Drivers({ host }: { host: Host }) {
  const { t } = useTranslation();
  const openHelp = useHelpOpener();
  if (host.drivers.length === 0) return null;
  const creates = (kinds: readonly UnitKind[]) => kinds.map((kind) => t(`templates.kinds.${kind}`)).join(', ');
  const columns: Column<HostDriver>[] = [
    {
      id: 'name',
      header: t('hosts.drivers.name'),
      cell: (driver) => (
        <span className="flex min-w-0 flex-col">
          <span className="font-medium text-text">{driver.title}</span>
          <code className="text-xs text-text-subtle">{driver.name}</code>
        </span>
      ),
    },
    { id: 'creates', header: t('hosts.drivers.creates'), from: 'sm', className: 'text-text-muted', cell: (driver) => creates(driver.creates) },
    { id: 'features', header: t('hosts.driverFeatures.title'), cell: (driver) => <DriverFeatures driver={driver} /> },
    { id: 'classes', header: t('hosts.drivers.classes'), from: 'md', className: 'text-text-muted', cell: (driver) => (driver.runtime_classes.length === 0 ? '–' : driver.runtime_classes.join(', ')) },
  ];
  return (
    <Section id="host-drivers" title={t('hosts.drivers.title')} lead={t('hosts.drivers.lead')}
      action={<HelpHint term={t('hosts.driverFeatures.title')} what={t('hosts.driverFeatures.about')}
        guide={{ article: 'host-driver-features', heading: null }} more={{ label: t('help.hint.more'), onOpen: () => openHelp('host-driver-features') }} />}>
      <Table label={t('hosts.drivers.title')} columns={columns} rows={host.drivers} rowKey={(driver) => driver.name} />
    </Section>
  );
}

function Overview({ host }: { host: Host }) {
  const { t } = useTranslation();
  const words = useAmountWords();
  return (
    <div className="flex flex-col gap-6">
      <Section id="host-about" title={t('hosts.overview.about')}>
        <Facts compact
          label={t('hosts.overview.about')}
          rows={[
            {
              name: t('hosts.overview.machine'),
              value: (
                <span className="flex flex-wrap items-center gap-2">
                  <Link to="/resources/machines/$id" params={{ id: host.machine.id }} className={linkClasses}>
                    {host.machine.name}
                  </Link>
                </span>
              ),
            },
            { name: t('hosts.overview.home'), value: host.home?.name ?? t('hosts.list.system') },
            ...(host.labels.length === 0 ? [] : [{ name: t('hosts.overview.labels'), value: host.labels.join(', ') }]),
            {
              name: t('hosts.overview.running'),
              value: (
                <span>
                  {t('hosts.list.environments', { count: host.running.environment })}
                  {host.running.provider > 0 && `${t('startFlow.separator')}${t('hosts.list.runtimes', { count: host.running.provider })}`}
                </span>
              ),
            },
            { name: t('hosts.overview.mine'), value: t('hosts.list.environments', { count: host.mine }) },
            { name: t('hosts.overview.whenFull'), value: <WhenFullWords host={host} /> },
            { name: t('hosts.capacity.cpu'), value: t('hosts.capacity.declared', { declared: words.cpu(host.declared.cpu_millis), capacity: host.capacity.cpu_millis > 0 ? words.cpu(host.capacity.cpu_millis) : '–' }) },
            { name: t('hosts.capacity.memory'), value: t('hosts.capacity.declared', { declared: words.memory(host.declared.memory_mb), capacity: host.capacity.memory_mb > 0 ? words.memory(host.capacity.memory_mb) : '–' }) },
            { name: t('hosts.capacity.disk'), value: t('hosts.capacity.declared', { declared: words.disk(host.declared.disk_gb), capacity: host.capacity.disk_gb > 0 ? words.disk(host.capacity.disk_gb) : '–' }) },
          ]}
        />
      </Section>
      <ResourceDetails><Facts compact rows={[{ name: t('hosts.overview.address'), value: host.address }]} /><Capacity host={host} /><Drivers host={host} /></ResourceDetails>
    </div>
  );
}

/** What a host does when it is full, and whose answer that is. */
function WhenFullWords({ host }: { host: Host }) {
  const { t } = useTranslation();
  return (
    <span className="flex flex-wrap items-center gap-2">
      <span>{t(`hosts.whenFull.${host.when_full.value}`)}</span>
      <Badge>{host.when_full.own ? t('hosts.whenFull.own') : t('hosts.whenFull.system')}</Badge>
    </span>
  );
}

/** The host's own settings: whether it takes new environments and what it does when it is full, each saved the moment it is chosen, and its name and labels, saved together. */
interface HostPatch { name?: string; labels?: string[]; schedulable?: boolean; when_full?: WhenFull | null }
function HostSettings({ host, manage }: { host: Host; manage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [name, setName] = useState(host.name);
  const [labels, setLabels] = useState(host.labels.join(', '));
  const [base, setBase] = useState(host);
  const [observed, setObserved] = useState(host.revision);
  const [undo, setUndo] = useState<{ patch: HostPatch; revision: number } | null>(null);
  const wanted = labels
    .split(',')
    .map((label) => label.trim())
    .filter((label) => label !== '');
  const labelsChanged = wanted.join('\n') !== base.labels.join('\n');
  const changed = name.trim() !== base.name || labelsChanged;
  if (host.revision !== observed) {
    setObserved(host.revision);
    if (undo !== null && host.revision > undo.revision) setUndo(null);
    if (!changed) { setBase(host); setName(host.name); setLabels(host.labels.join(', ')); }
  }

  const save = useMutation({
    mutationFn: (patch: HostPatch) => updateHost(host.id, { expected_revision: patch.name !== undefined || patch.labels !== undefined ? base.revision : host.revision, ...patch }),
    onMutate: (patch) => ({ previous: {
      ...(patch.name === undefined ? {} : { name: host.name }), ...(patch.labels === undefined ? {} : { labels: host.labels }),
      ...(patch.schedulable === undefined ? {} : { schedulable: host.schedulable }),
      ...(patch.when_full === undefined ? {} : { when_full: host.when_full.own ? host.when_full.value : null }),
    } }),
    onSuccess: (updated, patch, context) => {
      const latest = queryClient.getQueryData<Host>(hostKey(updated.id));
      if (latest !== undefined && latest.revision > updated.revision) { setUndo(null); return; }
      queryClient.setQueryData(hostKey(updated.id), updated);
      setUndo({ patch: context.previous, revision: updated.revision });
      if (patch.name !== undefined || patch.labels !== undefined) { setBase(updated); setName(updated.name); setLabels(updated.labels.join(', ')); }
      else if (updated.name === base.name && updated.labels.join('\n') === base.labels.join('\n')) setBase(updated);
    },
    // Somebody else changed it first: it is read again before the next try.
    onError: (error) => {
      if (error instanceof ApiError && error.is('conflict.stale_revision')) void queryClient.invalidateQueries({ queryKey: hostKey(host.id) });
    },
  });
  const revert = useMutation({ mutationFn: (entry: { patch: HostPatch; revision: number }) => updateHost(host.id, { expected_revision: entry.revision, ...entry.patch }),
    onSuccess: (updated) => {
      const latest = queryClient.getQueryData<Host>(hostKey(updated.id)); setUndo(null);
      if (latest !== undefined && latest.revision > updated.revision) return;
      queryClient.setQueryData(hostKey(updated.id), updated); setBase(updated); setName(updated.name); setLabels(updated.labels.join(', '));
    },
  });
  const busy = save.isPending || revert.isPending;
  const stale = save.error instanceof ApiError && save.error.is('conflict.stale_revision') || changed && host.revision > base.revision;
  const saving = (field: 'schedulable' | 'when_full' | 'names'): boolean =>
    save.isPending && (field === 'names' ? save.variables.schedulable === undefined && save.variables.when_full === undefined : save.variables[field] !== undefined);
  // What was chosen is shown as chosen until the server has answered.
  const whenFull: WhenFull | typeof FOLLOW_SYSTEM = saving('when_full') ? (save.variables?.when_full ?? FOLLOW_SYSTEM) : host.when_full.own ? host.when_full.value : FOLLOW_SYSTEM;

  return (
    <div className="flex flex-col gap-6">
      {!manage && <ReadOnlySettingsNotice owner={host.owner} />}
      <ul className="min-w-0 divide-y divide-border rounded-lg border border-border" aria-label={t('hosts.tabs.settings')}>
        <SettingRow name={t('hosts.settings.takeTitle')} readOnly={!manage} description={t('hosts.settings.takeLead')} help={<Hint id="host-schedulable" />}
          value={manage ? <Switch label={<span className="sr-only">{t('hosts.settings.takeTitle')}</span>} disabled={busy} checked={saving('schedulable') ? save.variables?.schedulable ?? host.schedulable : host.schedulable}
            onCheckedChange={(schedulable) => !busy && save.mutate({ schedulable })} /> : t(host.schedulable ? 'hosts.list.open' : 'hosts.list.closed')} />
        <SettingRow name={t('hosts.settings.fullTitle')} readOnly={!manage} description={t('hosts.settings.fullLead')} help={<Hint id="host-when-full" />}
          reason={host.when_full.own ? t('hosts.whenFull.own') : manage ? t('hosts.settings.fullFollows', { value: t(`hosts.whenFull.${host.when_full.value}`) }) : t('hosts.whenFull.system')} value={manage ? <Select<WhenFull | typeof FOLLOW_SYSTEM>
            label={t('hosts.settings.fullTitle')}
            hideLabel
            disabled={busy}
            value={whenFull}
            // The choice stays where it is while its answer comes, so that the keyboard stays on it; one more before then is let go.
            onValueChange={(next) => !busy && save.mutate({ when_full: next === FOLLOW_SYSTEM ? null : next })}
            options={[
              { value: 'warn', label: t('hosts.whenFull.warn') },
              { value: 'block', label: t('hosts.whenFull.block') },
              { value: FOLLOW_SYSTEM, label: t('hosts.whenFull.system') },
            ]}
          /> : t(`hosts.whenFull.${host.when_full.value}`)} />
        {manage && <SettingRow name={t('hosts.settings.name')} value={<Input label={t('hosts.settings.name')} hideLabel value={name} disabled={busy} onChange={(event) => { setName(event.target.value); setUndo(null); save.reset(); }} autoComplete="off" />} />}
        <SettingRow name={t('hosts.settings.labels')} help={<Hint id="host-labels" />} description={t('hosts.settings.labelsHint')}
          readOnly={!manage} value={manage ? <Input label={t('hosts.settings.labels')} hideLabel value={labels} disabled={busy} onChange={(event) => { setLabels(event.target.value); setUndo(null); save.reset(); }} autoComplete="off" /> : host.labels.join(', ')} />
      </ul>
          {manage && <div>
            <Button
              variant="primary"
              pending={saving('names')}
              blocked={busy || !changed || stale || name.trim() === ''}
              onClick={() => save.mutate({ ...(name.trim() === base.name ? {} : { name: name.trim() }), ...(labelsChanged ? { labels: wanted } : {}) })}
            >
              {t('hosts.settings.save')}
            </Button>
          </div>}
      {manage && undo !== null && !busy && !changed && save.error === null && revert.error === null && <div role="status" className="flex items-center gap-2 text-sm text-text-muted"><span>{t('resourceDetails.layout.saved')}</span><Button size="sm" onClick={() => revert.mutate(undo)}>{t('settings.compact.undo')}</Button></div>}
      {revert.error !== null && <ErrorAlert error={revert.error} />}
      {stale && (
        <InlineAlert tone="warning" title={t('hosts.settings.changed')}>
          {t('hosts.settings.changedLead', { revision: host.revision })}
          <SettingChanges rows={[{ name: t('hosts.settings.name'), current: host.name, draft: name }, { name: t('hosts.settings.labels'), current: host.labels, draft: wanted }]}
            disabled={busy || host.revision <= base.revision} onKeep={() => { setBase(host); save.reset(); }} />
        </InlineAlert>
      )}
      {save.error !== null && !stale && <ErrorAlert error={save.error} />}
      <SettingsEditor target={{ kind: 'host', id: host.id }} scope="host" compact only={['cleanup.unused_keep_days']} place={{ name: host.name, owner: host.owner, access: host.access, kind: 'host' }} />
    </div>
  );
}

export interface HostScreenProps {
  hostId: string;
  tab: HostTab;
  onTab: (tab: HostTab) => void;
}

export function HostScreen({ hostId, tab, onTab }: HostScreenProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const query = useQuery({ ...hostQuery(hostId), throwOnError: false });
  const cache = useQueryClient();
  const joined = useRef<string | null>(null);
  useEffect(() => {
    if (!query.isSuccess || !query.isFetchedAfterMount || query.isFetching) return;
    const signature = JSON.stringify(query.data);
    if (joined.current === signature) return;
    joined.current = signature;
    // Join an admitted observation after the native reader accepts it, never from queryFn.
    // A later visit's capacity/declarations must not leave an older loaded inventory behind.
    applyRow(cache, { kind: 'host', id: query.data.id, value: { ...query.data } });
  }, [cache, query.data, query.isFetchedAfterMount, query.isFetching, query.isSuccess]);
  const [making, setMaking] = useState(false);
  const back = useResourceHeading('hosts', Server, query.data?.name ?? t(query.isPending ? 'common.loading' : 'access.area.host'), query.data?.id);
  const acts = useHostActs({ onSettings: () => onTab('settings'), afterDelete: () => void navigate({ to: '/resources/hosts' }) });

  if (query.isPending) return <Skeletons />;
  if (query.isError) return query.error instanceof ApiError && query.error.code.startsWith('ports.')
    ? <div className="p-6"><PortFault error={query.error} onRefresh={() => void query.refetch()} /></div>
    : <ErrorState className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const host = query.data;
  const manage = holds(host.access, 'host.manage');
  const { edit, primary, menu } = acts.page(host);
  const sectionHref = (section: string) => `/resources/hosts/${encodeURIComponent(host.id)}?tab=${section}`;
  const settings = tab === 'policy' || tab === 'ports' ? tab : 'settings';

  const tabs = [
    { value: 'overview' as const, label: t('hosts.tabs.overview'), panel: <Overview host={host} /> },
    ...(manage
      ? [
          { value: 'runtimes' as const, label: t('hosts.tabs.runtimes'), count: host.running.environment + host.running.provider, panel: <HostRuntimes host={host} /> },
        ]
      : []),
    { value: 'storage' as const, label: t('resourceDetails.layout.storage'), panel: <DetailSections label={t('resourceDetails.layout.storage')} sections={[
      { value: 'storages', label: t('hosts.storages.title'), panel: <Storages host={host} /> },
      { value: 'mounts', label: t('resourceDetails.mount.here.title'), panel: <MountsHere machine={host.machine.id} name={host.name} /> },
      { value: 'directories', label: t('environmentData.roots'), panel: <EnvironmentDirectoriesHere machine={host.machine.id} /> },
      ...(manage ? [
        { value: 'kept', label: t('trace.keptTitle'), panel: <HostKept host={host} /> },
        { value: 'layers', label: t('keptLayers.title'), panel: <KeptLayers host={host} /> },
      ] : []),
    ]} /> },
    { value: 'settings' as const, label: t('hosts.tabs.settings'), panel: <DetailSections label={t('hosts.tabs.settings')} value={settings} onValueChange={onTab} hrefOf={sectionHref} sections={[
      { value: 'settings' as const, label: t('resourceDetails.layout.configuration'), panel: <HostSettings host={host} manage={manage} /> },
      { value: 'policy' as const, label: t('machines.tabs.policy'), panel: <MachinePolicyTab machine={host.machine} /> },
      { value: 'ports' as const, label: t('ports.hostForm.title'), panel: <HostPortsDeclaration hostId={host.id} /> },
    ]} /> },
    { value: 'access' as const, label: t('hosts.tabs.access'), panel: <ResourceAccess access={host.access} target={{ kind: 'host', id: host.id, name: host.name }} /> },
  ];
  const grouped = tab === 'policy' || tab === 'ports' ? 'settings' : tab;
  const shown = tabs.some((candidate) => candidate.value === grouped) ? grouped : 'overview';

  return (
    <Page>
      <PageHeader
        adminAccess={host.access.admin_access} ownership={host} opening={{ kind: 'host', id: host.id }}
        back={back}
        mark={
          <PlaceMark>
            <Server aria-hidden />
          </PlaceMark>
        }
        title={host.name}
        facts={[
          <Badge key="state" tone={host.schedulable ? 'accent' : 'warning'}>
            {host.schedulable ? t('hosts.list.open') : t('hosts.list.closed')}
          </Badge>,
          <StatusDot key="machine" tone={host.lost_at !== undefined ? 'warning' : host.machine.online ? 'success' : 'idle'} label={host.lost_at !== undefined ? t('loss.status') : host.machine.online ? t('machines.state.online') : t('machines.state.offline')} showLabel />,
        ]}
        actions={<>{holds(host.access, 'host.use') && <Button icon={<Plus aria-hidden />} onClick={() => setMaking(true)}>{t('environments.make.here')}</Button>}<HeadingActs admin={adminSpec(host, 'host.manage')} name={host.name} primary={primary?.id === 'update-now' ? null : primary} edit={edit} menu={menu}
          omit={[...(shown === 'settings' && settings === 'settings' ? ['pause', 'resume', 'edit', 'rename'] : []), ...(primary?.id === 'update-now' ? ['update-now'] : [])]} /></>}
      />
      <UpdateNotice update={host.software_update} action={primary?.id === 'update-now' ? primary : null} />
      <Tabs label={t('hosts.tabs.label')} value={shown} onValueChange={onTab} tabs={tabs} panelClassName="pt-4" />
      {acts.dialogs}
      {making && <MakeEnvironmentDialog hostId={host.id} onClose={() => setMaking(false)} />}
    </Page>
  );
}
