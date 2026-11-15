/** A speed, an access program or a context size that the model's managers switched on and that this thread cannot use: shown off in its own control, with what stands in the way.
 *
 * The thread's parameters say each such choice and why (`withheld`). Its control stands where the working one
 * would, dimmed, and opens a list of what the managers switched on, all off; above that list stands the reason in a
 * few words and, where the person may lift it, the act that does: the setting that has the feature off, or the
 * change of how or where the thread runs. Nothing is guessed here: a choice that is not listed is not shown.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { openChange } from '@/features/change/target';
import { ChoiceChip, type PickRow } from '@/features/composer/PickList';
import { NONE } from '@/features/composer/SpeedAndAccess';
import { formatContext } from '@/features/models/context';
import { accessProgramDisplayName } from '@/features/models/names';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { openWhere } from '@/features/thread/openWhere';
import type { ModelParameters, Thread, ThreadParameters, WithheldChoice, WithheldParameter } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';

/** The choice of a kind that the thread's parameters say is withheld, if one is. */
export const withheldOf = (parameters: ThreadParameters | undefined, choice: WithheldChoice): WithheldParameter | undefined =>
  parameters?.withheld?.find((entry) => entry.choice === choice);

/** The reason, in a few words. */
export function withheldWords(entry: WithheldParameter, agent: string, t: TFunction): string {
  switch (entry.reason.why) {
    case 'agent': return t('composer.withheld.agent', { agent });
    case 'access': return t('composer.withheld.access');
    case 'placement': return t('composer.withheld.placement');
    case 'provider_kind': return t('composer.withheld.providerKind');
    case 'feature': return t('composer.withheld.feature');
    case 'provider': return t('composer.withheld.provider');
    case 'condition': return t('composer.withheld.condition');
  }
}

/** Whoever manages the thread may change how and where it runs, unless it is archived, a side thread or held by a job. */
const mayChangeRuns = (thread: Thread): boolean => holds(thread.access, 'thread.manage') && !thread.archived && thread.side_of === null && thread.lock === null;

/** The act that lifts the reason where the person may: the setting that has the feature off, or the change of how or where the thread runs. */
function WithheldAct({ thread, entry }: { thread: Thread; entry: WithheldParameter }) {
  const { t } = useTranslation();
  const feature = entry.reason.why === 'feature';
  // The nearest place that can hold the feature switch: the topic, else the project, else the org the thread lies in.
  const place = [...thread.path].reverse().find((node) => node.kind === 'topic' || node.kind === 'project' || node.kind === 'org');
  const node = useQuery({ ...nodeQuery(place?.id ?? ''), enabled: feature && place !== undefined });
  if (feature) {
    if (place === undefined || !holds(node.data?.access, 'settings.manage')) return null;
    const scope = place.kind === 'org' ? 'org' : place.kind === 'project' ? 'project' : 'topic';
    return <span className="text-sm" data-withheld-act="feature"><SourceSettingLink source={{ scope, node: place }} def={{ key: 'agent.features', class: 'policy' }}
      target={{ kind: 'node', id: place.id }} fallback={place.name ?? ''} text={t('composer.withheld.act.feature')} /></span>;
  }
  if (!mayChangeRuns(thread)) return null;
  if (entry.reason.why === 'access') {
    return <Button size="sm" variant="ghost" className="px-2 text-sm" data-withheld-act="access"
      onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' })}>{t('composer.withheld.act.access')}</Button>;
  }
  if (entry.reason.why === 'placement') {
    return <Button size="sm" variant="ghost" className="px-2 text-sm" data-withheld-act="placement" onClick={() => openWhere(thread)}>{t('composer.withheld.act.placement')}</Button>;
  }
  return null;
}

/** What stands above the list of an off choice: why, and what to do about it. */
export function WithheldNote({ thread, entry }: { thread: Thread; entry: WithheldParameter }) {
  const { t } = useTranslation();
  return <div className="flex flex-col items-start gap-1 px-2 text-sm" data-withheld={entry.choice}>
    <p className="text-text-muted">{withheldWords(entry, thread.binding?.agent.name ?? '', t)}</p>
    <WithheldAct thread={thread} entry={entry} />
  </div>;
}

/** The speeds of a model, all off: the ordinary one is in force. */
export const speedRows = (parameters: ModelParameters, standard: string): PickRow[] => [
  { id: NONE, name: standard, checked: true, disabled: true },
  ...(parameters.service_tiers ?? []).map((tier): PickRow => ({ id: tier.id, name: tier.label, disabled: true })),
];

/** The access programs of a model, all off: none is in force. */
export const programRows = (parameters: ModelParameters, none: string, t: TFunction): PickRow[] => [
  { id: NONE, name: none, checked: true, disabled: true },
  ...(parameters.access_programs ?? []).map((program): PickRow => ({ id: program, name: accessProgramDisplayName(program, t), disabled: true })),
];

/** The context sizes a model's managers opened, all off: the model's own window is in force. */
export function sizeRows(parameters: ModelParameters, locale: string): PickRow[] {
  const own = parameters.context_window ?? null;
  const sizes = [...new Set([parameters.context_max, own].filter((size): size is number => typeof size === 'number' && size > 0))].sort((one, other) => other - one);
  return sizes.map((size): PickRow => ({ id: String(size), name: formatContext(size, locale), checked: size === (own ?? sizes[0]), disabled: true }));
}

/** A speed or an access program that the thread cannot use, as the control it would be, dimmed. */
export function WithheldChip({ thread, entry, what, control, rows }: { thread: Thread; entry: WithheldParameter; what: string; control: 'speed' | 'access'; rows: readonly PickRow[] }) {
  const { t } = useTranslation();
  return <ChoiceChip what={what} value={rows.find((row) => row.checked === true)?.name ?? ''} control={control} className="opacity-60"
    reason={withheldWords(entry, thread.binding?.agent.name ?? '', t)}
    groups={[{ id: control, label: null, rows }]} above={<WithheldNote thread={thread} entry={entry} />} onChoose={() => undefined} />;
}
