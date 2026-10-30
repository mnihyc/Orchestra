/** The hosts a person may use or manage, the system's among them: whose each is, which drivers it has, how much of its memory the environments there declare they need, what runs there and whether it takes new environments; with the means to enroll one.
 */
import { Link } from '@tanstack/react-router';
import { HOST_STATES, type HostState, type HostsSearch } from '@/features/resources/search';
import { Plus, Server } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceReaders } from '@/features/resources/inventory';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { resourceLabels } from '@/features/resources/labels';
import { useAmountWords } from '@/features/resources/amounts';
import { AddHostDialog } from '@/features/resources/hosts/AddHost';
import { declaredShare } from '@/features/resources/hosts/words';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import { useHostActs } from '@/features/resources/hosts/acts';
import { UpdateMark } from '@/features/resources/ComponentUpdate';
import { oneOf } from '@/features/resources/search';
import type { Host } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { Meter } from '@/ui/Meter';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { StatusDot } from '@/ui/StatusDot';
import type { Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

export { HOST_STATES, type HostState, type HostsSearch };

export function HostLink({ host, label = host.name }: { host: Pick<Host, 'id' | 'name'> & Partial<Host>; label?: string }) {
  return (
    <span className="flex min-w-0 flex-col gap-0.5"><ResourceName subject={{ kind: 'host', host }} asChild><Link to="/resources/hosts/$id" params={{ id: host.id }} className="truncate font-medium text-text hover:underline">
      {label}
    </Link></ResourceName><ResourceHint subject={{ kind: 'host', host }} /></span>
  );
}

export function HostList({ search, onSearch }: { search: HostsSearch; onSearch: (patch: Partial<HostsSearch>) => void }) {
  const { t } = useTranslation();
  const words = useAmountWords();
  const places = useKnownPlaces({ trees: false });
  const [adding, setAdding] = useState(false);
  const acts = useHostActs();
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));

  /** What runs on a host, in a few words: its environments and, where there are any, the runtimes of providers. */
  const runningOn = (host: Host): string =>
    [t('hosts.list.environments', { count: host.running.environment }),...(host.running.provider > 0 ? [t('hosts.list.runtimes', { count: host.running.provider })] : [])].join(t('startFlow.separator'));

  const columns = (rows: readonly Host[]): Column<Host>[] => {
    const labels = resourceLabels(rows, places);
    return [
    {
      id: 'name',
      header: t('hosts.list.name'),
      cell: (host, shown) => {
        // What a column that is left out of a narrow place would have said is said under the name instead.
        const withoutState = !shown.has('state');
        const withoutRunning = !shown.has('running');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <HostLink host={host} label={labels.get(host.id) ?? host.name} />
            <UpdateMark update={host.software_update} />
            {host.labels.length > 0 && <span className="text-sm wrap-anywhere text-text-subtle">{host.labels.join(t('startFlow.separator'))}</span>}
            {withoutRunning && <span className="text-sm text-text-muted">{runningOn(host)}</span>}
            {withoutState && (
              <Badge tone={host.schedulable ? 'accent' : 'warning'} className="self-start">
                {host.schedulable ? t('hosts.list.open') : t('hosts.list.closed')}
              </Badge>
            )}
          </div>
        );
      },
    },
    {
      id: 'owner',
      header: t('hosts.list.owner'),
      from: 'lg',
      cell: (host) => (host.owner === null ? <span className="text-text-muted">{t('hosts.list.system')}</span> : <UserChip user={host.owner} size={24} thing={{ kind: 'host', id: host.id }} />),
    },
    {
      id: 'machine',
      header: t('hosts.list.machine'),
      from: 'md',
      cell: (host) => <StatusDot tone={host.lost_at !== undefined ? 'warning' : host.machine.online ? 'success' : 'idle'} label={host.lost_at !== undefined ? t('loss.status') : host.machine.online ? t('machines.state.online') : t('machines.state.offline')} showLabel />,
    },
    { id: 'drivers', header: t('hosts.list.drivers'), from: 'xl', className: 'text-text-muted', cell: (host) => (host.drivers.length === 0 ? '–' : host.drivers.map((driver) => driver.title).join(', ')) },
    {
      id: 'memory',
      header: t('hosts.list.memory'),
      from: 'sm',
      cell: (host) => {
        const share = declaredShare(host.declared.memory_mb, host.capacity.memory_mb);
        return share === null ? (
          <span className="text-text-subtle">–</span>
        ) : (
          <span className="flex flex-col gap-1">
            <Meter look="compact" value={share} label={t('hosts.list.memoryOf', { declared: words.memory(host.declared.memory_mb), capacity: words.memory(host.capacity.memory_mb) })} />
            <span className="text-sm text-text-muted">{t('hosts.list.memoryShort', { declared: words.memory(host.declared.memory_mb), capacity: words.memory(host.capacity.memory_mb) })}</span>
          </span>
        );
      },
    },
    {
      id: 'running',
      header: t('hosts.list.running'),
      from: 'xl',
      cell: (host) => <span className="text-text-muted">{runningOn(host)}</span>,
    },
    {
      id: 'state',
      header: t('hosts.list.state'),
      from: 'sm',
      cell: (host) => <Badge tone={host.schedulable ? 'accent' : 'warning'}>{host.schedulable ? t('hosts.list.open') : t('hosts.list.closed')}</Badge>,
    },
    ];
  };

  const add = (
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
      {t('hosts.list.add')}
    </Button>
  );

  return (
    <>
      <ResourceList
        kind="hosts"
        search={{ ...search }}
        acts={acts.list}
        title={t('screens.hosts.title')}
        icon={Server}
        lead={t('hosts.list.lead')}
        label={t('hosts.list.label')}
        readRow={resourceReaders.hosts}
        filter={{ state: search.state }}
        columns={columns}
        rowKey={(host) => host.id}
        rowClassName={(host) => (host.schedulable ? undefined : 'bg-bg-subtle')}
        text={text}
        onText={setText}
        searchLabel={t('hosts.list.search')}
        searchPlaceholder={t('hosts.list.searchPlaceholder')}
        filters={
          <SegmentedControl
            label={t('hosts.list.stateFilter')}
            value={search.state ?? 'all'}
            onValueChange={(next) => onSearch({ state: oneOf(HOST_STATES, next) })}
            segments={[
              { value: 'all', name: t('choiceCatalog.hostAvailability.all.name') },
              { value: 'open', name: t('choiceCatalog.hostAvailability.open.name') },
              { value: 'closed', name: t('choiceCatalog.hostAvailability.closed.name') },
            ]}
          />
        }
        action={add}
        empty={{ icon: <Server aria-hidden />, title: t('hosts.list.none'), description: t('hosts.list.noneLead'), action: add }}
        noMatch={t('hosts.list.noMatch')}
      />
      {adding && <AddHostDialog onClose={() => setAdding(false)} />}
    </>
  );
}
