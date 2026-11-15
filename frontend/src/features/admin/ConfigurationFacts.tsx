/** Only the reporting process's captured allowlist belongs to its System disclosure. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { ConfigurationKey, ConfigurationValue, SystemInfo } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { FactDisclosure, type DisclosureFact } from '@/ui/FactDisclosure';
import { FilterBar } from '@/ui/FilterBar';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';

const labels: Record<ConfigurationKey, string> = {
  'database.acquire_timeout_ms': 'admin.configuration.acquire',
  'database.request_statement_timeout_ms': 'admin.configuration.requestStatement',
  'database.background_statement_timeout_ms': 'admin.configuration.backgroundStatement',
  'database.idle_transaction_timeout_ms': 'admin.configuration.idleTransaction',
};

export function ConfigurationFacts({ observation }: { observation: SystemInfo['effective_configuration'] }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const [search, setSearch] = useState('');
  const duration = (value: ConfigurationValue): ReactNode => {
    if (value.kind === 'indefinite') return t('admin.configuration.noDeadline');
    if (!/^\d{1,20}$/.test(value.milliseconds)) return t('factDisclosure.notReported');
    return <span title={`${BigInt(value.milliseconds).toLocaleString(context.locale)} ms`}>{formatSpan(Number(value.milliseconds), context)}</span>;
  };
  const entries = observation?.values.items ?? [];
  const filtered = entries.filter((entry) => t(labels[entry.public_key]).toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()));
  const facts: DisclosureFact[] = !observation ? [{ key: 'unreported', label: t('admin.configuration.running') }]
    : filtered.map((entry) => ({ key: entry.public_key, label: t(labels[entry.public_key]), value: <span className="inline-flex min-w-0 flex-wrap items-baseline gap-1">
      <span>{t('admin.configuration.runningValue')}:</span>{duration(entry.typed_value)}<span>·</span>
      <span>{entry.origin === null ? t('factDisclosure.notReported') : t(`admin.configuration.origins.${entry.origin}`)}</span>
      <span>· {t(entry.application === 'restart' ? 'admin.configuration.restart' : 'admin.configuration.live')}</span>
    </span> }));
  return <FactDisclosure groups={[{ key: 'configuration', kind: 'current_setting', title: !observation ? t('admin.configuration.running') : t('admin.configuration.runningService', { service: t(`admin.configuration.services.${observation.service}`) }), facts }]}
    header={!observation ? undefined : <div className="flex min-w-0 flex-col gap-2">
      <span className="text-sm text-text-muted"><RelativeTime at={observation.measured_at}/></span>
      <FilterBar search={<Input label={t('admin.configuration.search')} hideLabel type="search" value={search} onChange={(event) => setSearch(event.target.value)} autoComplete="off"/>}/>
      {filtered.length === 0 && <span className="text-sm text-text-muted">{t('admin.configuration.noMatch')}</span>}
    </div>}/>;
}
