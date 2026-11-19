/** The system's hosts: capacity, a meter of what their units declare they need, what each does when full, load, drivers, enrollment, runtimes, mount locations, recycle and move.
 *
 * Every host on the system, the system's own and those members added for their orgs: whose it is, how
 * much it has, how much of its memory the units on it declare they need (nothing of it is reserved),
 * the drivers it reports and what runs on it. A host whose declared memory reaches the system's
 * percentage is marked full, with what being full does there: it warns and lets the host answer, or it
 * takes nothing more. Above the list are the two settings that decide that for every host that says
 * nothing of its own, each changed where it is shown. A host's own page, where it is managed, is one
 * click from its name.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { Server } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { hostsQuery } from '@/api/admin';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { fullEffect, FULL_PERCENT_SHIPPED, declaredShare, megabytes } from '@/features/admin/hosts';
import { ChoiceSetting, NumberSetting, useSystemSettings } from '@/features/admin/SettingRows';
import type { Host, WhenFull } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { boxClasses, Section } from '@/shell/Page';
import { StatusTag } from '@/features/admin/StatusTag';
import { buttonClasses } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

/** The column that holds a host's memory. */
const MEMORY = 'memory';

/** The memory of a host as a meter with the figures said in words, and the badge that says it is full. */
function Memory({ host, percent, inherited }: { host: Host; percent: number; inherited: WhenFull | undefined }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  if (host.capacity.memory_mb <= 0) return <span className="text-sm text-text-muted">–</span>;
  const effect = fullEffect(host, percent);
  // Native host/fleet readers resolve own.unwrap_or(system); this page already observes the current system value.
  const does = effect.own ? effect.does : inherited ?? effect.does;
  const share = declaredShare(host.capacity, host.declared);
  const figures = t('admin.hosts.memoryOf', { declared: formatBytes(megabytes(host.declared.memory_mb), locale), capacity: formatBytes(megabytes(host.capacity.memory_mb), locale) });
  return (
    <div className="flex min-w-[10rem] flex-col gap-1">
      <Meter value={share} label={t('admin.hosts.memoryLabel', { name: host.name })} look="compact" />
      <span className="text-sm text-text-muted">{figures}</span>
      {effect.full && (
        <span className="flex flex-wrap items-center gap-1" data-full={does}>
          <StatusTag tone={does === 'block' ? 'danger' : 'warning'}>{t('admin.hosts.full')}</StatusTag>
          <span className="text-sm text-text">{t(`admin.hosts.does.${does}`)}</span>
          {effect.own && <span className="text-sm text-text-muted">{t('admin.hosts.ownWord')}</span>}
        </span>
      )}
    </div>
  );
}

export function Hosts() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const hosts = useQuery(hostsQuery);
  const { find } = useSystemSettings();
  const percentSetting = find('hosts.full_percent')?.value;
  const percent = typeof percentSetting === 'number' ? percentSetting : FULL_PERCENT_SHIPPED;
  const whenFull = find('hosts.when_full')?.value;
  const inherited = whenFull === 'warn' || whenFull === 'block' ? whenFull : undefined;

  const columns: Column<Host>[] = [
    {
      id: 'host',
      header: t('admin.hosts.host'),
      cell: (host, shown) => {
        // Where the memory column is left out for want of room, the meter stands under the name.
        const meterHere = !shown.has(MEMORY);
        return (
          <span className="flex min-w-0 flex-col gap-1">
            <ResourceName subject={{ kind: 'host', host: { ...host, full: fullEffect(host, percent).full } }} asChild><Link to="/resources/hosts/$id" params={{ id: host.id }} className="w-fit font-medium text-accent-text hover:underline">
              {host.name}
            </Link></ResourceName><ResourceHint subject={{ kind: 'host', host: { ...host, full: fullEffect(host, percent).full } }} />
            <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
              {host.home === null ? <span>{t('admin.hosts.system')}</span> : <span>{t('admin.hosts.homeOf', { org: host.home.name ?? '' })}</span>}
              {!host.schedulable && <StatusTag>{t('admin.hosts.notSchedulable')}</StatusTag>}
            </span>
            <span className="text-sm wrap-anywhere text-text-subtle">{host.drivers.map((driver) => driver.title).join(', ')}</span>
            {meterHere && <Memory host={host} percent={percent} inherited={inherited} />}
          </span>
        );
      },
    },
    { id: 'owner', header: t('admin.hosts.owner'), from: 'sm', cell: (host) => (host.owner === null ? <span className="text-text-subtle">{t('admin.hosts.noOwner')}</span> : <UserChip user={host.owner} size={24} />) },
    { id: MEMORY, header: t('admin.hosts.memory'), from: 'md', cell: (host) => <Memory host={host} percent={percent} inherited={inherited} /> },
    {
      id: 'holds',
      header: t('admin.hosts.holds'),
      from: 'lg',
      cell: (host) => (
        <span className="flex flex-col text-sm whitespace-nowrap text-text-muted">
          <span>{host.capacity.cpu_millis > 0 ? t('admin.hosts.cpus', { count: host.capacity.cpu_millis / 1000, number: formatNumber(host.capacity.cpu_millis / 1000, locale) }) : '–'}</span>
          <span>{host.capacity.disk_gb > 0 ? t('admin.hosts.disk', { size: formatBytes(host.capacity.disk_gb * 1024 ** 3, locale) }) : '–'}</span>
          <span>{t('admin.hosts.environments', { count: host.running.environment, number: formatNumber(host.running.environment, locale) })}</span>
          <span>{t('admin.hosts.providers', { count: host.running.provider, number: formatNumber(host.running.provider, locale) })}</span>
        </span>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <Section id="admin-hosts-policy" title={t('admin.hosts.policyTitle')} lead={t('admin.hosts.policyLead')}>
        <div className={cx(boxClasses, 'flex flex-col gap-6 p-4')}>
          <NumberSetting settingKey="hosts.full_percent" min={1} max={100} unit="%" label={t('admin.hosts.fullFrom')} hint={t('admin.hosts.fullFromHint')} />
          <ChoiceSetting
            settingKey="hosts.when_full"
            label={t('admin.hosts.whenFull')}
            hint={t('admin.hosts.whenFullHint')}
            options={[
              { value: 'warn', label: t('admin.hosts.whenFullWarn') },
              { value: 'block', label: t('admin.hosts.whenFullBlock') },
            ]}
          />
        </div>
      </Section>

      <Section
        id="admin-hosts-list"
        title={t('admin.hosts.listTitle')}
        lead={t('admin.hosts.listLead')}
        action={
          <Link to="/resources/hosts" className={buttonClasses({ size: 'sm' })}>
            {t('admin.hosts.addOpen')}
          </Link>
        }
      >
        {hosts.isPending ? (
          <SkeletonRows rows={3} />
        ) : hosts.isError ? (
          <ErrorState error={hosts.error} retrying={hosts.isRefetching} onRetry={() => void hosts.refetch()} />
        ) : hosts.data.length === 0 ? (
          <EmptyState compact icon={<Server aria-hidden />} title={t('admin.hosts.none')} description={t('admin.hosts.noneHint')} />
        ) : (
          <>
            <Table label={t('admin.sections.hosts.title')} columns={columns} rows={hosts.data} rowKey={(host) => host.id} />
            <p className="text-sm text-text-muted">{t('admin.hosts.note')}</p>
          </>
        )}
      </Section>
    </div>
  );
}
