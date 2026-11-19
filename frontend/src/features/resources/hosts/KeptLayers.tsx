/** A host shows each environment's kept installed-file layers, including archived environments with no unit. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { Host, HostKeptLayers } from '@/generated/api-types';
import { formatBytes } from '@/lib/format';
import { Section } from '@/shell/Page';
import { Table, type Column } from '@/ui/Table';

export function KeptLayers({ host }: { host: Host }) {
  const { t, i18n } = useTranslation();
  const rows = host.kept_layers ?? [];
  if (rows.length === 0) return null;
  const columns: Column<HostKeptLayers>[] = [
    { id: 'environment', header: t('keptLayers.environment'), cell: (row) => row.environment === null ? t('ui.nameUnknown') : <Link to="/resources/environments/$id" params={{ id: row.environment_id }} className="text-link hover:underline">{row.environment.name}</Link> },
    { id: 'generation', header: t('keptLayers.generation'), cell: (row) => row.generation },
    { id: 'bytes', header: t('keptLayers.size'), cell: (row) => formatBytes(row.bytes, i18n.resolvedLanguage ?? 'en') },
  ];
  return <Section id="host-kept-layers" title={t('keptLayers.title')} lead={t('keptLayers.lead')}><Table label={t('keptLayers.title')} columns={columns} rows={rows} rowKey={(row) => `${row.environment_id}:${row.generation}`} /></Section>;
}
