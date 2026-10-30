/** Person, exact act, kind and date filters with their state carried by the trail's address. */
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import type { UserRef, AuditKind } from '@/generated/api-types';
import { PersonLocator } from '@/features/people/PersonLocator';
import { Checkbox } from '@/ui/Checkbox';
import { DateField } from '@/ui/DateField';
import { compactControls } from '@/ui/FilterBar';
import { cx } from '@/ui/cx';
import { Select } from '@/ui/Select';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { ACTIONS } from './catalogue';
import { AUDIT_KINDS, type TraceScope, type TraceSearch } from './params';
import { auditActionName } from './words';

export function TraceFilters({ scope, filters, person, onChange }: { scope: TraceScope; filters: TraceSearch; person: UserRef | null; onChange: (next: TraceSearch) => void }) {
  const { t } = useTranslation();
  const knownKind = filters.kind !== undefined && AUDIT_KINDS.includes(filters.kind as AuditKind);
  const acts = knownKind ? ACTIONS[filters.kind as AuditKind] : Object.values(ACTIONS).flat();
  return <div className={cx('grid min-w-0 grid-cols-1 items-end gap-3 @min-[36rem]:grid-cols-2 @min-[64rem]:grid-cols-3', compactControls)}>
    <PersonLocator key={scope.type === 'node' ? scope.id : 'system'} label={t('trace.person')} value={filters.actor ?? null} person={person}
      orgId={scope.type === 'node' ? scope.orgId : null} onChoose={(person) => onChange({ ...filters, actor: person?.id })} />
    <Select label={t('trace.kind')} value={filters.kind ?? 'all'} onValueChange={(value) => onChange({ ...filters, kind: value === 'all' ? undefined : value })} options={[{ value: 'all', ...choiceOption(t, 'dynamic.value', { name: t('trace.allKinds'), context: t('trace.kind') }) }, ...(filters.kind && !knownKind ? [{ value: filters.kind, name: t('trace.kinds.unknown') }] : []), ...AUDIT_KINDS.map((kind) => ({ value: kind, ...choiceOption(t, `traceKind.${kind}`) }))]} />
    {filters.actor && <Checkbox label={t('trace.includeAgents')} checked={filters.include_agents === true} onCheckedChange={(include_agents) => onChange({ ...filters, include_agents })} />}
    <DateField label={t('trace.from')} value={filters.from?.slice(0, 10) ?? ''} onValueChange={(value) => onChange({ ...filters, from: value || undefined })} />
    <DateField label={t('trace.to')} value={filters.to?.slice(0, 10) ?? ''} onValueChange={(value) => onChange({ ...filters, to: value || undefined })} />
    <ChoiceLocator label={t('trace.action')} value={filters.action ?? 'all'} onValueChange={(value) => onChange({ ...filters, action: value === 'all' ? undefined : value })} options={[{ value: 'all', name: t('trace.allActions') }, ...(filters.action && !acts.includes(filters.action) ? [{ value: filters.action, name: auditActionName(filters.action, t) }] : []), ...acts.map((action) => ({ value: action, name: auditActionName(action, t) }))]} />
  </div>;
}
