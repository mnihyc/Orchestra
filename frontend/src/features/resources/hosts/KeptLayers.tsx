/** A host shows each environment's kept installed-file layers, including archived environments with no unit. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { Host, HostKeptLayers } from '@/generated/api-types';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Table, type Column } from '@/ui/Table';
import { StateLine } from '@/ui/StateLine';

export function KeptLayers({ host }: { host: Host }) {
  const { t, i18n } = useTranslation();
  const rows = host.kept_layers ?? [];
  if (rows.length === 0) return <Section id="host-kept-layers" title={t('keptLayers.title')}>
    <StateLine compact message={t(host.kept_layers === undefined || !(holds(host.access, 'host.view') || holds(host.access, 'host.manage')) ? 'implicitAvailability.detailsUnavailable' : 'implicitAvailability.noKeptLayers')} />
  </Section>;
  // Layers of one environment come in order; a person is told which is the latest, not what number it has.
  const latest = new Map<string, number>();
  for (const row of rows) latest.set(row.environment_id, Math.max(latest.get(row.environment_id) ?? row.generation, row.generation));
  const columns: Column<HostKeptLayers>[] = [
    { id: 'environment', header: t('keptLayers.environment'), cell: (row) => row.environment === null ? t('ui.nameUnknown') : <Link to="/resources/environments/$id" params={{ id: row.environment_id }} className="text-link hover:underline">{row.environment.name}</Link> },
    { id: 'which', header: t('keptLayers.which'), cell: (row) => t(row.generation === latest.get(row.environment_id) ? 'keptLayers.latest' : 'keptLayers.earlier') },
    { id: 'bytes', header: t('keptLayers.size'), cell: (row) => formatBytes(row.bytes, i18n.resolvedLanguage ?? 'en') },
  ];
  return <Section id="host-kept-layers" title={t('keptLayers.title')} lead={t('keptLayers.lead')}><Table label={t('keptLayers.title')} columns={columns} rows={rows} rowKey={(row) => `${row.environment_id}:${row.generation}`} /></Section>;
}
