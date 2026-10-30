/** What a row of a table and a stack of the chart are about: the member of a dimension that a key names, the words for it, the value that narrows the screen to it, and the key that is every other member together.
 *
 * Pure functions over the keys the API returns. A model is written by its display name; a place the
 * reader may not see is a line without a name, and is
 * counted all the same.
 */
import type { TFunction } from 'i18next';
import { accessProgramDisplayName, effortDisplayName, modelDisplayName } from '@/features/models/names';
import { programNameIn } from '@/features/resources/programNames';
import { i18n } from '@/i18n';
import { offeredChoice } from '@/i18n/choices';

import type { NodeRef, UsageDimension, UsageKey } from '@/generated/api-types';
import { FILTER_DIMENSION, type FilterName } from '@/features/usage/search';

/** Whether a key is the one that stands for every member beyond the largest: it names nothing in any dimension. */
export function isRest(key: UsageKey): boolean {
  return (
    key.person === undefined &&
    key.model === undefined &&
    key.provider === undefined &&
    key.agent === undefined &&
    key.environment === undefined &&
    key.effort === undefined &&
    key.tier === undefined &&
    key.program === undefined &&
    key.org === undefined &&
    key.project === undefined &&
    key.topic === undefined &&
    key.thread === undefined
  );
}

function nodeOf(dimension: UsageDimension, key: UsageKey): NodeRef | null | undefined {
  switch (dimension) {
    case 'org':
      return key.org;
    case 'project':
      return key.project;
    case 'topic':
      return key.topic;
    case 'thread':
      return key.thread;
    default:
      return undefined;
  }
}

/** What stays the same for a member whenever it is asked for: the colour of a stack, the key of a row. */
export function keyId(dimension: UsageDimension, key: UsageKey): string {
  if (isRest(key)) return 'rest';
  switch (dimension) {
    case 'person':
      return key.person?.id ?? 'nobody';
    case 'model':
      return key.model ?? 'unknown';
    case 'provider':
      return key.provider?.id ?? 'unknown';
    case 'agent':
      return key.agent?.kind ?? 'none';
    case 'environment':
      return key.environment ?? 'none';
    case 'effort':
      return key.effort ?? 'none';
    case 'tier':
      return key.tier ?? 'standard';
    case 'program':
      return key.program ?? 'none';
    default: {
      const node = nodeOf(dimension, key);
      return node === undefined ? 'unknown' : node === null ? 'direct' : node.redacted ? `hidden:${node.id}` : node.id;
    }
  }
}

/** The words for a member, as a row and a stack say them. */
export function keyLabel(dimension: UsageDimension, key: UsageKey, t: TFunction): string {
  if (isRest(key)) return t('usage.key.rest');
  switch (dimension) {
    case 'person':
      return key.person === null || key.person === undefined ? t('usage.key.nobody') : key.person.display_name;
    case 'model':
      return key.model === null || key.model === undefined ? t('usage.key.unknown') : modelDisplayName(key.model_display, t);
    case 'provider':
      return key.provider?.name ?? t('usage.key.unknown');
    case 'agent':
      return key.agent === null || key.agent === undefined ? t('usage.key.noAgent') : key.agent.name === key.agent.kind ? programNameIn(key.agent, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown') : key.agent.name;
    case 'environment':
      return key.environment === 'managed' ? t('usage.key.managed') : key.environment === 'connected' ? t('usage.key.connected') : t('usage.key.noEnvironment');
    case 'effort':
      return key.effort === null || key.effort === undefined ? t('usage.key.noEffort') : effortDisplayName(key.effort, t);
    case 'tier':
      return key.tier === null || key.tier === undefined ? t('usage.key.standard') : offeredChoice(t, 'serviceTier', key.tier)?.name ?? key.tier;
    case 'program':
      return key.program === null || key.program === undefined ? t('usagePerf.noProgram') : accessProgramDisplayName(key.program, t);
    default: {
      const node = nodeOf(dimension, key);
      if (node === undefined) return t('usage.key.unknown');
      if (node === null) return t('usage.key.direct');
      return node.redacted || node.name === null ? t('usage.key.hidden', { kind: t(`usage.key.kind.${node.kind}`) }) : node.name;
    }
  }
}

/** Only a distinct human program title adds information; a routing key is never a subtitle. */
export function keyDetail(dimension: UsageDimension, key: UsageKey, _t?: TFunction): string | null {
  if (isRest(key)) return null;
  if (dimension === 'agent' && key.agent !== null && key.agent !== undefined) {
    const title = programNameIn(key.agent, i18n.resolvedLanguage ?? i18n.language);
    return title === key.agent.name ? null : title;
  }
  return null;
}

/** The value the whole screen is narrowed to by this member, or nothing for a member that cannot be a filter (a place the reader cannot see, the rest). */
export function filterValue(dimension: UsageDimension, key: UsageKey): string | null {
  if (isRest(key)) return null;
  switch (dimension) {
    case 'person':
      return key.person?.id ?? null;
    case 'model':
      return key.model ?? null;
    case 'effort': return key.effort ?? 'none';
    case 'tier': return key.tier ?? 'none';
    case 'program': return key.program ?? 'none';
    case 'org': return key.org !== undefined && key.org !== null && !key.org.redacted ? key.org.id : null;
    case 'topic': return key.topic !== undefined && key.topic !== null && !key.topic.redacted ? key.topic.id : null;
    case 'provider':
      return key.provider?.id ?? null;
    case 'agent':
      return key.agent?.kind ?? null;
    case 'environment':
      return key.environment ?? null;
    case 'project':
      return key.project !== undefined && key.project !== null && !key.project.redacted ? key.project.id : null;
    case 'thread':
      return key.thread !== undefined && key.thread !== null && !key.thread.redacted ? key.thread.id : null;
    default:
      return null;
  }
}

/** Whether a table of this dimension has rows that narrow the screen. */
export function narrows(dimension: UsageDimension): boolean {
  return Object.values(FILTER_DIMENSION).includes(dimension);
}

/** The words for a filter that is set: "Model: GPT-5.4". The label of the value is found among the rows of the table it narrows. */
export function chipLabel(name: FilterName, label: string, t: TFunction): string {
  return t('usage.chip.label', { what: t(`usage.chip.${name}`), value: label });
}
