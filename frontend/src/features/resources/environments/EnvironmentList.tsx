/** The environments a person may use or manage: where each comes from, how it runs, how it stands, the threads in it, whether it is behind, and what the reader may do there; with the means to register a directory on a machine.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Boxes, Cpu, Laptop, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { resourceReaders } from '@/features/resources/inventory';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { resourceLabels } from '@/features/resources/labels';
import { PortsExposure } from '@/features/ports/Exposure';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { machinesQuery } from '@/api/resources';
import { useEnvironmentActs } from '@/features/resources/environments/acts';
import { BUSY_POLL_MS, environmentChangesVisible, environmentDisplayStatus, environmentRemovalPhase, holding, howItRuns, isBusy, threadCounts } from '@/features/resources/environments/words';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { TemporaryStatus } from '@/features/resources/environments/Temporary';
import { RegisterEnvironmentDialog } from '@/features/resources/machines/RegisterEnvironment';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import { oneOf } from '@/features/resources/search';
import type { Environment, EnvironmentStatus, Machine } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { UpdateMark } from '@/features/resources/ComponentUpdate';
import { Button } from '@/ui/Button';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { Menu } from '@/ui/Menu';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import type { Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { StatusDot } from '@/ui/StatusDot';
import { RelativeTime } from '@/ui/RelativeTime';
import { Tooltip } from '@/ui/Tooltip';
import { RuntimeChange } from '@/features/resources/TechnicalValue';

export type EnvironmentKind = 'machine' | 'template';

export const ENVIRONMENT_KINDS: readonly EnvironmentKind[] = ['machine', 'template'];
export const ENVIRONMENT_STATUSES: readonly EnvironmentStatus[] = ['running', 'provisioning', 'recreating', 'moving', 'stopped', 'none', 'offline', 'failed', 'disabled', 'lost'];

export interface EnvironmentsSearch {
  q?: string;
  kind?: EnvironmentKind;
  status?: EnvironmentStatus;
}

export function EnvironmentLink({ environment, label = environment.name }: { environment: Pick<Environment, 'id' | 'name'> & Partial<Environment>; label?: string }) {
  return (
    <span className="flex min-w-0 flex-col gap-0.5"><ResourceName subject={{ kind: 'environment', environment }} asChild><Link to="/resources/environments/$id" params={{ id: environment.id }} className="truncate font-medium text-text hover:underline">
      {label}
    </Link></ResourceName><ResourceHint subject={{ kind: 'environment', environment }} /></span>
  );
}

export function EnvironmentStatusText({ environment }: { environment: Pick<Environment, 'status'> & Partial<Pick<Environment, 'removal' | 'remove_at' | 'job'>> }) {
  const { t } = useTranslation();
  const phase = environmentRemovalPhase(environment);
  if (phase !== undefined) {
    const state = <StatusDot tone={phase === 'restoring' ? 'info' : phase === 'kept' || phase === 'purged' ? 'idle' : 'warning'} label={t(`environments.lifecycle.${phase}`)} showLabel />;
    return phase === 'scheduled' && environment.remove_at ? <Tooltip content={<RelativeTime at={environment.remove_at} />}><span tabIndex={0} className="inline-flex rounded-sm">{state}</span></Tooltip> : state;
  }
  return <EnvironmentDot status={environmentDisplayStatus(environment)} showLabel />;
}

export function EnvironmentList({ search, onSearch }: { search: EnvironmentsSearch; onSearch: (patch: Partial<EnvironmentsSearch>) => void }) {
  const { t } = useTranslation();
  const kindLabel = t(`environments.list.showingKinds.${search.kind ?? 'all'}`);
  const places = useKnownPlaces({ trees: false });
  const [addingOpen, setAddingOpen] = useState(false);
  const machines = useQuery({ ...machinesQuery, enabled: addingOpen });
  const [registering, setRegistering] = useState<Machine | null>(null);
  const [making, setMaking] = useState(false);
  const acts = useEnvironmentActs();
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));

  const columns = (rows: readonly Environment[]): Column<Environment>[] => {
    const labels = resourceLabels(rows, places);
    return [
    {
      id: 'name',
      header: t('environments.list.name'),
      cell: (environment, shown) => {
        // What a column that is left out of a narrow place would have said is said under the name instead.
        const withoutRuns = !shown.has('runs');
        const withoutStatus = !shown.has('status');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <span className="flex flex-wrap items-center gap-2"><EnvironmentLink environment={environment} label={labels.get(environment.id) ?? environment.name} /><PortsExposure environment={environment} /></span>
            <TemporaryStatus environment={environment} />
            {environment.updates.filter((update) => update.what === 'harness').map((update) => <span key={update.name} className="text-sm wrap-anywhere text-text-muted"><RuntimeChange change={update} /></span>)}
            {withoutRuns && (
              <span title={howItRuns(environment, t)} className="text-sm wrap-anywhere text-text-muted">
                {howItRuns(environment, t, true)}
              </span>
            )}
            {withoutStatus && <EnvironmentStatusText environment={environment} />}
            {withoutStatus && environmentChangesVisible(environment) && <UpdateMark update={environment.connector_update} />}
          </div>
        );
      },
    },
    { id: 'owner', header: t('environments.list.owner'), from: 'lg', cell: (environment) => <UserChip user={environment.owner} size={24} thing={{ kind: 'environment', id: environment.id }} /> },
    {
      id: 'runs',
      header: t('environments.list.runs'),
      from: 'md',
      cell: (environment) => (
        <span title={howItRuns(environment, t)} className="wrap-anywhere text-text-muted">
          {howItRuns(environment, t, true)}
        </span>
      ),
    },
    {
      id: 'status',
      header: t('environments.list.status'),
      from: 'sm',
      cell: (environment) => (
        <span className="flex flex-col gap-1">
          <EnvironmentStatusText environment={environment} />
          {environmentChangesVisible(environment) && <UpdateMark update={environment.connector_update} />}
          {environmentChangesVisible(environment) && (environment.updates.length > 0 || environment.update_required) && !environment.connector_update?.behind && (
            <Badge tone="warning" className="self-start">
              {environment.update_required ? t('environments.list.updateRequired') : t('environments.list.update')}
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'threads',
      header: t('environments.list.threads'),
      from: 'lg',
      cell: (environment) => {
        const counts = threadCounts(environment);
        if (counts.threads === 0 && counts.hidden === 0) return <span className="text-text-subtle">{t('environments.list.noThreads')}</span>;
        return (
          <span className="flex flex-wrap gap-x-3 gap-y-1 text-text-muted">
            <span>{t('environments.list.threadCount', { count: counts.threads })}</span>
            {counts.topics > 0 && <span>{t('environments.list.topicCount', { count: counts.topics })}</span>}
            {counts.hidden > 0 && <span>{t('environments.list.hiddenCount', { count: counts.hidden })}</span>}
          </span>
        );
      },
    },
    {
      id: 'may',
      header: t('environments.list.may'),
      from: 'xl',
      cell: (environment) => <Badge>{t(`environments.holding.${holding(environment.access.permissions)}`)}</Badge>,
    },
    ];
  };

  const usable = (machines.data ?? []).filter((machine) => machine.status === 'active');
  const add = (
    <Menu
      label={t('environments.list.add')}
      align="end"
      onOpenChange={setAddingOpen}
      entries={[
        { id: 'template', label: t('environments.list.makeFromTemplate'), icon: <Cpu aria-hidden />, description: t('environments.list.makeFromTemplateWhy'), onSelect: () => setMaking(true) },
        { type: 'separator', id: 'apart' },
        ...(machines.isPending
          ? [{ type: 'label' as const, id: 'loading', label: t('resources.inventoryLoading') }]
          : usable.length === 0
          ? [{ id: 'none', label: t('environments.list.noMachine'), disabled: t('environments.list.noMachineWhy'), onSelect: () => undefined }]
          : [
              { type: 'label' as const, id: 'on', label: t('environments.list.registerOn') },
              ...usable.map((machine) => ({
                id: machine.id,
                label: machine.name,
                icon: <Laptop aria-hidden />,
                description: machine.online ? t('machines.state.online') : t('machines.state.offline'),
                onSelect: () => setRegistering(machine),
              })),
            ]),
      ]}
      trigger={
        <Button variant="primary" icon={<Plus aria-hidden />}>
          {t('environments.list.add')}
        </Button>
      }
    />
  );

  return (
    <>
      <ResourceList
        kind="environments"
        search={{ ...search }}
        acts={acts.list}
        title={t('screens.environments.title')}
        icon={Boxes}
        lead={t('environments.list.lead')}
        label={t('environments.list.label')}
        readRow={resourceReaders.environments}
        filter={{ state: search.status, kind: search.kind }}
        poll={{ interval: BUSY_POLL_MS, while: (environment) => isBusy(environment.status) }}
        columns={columns}
        rowKey={(environment) => environment.id}
        rowClassName={(environment) => (environment.status === 'disabled' ? 'bg-bg-subtle' : undefined)}
        text={text}
        onText={setText}
        searchLabel={t('environments.list.search')}
        searchPlaceholder={t('environments.list.searchPlaceholder')}
        filters={
          <>
            <div className="flex flex-col items-start gap-1">
            <span className="text-sm font-medium text-text">{kindLabel}</span>
            <SegmentedControl
              label={kindLabel}
              value={search.kind ?? 'all'}
              onValueChange={(next) => onSearch({ kind: oneOf(ENVIRONMENT_KINDS, next) })}
              segments={[
                { value: 'all', name: t('choiceCatalog.environmentFilter.all.name') },
                { value: 'machine', name: t('choiceCatalog.environmentFilter.machine.name') },
                { value: 'template', name: t('choiceCatalog.environmentFilter.template.name') },
              ]}
            />
            </div>
            <Select
              label={t('environments.list.statusFilter')}
              size="sm"
              value={search.status ?? 'any'}
              onValueChange={(next) => onSearch({ status: oneOf(ENVIRONMENT_STATUSES, next) })}
              options={[{ value: 'any', ...choiceOption(t, 'statusFilter.any') }, ...ENVIRONMENT_STATUSES.map((status) => ({ value: status, ...choiceOption(t, 'statusFilter.value', { name: t(`environments.status.${status}`) }) }))]}
              fieldClassName="w-auto"
            />
          </>
        }
        action={add}
        empty={{ icon: <Boxes aria-hidden />, title: t('environments.list.none'), description: t('environments.list.noneLead'), action: add }}
        noMatch={t('environments.list.noMatch')}
      />
      {registering !== null && <RegisterEnvironmentDialog machine={registering} open onClose={() => setRegistering(null)} />}
      {making && <MakeEnvironmentDialog onClose={() => setMaking(false)} />}
    </>
  );
}
