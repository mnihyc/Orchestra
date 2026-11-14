/** The agent features: a switch for each feature that a parameter of an installed agent program names, with that parameter's title and what it opens in a few words.
 *
 * A parameter that carries a feature is offered in a thread only where the feature is on, so the switch is where a person finds out why a
 * parameter of a program is missing, and where it is switched. What each feature starts as is the registry's own default (and what places above set), never
 * this page's. A stored feature that no installed program names is kept visible with an act that removes it, so that nothing the page does not
 * explain stays in the value.
 */
import { RotateCcw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { HarnessKindInfo } from '@/generated/api-types';
import { readDescriptor } from '@/features/resources/agents/descriptor';
import { useHarnessNames } from '@/features/resources/harnesses';
import { isRecord } from '@/features/settings/draftChecks';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';

export const AGENT_FEATURES_SETTING = 'agent.features';

/** One feature that a parameter of an installed program names. */
export interface FeatureOffer {
  feature: string;
  /** The program or programs and the title of the parameter or parameters that carry it. */
  name: string;
  /** What it opens, in the first sentence of what the parameter says of itself. */
  says: string;
}

/** The first sentence of a text: what a parameter does, in a few words. */
export function firstSentence(text: string): string {
  const end = text.search(/[.!?](\s|$)/u);
  return (end === -1 ? text : text.slice(0, end + 1)).trim();
}

/** Every feature that a parameter of any program names, each with the program and parameter that carry it. A program that has not reported what it declares names none. */
export function featureOffers(kinds: readonly HarnessKindInfo[], locale: string, program: (kind: string) => string): FeatureOffer[] {
  const found = new Map<string, { programs: string[]; titles: string[]; says: string }>();
  for (const kind of kinds) {
    for (const parameter of readDescriptor(kind.descriptor, locale).parameters) {
      if (parameter.feature === null) continue;
      const held = found.get(parameter.feature) ?? { programs: [], titles: [], says: '' };
      const name = program(kind.kind);
      if (!held.programs.includes(name)) held.programs.push(name);
      if (!held.titles.includes(parameter.title)) held.titles.push(parameter.title);
      if (held.says === '') held.says = firstSentence(parameter.description);
      found.set(parameter.feature, held);
    }
  }
  return [...found].map(([feature, held]) => ({ feature, name: `${held.programs.join(', ')}: ${held.titles.join(', ')}`, says: held.says }))
    .sort((first, second) => first.name.localeCompare(second.name));
}

/** The features the installed programs name, read from the same list that names the programs. */
function useFeatureOffers() {
  const { i18n } = useTranslation();
  const { kinds, title } = useHarnessNames();
  const offers = kinds.data === undefined ? [] : featureOffers(kinds.data, i18n.resolvedLanguage ?? 'en', title);
  return { kinds, offers };
}

export interface AgentFeaturesChoiceProps extends ValueControlProps {
  /** The setting's default in the registry: what each feature is while no place says. */
  defaults: unknown;
  /** The features that places above set, which a feature without an entry here follows. */
  inherited: unknown;
}

export function AgentFeaturesChoice({ value, onChange, onEdited, disabled = false, defaults, inherited }: AgentFeaturesChoiceProps) {
  const { t } = useTranslation();
  const { kinds, offers } = useFeatureOffers();
  const own = isRecord(value) ? value : {};
  const above: Record<string, unknown> = { ...(isRecord(defaults) ? defaults : {}), ...(isRecord(inherited) ? inherited : {}) };
  const write = (next: Record<string, unknown>) => { onEdited?.(); onChange(next); };
  // A feature that is as it already stands above needs no entry here: only a difference is written.
  const set = (feature: string, on: boolean) => {
    const { [feature]: _before, ...rest } = own;
    write(on === (above[feature] === true) ? rest : { ...rest, [feature]: on });
  };
  if (kinds.isPending) return <SkeletonRows read={kinds} rows={2} />;
  if (kinds.isError) return <ErrorState compact error={kinds.error} retrying={kinds.isRefetching} onRetry={() => void kinds.refetch()} />;
  const strays = Object.keys(own).filter((key) => !offers.some((offer) => offer.feature === key));
  if (offers.length === 0 && strays.length === 0) return <StateLine compact message={t('settingsEditors.features.none')} />;
  return <ul aria-label={t('settingsEditors.features.label')} className="flex min-w-0 flex-col divide-y divide-border rounded-md border border-border">
    {offers.map((offer) => {
      const stored = Object.hasOwn(own, offer.feature);
      return <li key={offer.feature} data-agent-feature={offer.feature} className="flex min-w-0 items-start gap-2 px-3 py-2">
        <Switch className="min-w-0 flex-1" label={offer.name} hint={offer.says === '' ? undefined : offer.says} disabled={disabled}
          checked={stored ? own[offer.feature] === true : above[offer.feature] === true} onCheckedChange={(next) => set(offer.feature, next)} />
        {stored && <IconButton size="sm" label={t('settingsEditors.features.useDefault', { name: offer.name })} icon={<RotateCcw aria-hidden />} disabled={disabled}
          onClick={() => { const { [offer.feature]: _gone, ...rest } = own; write(rest); }} />}
      </li>;
    })}
    {strays.map((key) => <li key={key} data-agent-feature-unknown={key} className="flex min-w-0 items-center justify-between gap-2 px-3 py-2">
      <span className="min-w-0 wrap-anywhere"><code>{key}</code>
        <span className="text-sm text-text-muted"> · {t(own[key] === true ? 'settingsEditors.features.on' : 'settingsEditors.features.off')} · {t('settingsEditors.features.unknown')}</span></span>
      <IconButton size="sm" label={t('settingsEditors.features.remove', { name: key })} icon={<X aria-hidden />} disabled={disabled}
        onClick={() => { const { [key]: _gone, ...rest } = own; write(rest); }} />
    </li>)}
  </ul>;
}

/** The features in a few words: each by the program and parameter that carry it, and whether it is on. */
export function AgentFeaturesSummary({ value }: { value: Record<string, unknown> }) {
  const { t, i18n } = useTranslation();
  const { offers } = useFeatureOffers();
  const entries = Object.entries(value).filter((entry): entry is [string, boolean] => typeof entry[1] === 'boolean');
  if (entries.length === 0) return <span>{t('settingsEditors.features.summaryNone')}</span>;
  const named = entries.map(([key, on]) => t('settingsEditors.features.summaryOne', { name: offers.find((offer) => offer.feature === key)?.name ?? key, state: t(on ? 'settingsEditors.features.on' : 'settingsEditors.features.off') }));
  return <span className="min-w-0 wrap-anywhere">{new Intl.ListFormat(i18n.resolvedLanguage ?? i18n.language, { style: 'long', type: 'conjunction' }).format(named)}</span>;
}
