/** External plugin lifecycle and errors.
 *
 * The external implementations loaded on any part of the system: a notifier, a driver, an executor.
 * Each is a row with its kind, name and version, where it runs (a part of the system, or a machine),
 * the state it is in and, where it is not running well, what it last said was wrong. A state is
 * always a word beside its mark.
 */
import { useQuery } from '@tanstack/react-query';
import { CircleAlert, CircleCheck, Puzzle, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { pluginsQuery } from '@/api/admin';
import type { AdminPlugin } from '@/generated/api-types';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';

/** The column that holds a plugin's state. */
const STATE = 'state';

const STATES = {
  running: { Icon: CircleCheck, tone: 'text-success' },
  restarting: { Icon: RefreshCw, tone: 'text-warning' },
  failed: { Icon: CircleAlert, tone: 'text-danger' },
} as const;

function State({ plugin }: { plugin: AdminPlugin }) {
  const { t } = useTranslation();
  const { Icon, tone } = STATES[plugin.state];
  return (
    <span className="flex items-center gap-1 text-text">
      <Icon aria-hidden className={tone} />
      {t(`admin.plugins.states.${plugin.state}`)}
    </span>
  );
}

export function Plugins() {
  const { t, i18n } = useTranslation();
  const plugins = useQuery(pluginsQuery);
  const where = (plugin: AdminPlugin): string => {
    const key = `admin.plugins.components.${plugin.component}`;
    const part = i18n.exists(key) ? (t as (key: string) => string)(key) : plugin.component;
    return plugin.machine === null ? part : t('admin.plugins.onMachine', { part, machine: plugin.machine.name });
  };

  const columns: Column<AdminPlugin>[] = [
    {
      id: 'name',
      header: t('admin.plugins.name'),
      cell: (plugin, shown) => {
        const stateHere = !shown.has(STATE);
        return (
          <span className="flex min-w-0 flex-col gap-1">
            <span className="font-medium text-text">{plugin.name}</span>
            <span className="text-sm text-text-muted">{plugin.kind}</span>
            {stateHere && <State plugin={plugin} />}
          </span>
        );
      },
    },
    { id: 'version', header: t('admin.plugins.version'), from: 'sm', cell: (plugin) => (plugin.version === '' ? <span className="text-text-subtle">{t('admin.plugins.noVersion')}</span> : <span className="font-mono">{plugin.version}</span>) },
    { id: 'where', header: t('admin.plugins.where'), from: 'md', cell: (plugin) => <span className="wrap-anywhere">{where(plugin)}</span> },
    { id: STATE, header: t('admin.plugins.state'), from: 'sm', cell: (plugin) => <State plugin={plugin} /> },
    { id: 'error', header: t('admin.plugins.lastError'), cell: (plugin) => (plugin.detail === '' ? <span className="text-text-subtle">–</span> : <span className="text-sm wrap-anywhere text-text-muted">{plugin.detail}</span>) },
  ];

  if (plugins.isPending) return <SkeletonRows rows={3} />;
  if (plugins.isError) return <ErrorState error={plugins.error} retrying={plugins.isRefetching} onRetry={() => void plugins.refetch()} />;
  if (plugins.data.length === 0) return <EmptyState compact icon={<Puzzle aria-hidden />} title={t('admin.plugins.none')} description={t('admin.plugins.noneHint')} />;
  return <Table label={t('admin.sections.plugins.title')} columns={columns} rows={plugins.data} rowKey={(plugin) => `${plugin.component}/${plugin.machine?.id ?? ''}/${plugin.kind}/${plugin.name}`} />;
}
