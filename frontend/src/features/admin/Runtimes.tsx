/** Both pools on every host: environments and provider runtimes, what each is behind in, Update required, and updating those ticked.
 *
 * Every unit the system runs, in the two pools it keeps: the environments people work in and the
 * runtimes that providers run in. Each row says which unit it is, where it runs, what state it is in
 * and why where it is not well, what it is made from, how many things it is behind in, and when it
 * last did anything. Narrowed to one pool or one host from the address. A unit that is behind is
 * updated from its own page, which its name opens.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Boxes } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { hostsQuery, runtimesQuery } from '@/api/admin';
import type { SectionProps } from '@/features/admin/sections';
import type { Runtime } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { StatusTag } from '@/features/admin/StatusTag';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { FilterBar } from '@/ui/FilterBar';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { type DotTone, StatusDot } from '@/ui/StatusDot';
import { type Column, Table } from '@/ui/Table';

type Pool = 'all' | 'environment' | 'provider';

/** The column that holds a unit's state. */
const STATE = 'state';
const BEHIND = 'behind';
const ALL_HOSTS = '-';

/** States use the shared chip and catalogue words; a future state never exposes an internal key. */
const STATE_TONES: Record<string, DotTone> = {
  running: 'success',
  stopped: 'idle',
  provisioning: 'info',
  recreating: 'info',
  moving: 'info',
  failed: 'danger',
  offline: 'off',
  disabled: 'off',
};

function State({ runtime }: { runtime: Runtime }) {
  const { t } = useTranslation();
  const key = `admin.runtimes.states.${runtime.state}`;
  const word = t(key, { defaultValue: t('ui.statusUnknown') });
  return (
    <span className="flex min-w-0 flex-col">
      <StatusDot tone={STATE_TONES[runtime.state] ?? 'idle'} label={word} showLabel />
      {runtime.state_detail !== null && <span className="text-sm wrap-anywhere text-text-muted">{runtime.state_detail}</span>}
    </span>
  );
}

export function Runtimes({ search, onSearch }: SectionProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const pool: Pool = search.kind ?? 'all';
  const host = search.host ?? ALL_HOSTS;
  const hosts = useQuery(hostsQuery);
  const runtimes = useQuery(runtimesQuery({ host: host === ALL_HOSTS ? '' : host, kind: pool === 'all' ? '' : pool }));
  const hostName = (id: string) => hosts.data?.find((one) => one.id === id)?.name ?? '';
  const behind = (runtimes.data ?? []).filter((one) => one.updates > 0).length;

  const columns: Column<Runtime>[] = [
    {
      id: 'unit',
      header: t('admin.runtimes.unit'),
      cell: (runtime, shown) => {
        const stateHere = !shown.has(STATE);
        const name = runtime.environment?.name ?? runtime.provider?.name ?? t('ui.nameUnknown');
        return (
          <span className="flex min-w-0 flex-col gap-1">
            {runtime.environment !== null ? (
              <Link to="/resources/environments/$id" params={{ id: runtime.environment.id }} className="focus-inset w-fit max-w-full font-medium wrap-anywhere text-text hover:underline">
                {name}
              </Link>
            ) : runtime.provider !== null ? (
              <Link to="/resources/providers/$id" params={{ id: runtime.provider.id }} className="focus-inset w-fit max-w-full font-medium wrap-anywhere text-text hover:underline">
                {name}
              </Link>
            ) : (
              <span className="font-medium text-text">{name}</span>
            )}
            {pool === 'all' && <span className="text-sm text-text-muted">{t(`admin.runtimes.pools.${runtime.kind}`)}</span>}
            {stateHere && <State runtime={runtime} />}
            {!shown.has(BEHIND) && runtime.updates > 0 && <StatusTag tone="warning">
              {t('admin.runtimes.behindIn', { count: runtime.updates, number: formatNumber(runtime.updates, locale) })}
            </StatusTag>}
          </span>
        );
      },
    },
    { id: 'host', header: t('admin.runtimes.host'), from: 'md', cell: (runtime) => <span className="wrap-anywhere">{hostName(runtime.host_id)}</span> },
    { id: STATE, header: t('admin.runtimes.state'), from: 'sm', cell: (runtime) => <State runtime={runtime} /> },
    { id: 'template', header: t('admin.runtimes.template'), from: 'lg', cell: (runtime) => <span className="wrap-anywhere">{runtime.template.name}</span> },
    {
      id: BEHIND,
      header: t('admin.runtimes.behind'),
      from: 'md',
      cell: (runtime) =>
        runtime.updates > 0 ? (
          <StatusTag tone="warning">{t('admin.runtimes.behindIn', { count: runtime.updates, number: formatNumber(runtime.updates, locale) })}</StatusTag>
        ) : (
          <span className="text-text-subtle">{t('admin.runtimes.current')}</span>
        ),
    },
    {
      id: 'active',
      header: t('admin.runtimes.lastActive'),
      from: 'lg',
      cell: (runtime) => (runtime.last_active_at === null ? <span className="text-text-subtle">{t('admin.runtimes.never')}</span> : <RelativeTime at={runtime.last_active_at} className="text-text-muted" />),
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <FilterBar>
        <SegmentedControl
          label={t('admin.runtimes.pool')}
          value={pool}
          onValueChange={(next) => onSearch({ kind: next === 'all' ? undefined : next })}
          segments={[
            { value: 'all', name: t('choiceCatalog.runtimePool.all.name') },
            { value: 'environment', name: t('choiceCatalog.runtimePool.environment.name') },
            { value: 'provider', name: t('choiceCatalog.runtimePool.provider.name') },
          ]}
        />
        <Select
          label={t('admin.runtimes.host')}
          value={host}
          onValueChange={(next) => onSearch({ host: next === ALL_HOSTS ? undefined : next })}
          options={[{ value: ALL_HOSTS, name: t('admin.runtimes.allHostsName'), description: t('admin.runtimes.allHosts') }, ...(hosts.data ?? []).map((one) => ({ value: one.id, ...choiceOption(t, 'dynamic.resource', { name: one.name }) }))]}
          fieldClassName="min-w-[12rem]"
        />
      </FilterBar>

      {runtimes.isPending ? (
        <SkeletonRows rows={4} />
      ) : runtimes.isError ? (
        <ErrorState error={runtimes.error} retrying={runtimes.isRefetching} onRetry={() => void runtimes.refetch()} />
      ) : runtimes.data.length === 0 ? (
        <EmptyState compact icon={<Boxes aria-hidden />} title={t('admin.runtimes.none')} />
      ) : (
        <>
          <Table label={t('admin.sections.runtimes.title')} columns={columns} rows={runtimes.data} rowKey={(runtime) => runtime.id} />
          {behind > 0 && <p className="text-sm text-text-muted">{t('admin.runtimes.updateNote', { count: behind, number: formatNumber(behind, locale) })}</p>}
        </>
      )}
    </div>
  );
}
