/** A host manager sees kept disk and unresolved cleanup without receiving an early-purge act. */
import { HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Host } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { EmptyState } from '@/ui/EmptyState';
import { retainedJobWords } from './jobWords';

export function HostKept({ host }: { host: Pick<Host, 'kept_data' | 'access'> }) {
  const { t } = useTranslation(); const format = useFormatContext();
  if (!holds(host.access, 'host.manage')) return null;
  // These rows are already cut by the host reader; no second route or environment lookup is needed.
  const rows = host.kept_data ?? [];
  const bytes = rows.reduce((sum, row) => sum + (row.bytes ?? 0), 0);
  const unknown = rows.filter((row) => row.bytes === null).length;
  const day = (value: string | null) => value === null ? t('trace.unknown') : new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(value));
  return <Section id="host-kept" title={t('trace.keptTitle')} lead={t('trace.keptLead')}>
    {rows.length>0 && <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted"><span>{t('trace.hostMeasured',{count:rows.length,size:formatBytes(bytes,format.locale)})}</span>{unknown>0 && <span>{t('trace.hostUnmeasured',{count:unknown})}</span>}</p>}
    {rows.length === 0 ? <EmptyState compact icon={<HardDrive aria-hidden />} title={t('trace.keptEmpty')} /> : <ul className="divide-y divide-border">{rows.map((entry) => <li key={entry.environment_id} data-kept-environment={entry.environment_id} className="flex flex-col gap-2 py-4">
      <div className="flex flex-wrap items-center gap-2"><span className="font-medium wrap-anywhere">{entry.name ?? t('placeReview.privateEnvironment')}</span><Badge tone="warning">{t(`trace.phases.${entry.phase}`)}</Badge></div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
        <dt className="text-text-muted">{t('trace.size')}</dt><dd>{entry.bytes === null ? t('trace.unknown') : formatBytes(entry.bytes, format.locale)}</dd>
        <dt className="text-text-muted">{t('trace.home')}</dt><dd className="wrap-anywhere">{entry.home?.redacted === true ? t('trace.unknown') : entry.home?.name ?? t('trace.unknown')}</dd>
        <dt className="text-text-muted">{t('trace.storage')}</dt><dd className="wrap-anywhere">{entry.storages.map((storage) => storage ?? t('hosts.storages.default')).join(', ') || t('trace.unknown')}</dd>
      </dl>
      <p className="text-sm text-text-muted">{t('trace.until', { day: day(entry.purge_at) })}</p>
      {entry.job !== null && <p className="text-sm wrap-anywhere text-text-muted">{retainedJobWords(t,entry.job)}</p>}
    </li>)}</ul>}
  </Section>;
}
