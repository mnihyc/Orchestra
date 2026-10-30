/** A machine's reported port ceiling in human words; command text retains its exact original JSON. */
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { record } from '@/features/resources/UnitCommands';
import type { PortOpeningKind, PortRange } from '@/generated/api-types';
import { choiceCopy } from '@/i18n/choices';
import { Choice } from '@/ui/Choice';

function audience(value: unknown): value is PortOpeningKind {
  return value === 'place' || value === 'host' || value === 'link';
}

function ranges(value: unknown): PortRange[] | null {
  if (!Array.isArray(value)) return null;
  const rows = value.map((item: unknown): PortRange | null => {
    const { from, to } = record(item);
    return typeof from === 'number' && typeof to === 'number' && Number.isInteger(from) && Number.isInteger(to)
      && from >= 1 && to <= 65_535 && from <= to ? { from, to } : null;
  });
  return rows.some((row) => row === null) ? null : rows.filter((row): row is PortRange => row !== null);
}

function rangeWords(rows: readonly PortRange[]): string {
  return rows.map((row) => row.from === row.to ? String(row.from) : `${row.from}–${row.to}`).join(', ');
}

export function PortPolicyValue({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const policy = record(value);
  const offered: unknown[] | null = Array.isArray(policy.allowed) ? policy.allowed : null;
  const allowed = offered?.every(audience) === true ? offered : null;
  if (allowed === null) return <p className="text-sm text-text-muted" data-policy-port-value>{t('machinePortValue.unavailable')}</p>;
  const publish = record(policy.publish);
  const publishedRanges = ranges(publish.ranges);
  const declared = typeof publish.incoming_address === 'string' && publish.incoming_address.trim() !== ''
    && typeof publish.listen === 'string' && publish.listen.trim() !== '' && publishedRanges !== null && publishedRanges.length > 0;
  const limits = policy.limits === null || policy.limits === undefined ? null : record(policy.limits);
  const limitedRanges = ranges(limits?.ranges);
  const address = (candidate: unknown) => typeof candidate === 'string' && candidate.trim() !== '' ? candidate : t('machinePortValue.unavailable');
  const listen = (candidate: unknown) => candidate === 'all' ? choiceCopy(t, 'hostBinding.all').name : address(candidate);
  return <div className="flex flex-col gap-3" data-policy-port-value>
    <div><p className="text-sm font-medium">{t('machinePortValue.audiences')}</p>
      {allowed.length === 0 ? <EmptyState compact title={t('machines.policy.none')} />
          : <ul className="flex flex-col gap-1">{allowed.map((kind) => <li key={kind}><Choice {...choiceCopy(t, `portAudience.${kind}`)} compact /></li>)}</ul>}
    </div>
    <div><p className="text-sm font-medium">{t('machinePortValue.listener')}</p>
      {declared ? <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
        <dt>{t('ports.hostForm.reached')}</dt><dd className="wrap-anywhere">{address(publish.incoming_address)}</dd>
        <dt>{t('ports.hostForm.bind')}</dt><dd className="wrap-anywhere">{listen(publish.listen)}</dd>
        <dt>{t('ports.hostForm.range')}</dt><dd>{rangeWords(publishedRanges)}</dd>
      </dl> : <p className="text-sm text-text-muted">{t(policy.publish === null || policy.publish === undefined ? 'machinePortValue.noListener' : 'machinePortValue.unavailable')}</p>}
    </div>
    <div><p className="text-sm font-medium">{t('machinePortValue.limits')}</p>
      {limits === null ? <p className="text-sm text-text-muted">{t('machinePortValue.noLimits')}</p> : <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
        <dt>{t('ports.hostForm.reached')}</dt><dd className="wrap-anywhere">{limits.incoming_address === null || limits.incoming_address === undefined ? t('machinePortValue.unrestricted') : address(limits.incoming_address)}</dd>
        <dt>{t('ports.hostForm.bind')}</dt><dd className="wrap-anywhere">{limits.listen === null || limits.listen === undefined ? t('machinePortValue.unrestricted') : listen(limits.listen)}</dd>
        <dt>{t('ports.hostForm.range')}</dt><dd>{limits.ranges === null || limits.ranges === undefined ? t('machinePortValue.unrestricted') : limitedRanges === null ? t('machinePortValue.unavailable') : limitedRanges.length === 0 ? t('machines.policy.none') : rangeWords(limitedRanges)}</dd>
      </dl>}
    </div>
  </div>;
}
