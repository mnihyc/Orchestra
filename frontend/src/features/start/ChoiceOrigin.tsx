/** The origin of a selected start value, with the same mark in both entry points. */
import { useTranslation } from 'react-i18next';
import { offeredChoice } from '@/i18n/choices';
import { providerPickerLabels } from '@/features/resources/providers/labels';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { ResourceSubject } from '@/features/resource-facts/facts';
import { Badge } from '@/ui/Badge';
import { modelName, type ChoiceOrigin, type Resolved } from './choices';

export function ChoiceOriginMark({ origin }: { origin: ChoiceOrigin | 'manual' }) {
  const { t } = useTranslation();
  return origin === null ? null : <Badge>{origin === 'manual' ? t('startImplicit.origin.manual') : origin === 'topic_default' ? t('start.topicDefault') : t(`startImplicit.origin.${origin}`)}</Badge>;
}

/** One origin describes the setup; only actual exceptions need their own mark. */
export function StartValues({ resolved }: { resolved: Resolved }) {
  const { t } = useTranslation();
  const providerNames = providerPickerLabels(resolved.providers.map((row) => row.entry.provider));
  const values: { field: keyof Resolved['origins']; name: string | null; subject?: ResourceSubject }[] = [
    { field: 'agent', name: resolved.agent?.entry.agent.name ?? null, subject:resolved.agent===null?undefined:{kind:'agent',agent:resolved.agent.entry.agent} },
    { field: 'provider', name: resolved.provider === null ? null : providerNames.get(resolved.provider.id) ?? resolved.provider.entry.provider.name, subject:resolved.provider===null?undefined:{kind:'provider',provider:resolved.provider.entry.provider} },
    { field: 'model', name: resolved.model === null ? null : modelName(resolved.model), subject:resolved.model===null||resolved.provider===null?undefined:{kind:'model',model:resolved.model,providerId:resolved.provider.id} },
    { field: 'effort', name: resolved.effort === null ? null : offeredChoice(t, 'effort', resolved.effort)?.name ?? t('changeCompact.otherEffort') },
    { field: 'environment', name: resolved.fresh ? t('start.quick.inNew') : resolved.environment?.environment.name ?? null, subject:resolved.environment===null?undefined:{kind:'environment',environment:resolved.environment.environment} },
    { field: 'arrangement', name: resolved.arrangement === null ? null : t(`choiceCatalog.arrangement.${resolved.arrangement}.name`) },
  ];
  const visible = values.filter((entry) => entry.name !== null);
  const first = visible[0];
  if (first === undefined) return null;
  const counts = new Map<ChoiceOrigin, number>();
  for (const { field } of visible) {
    const origin = resolved.origins[field];
    counts.set(origin, (counts.get(origin) ?? 0) + 1);
  }
  // The first displayed value wins ties, keeping the common label stable.
  let common = resolved.origins[first.field];
  for (const { field } of visible) {
    const origin = resolved.origins[field];
    if ((counts.get(origin) ?? 0) > (counts.get(common) ?? 0)) common = origin;
  }
  return <>
    <ChoiceOriginMark origin={common} />
    <span role="list" className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
      {visible.map(({ field, name, subject }) => <span key={field} role="listitem" className="inline-flex min-w-0 flex-wrap items-center gap-1" data-start-value={field}>
        {subject===undefined ? <span className="max-w-full truncate" title={name ?? undefined}>{name}</span> : <ResourceChip subject={subject} hint={false}>{name}</ResourceChip>}
        {resolved.origins[field] !== common && <ChoiceOriginMark origin={resolved.origins[field] ?? 'manual'} />}
      </span>)}
    </span>
  </>;
}
