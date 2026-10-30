/** The machines of a person: their name, system, whether they are connected, which connector they run, when they were last seen and what they do, with the means to add one.
 */
import { Laptop, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { resourceReaders } from '@/features/resources/inventory';
import { UpdateMark } from '@/features/resources/ComponentUpdate';
import { useMachineActs } from '@/features/resources/machines/acts';
import { AddMachineDialog } from '@/features/resources/machines/Enrollment';
import { MachineRoleChips } from '@/features/resources/machines/EnrollmentRoles';
import { MachineLink } from '@/features/resources/machines/MachineLink';
export { MachineLink } from '@/features/resources/machines/MachineLink';
import { architectureName, OS_NAMES } from '@/features/resources/machines/facts';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import type { Machine } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { StatusDot } from '@/ui/StatusDot';
import type { Column } from '@/ui/Table';

export type MachineFilter = 'online' | 'offline' | 'revoked';

export interface MachinesSearch {
  q?: string;
  status?: MachineFilter;
}

/** How a machine stands: revoked for good, or connected now, or not. */
export function machineState(machine: Pick<Machine, 'status' | 'online'>): MachineFilter {
  return machine.status === 'revoked' ? 'revoked' : machine.online ? 'online' : 'offline';
}

/** What a machine's state looks like as a dot: connected is green, away is quiet, revoked is struck. */
export function MachineStatus({ machine, showLabel = true }: { machine: Pick<Machine, 'status' | 'online'> & Partial<Pick<Machine, 'lost_at'>>; showLabel?: boolean }) {
  const { t } = useTranslation();
  const state = machineState(machine);
  return <StatusDot tone={machine.lost_at !== undefined ? 'warning' : state === 'online' ? 'success' : state === 'offline' ? 'idle' : 'off'} label={machine.lost_at !== undefined ? t('loss.status') : t(`machines.state.${state}`)} showLabel={showLabel} />;
}

export function MachineList({ search, onSearch }: { search: MachinesSearch; onSearch: (patch: Partial<MachinesSearch>) => void }) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const acts = useMachineActs();
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));

  const columns: Column<Machine>[] = [
    {
      id: 'name',
      header: t('machines.list.name'),
      cell: (machine, shown) => {
        // What a column that is left out of a narrow place would have said is said under the name instead.
        const withoutPlatform = !shown.has('platform');
        const withoutState = !shown.has('state');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <MachineLink machine={machine} />
            <UpdateMark update={machine.software_update} />
            {withoutPlatform && (
              <span className="text-sm text-text-muted">
                {OS_NAMES[machine.platform_os]} ({architectureName(machine.platform_arch, t)})
              </span>
            )}
            {withoutState && <MachineStatus machine={machine} />}
          </div>
        );
      },
    },
    {
      id: 'platform',
      header: t('machines.list.platform'),
      from: 'sm',
      cell: (machine) => (
        <span className="whitespace-nowrap">
          {OS_NAMES[machine.platform_os]} ({architectureName(machine.platform_arch, t)})
        </span>
      ),
    },
    { id: 'state', header: t('machines.list.state'), from: 'sm', cell: (machine) => <MachineStatus machine={machine} /> },
    { id: 'version', header: t('machines.list.version'), from: 'xl', className: 'whitespace-nowrap text-text-muted', cell: (machine) => machine.connector_version },
    {
      id: 'seen',
      header: t('machines.list.seen'),
      from: 'md',
      className: 'whitespace-nowrap text-text-muted',
      cell: (machine) => (machine.last_seen_at === null ? t('machines.list.never') : <RelativeTime at={machine.last_seen_at} />),
    },
    {
      id: 'roles',
      header: t('machines.list.roles'),
      from: 'lg',
      cell: (machine) => <MachineRoleChips roles={machine.roles} />,
    },
  ];

  const add = (
    <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
      {t('machines.list.add')}
    </Button>
  );

  return (
    <>
      <ResourceList
        kind="machines"
        search={{ ...search }}
        acts={acts.list}
        title={t('screens.machines.title')}
        icon={Laptop}
        lead={t('machines.list.lead')}
        label={t('machines.list.label')}
        readRow={resourceReaders.machines}
        filter={{ state: search.status }}
        columns={columns}
        rowKey={(machine) => machine.id}
        rowClassName={(machine) => (machine.status === 'revoked' ? 'bg-bg-subtle' : undefined)}
        text={text}
        onText={setText}
        searchLabel={t('machines.list.search')}
        searchPlaceholder={t('machines.list.searchPlaceholder')}
        filters={
          <SegmentedControl
            label={t('machines.list.filter')}
            value={search.status ?? 'all'}
            onValueChange={(value) => onSearch({ status: value === 'all' ? undefined : value })}
            segments={[
              { value: 'all', label: t('machines.list.all') },
              { value: 'online', label: t('machines.state.online') },
              { value: 'offline', label: t('machines.state.offline') },
              { value: 'revoked', label: t('machines.state.revoked') },
            ]}
          />
        }
        action={add}
        empty={{ icon: <Laptop aria-hidden />, title: t('machines.list.none'), description: t('machines.list.noneLead'), action: add }}
        noMatch={t('machines.list.noMatch')}
      />
      {adding && <AddMachineDialog onClose={() => setAdding(false)} />}
    </>
  );
}
