/** The list of models, in groups under the name of the provider each runs on, so that choosing a model chooses its provider.
 *
 * A provider is no control of its own: none of the products puts the account in the box, and the one that has
 * several shows it as a heading in its list of models. Here the heading is the provider's name (and, where two
 * share a name, its place after it, with a small word where it is not ready). Under each heading stand its models, one row each.
 * The providers the person cannot use come last, in two groups: those to ask for (choosing one opens the request for its use,
 * in the list itself) and those that are unavailable, greyed, with the reason in a few words. Search stands
 * above eight rows, and the list has its own scroll.
 */
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { AccessRequest } from '@/features/access/AccessRequest';
import { type ProviderChoice, providerAccess, providerChoicesOf, providerWhy, missingUse } from '@/features/composer/BindingLists';
import { modelDetail, optionName } from '@/features/composer/modelRows';
import { ChoiceChip, type PickGroup, type PickRow } from '@/features/composer/PickList';
import type { BindingOptions, ModelOption } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';

export interface ProviderModels {
  provider: ProviderChoice;
  models: readonly ModelOption[];
}

/** The providers of an answer to what a thread could be bound to, each with the models it lists. */
export function providerModelsOf(options: Pick<BindingOptions, 'providers'>): ProviderModels[] {
  const choices = providerChoicesOf(options);
  return options.providers.map((entry, at) => ({ provider: choices[at] as ProviderChoice, models: entry.models }));
}

const fold = (text: string): string => text.normalize('NFKC').toLocaleLowerCase();
const modelId = (provider: string, model: string): string => JSON.stringify(['model', provider, model]);
const askId = (provider: string): string => JSON.stringify(['ask', provider]);

export interface ProviderModelChipProps {
  providers: readonly ProviderModels[];
  /** What runs now: the provider, and the model where there is one. */
  current: { provider: string; model: string | null };
  /** The name of the model that is chosen now: the only words on the button. */
  name: string;
  /** A model of the provider's list was pressed; `close` shuts the list. */
  onModel: (provider: ProviderChoice, option: ModelOption, close: () => void) => void;
  disabled?: boolean;
  reason?: string | undefined;
  /** The models cannot be chosen now: they are shown, off. */
  locked?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  above?: ReactNode;
  below?: ReactNode;
  /** The providers are being read. */
  pending?: boolean;
  /** What the control is called; the model's. */
  what: string;
  searchLabel: string;
}

/** The model list of a thread, a dialog or a switch, grouped under the providers' names. */
export function ProviderModelChip({ providers, current, name, onModel, disabled, reason, locked = false, open, onOpenChange, above, below, pending = false, what, searchLabel }: ProviderModelChipProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [asking, setAsking] = useState<ProviderChoice | null>(null);
  const shared = (choice: ProviderChoice) => providers.some((other) => other.provider.id !== choice.id && fold(other.provider.name) === fold(choice.name));
  const heading = (choice: ProviderChoice): string => [choice.name, shared(choice) ? choice.address : '', choice.status === 'ready' ? '' : t(`composer.pick.provider.why.${choice.status}`)]
    .filter((part) => part !== '').join(t('startFlow.separator'));
  const using = providers.filter((entry) => providerAccess(entry.provider) === 'use');
  const groups: PickGroup[] = using.map((entry) => ({
    id: `provider:${entry.provider.id}`,
    label: heading(entry.provider),
    rows: entry.models.map((option): PickRow => ({ id: modelId(entry.provider.id, option.model_name), name: optionName(option), keywords: `${option.model_name} ${entry.provider.name}`,
      checked: entry.provider.id === current.provider && option.model_name === current.model, disabled: locked || !option.allowed,
      detail: option.allowed ? modelDetail(option, '', locale, t) : t(`composer.model.reason.${option.reason ?? 'disabled'}`, { provider: entry.provider.name }) })),
  }));
  const aside = (access: 'ask' | 'off'): PickRow[] => providers.filter((entry) => providerAccess(entry.provider) === access).map((entry): PickRow => ({
    id: askId(entry.provider.id), name: entry.provider.name, disabled: access === 'off', keywords: entry.provider.address,
    place: [shared(entry.provider) ? entry.provider.address : '', access === 'off' ? providerWhy(entry.provider, t) : ''].filter((part) => part !== '').join(t('startFlow.separator')),
  }));
  groups.push({ id: 'ask', label: t('composer.pick.provider.ask'), always: true, rows: aside('ask') }, { id: 'off', label: t('composer.pick.provider.off'), always: true, rows: aside('off') });
  return <ChoiceChip what={what} value={name} control="model" disabled={disabled === true} reason={reason} title={name} searchLabel={searchLabel}
    {...(open === undefined ? {} : { open })} {...(onOpenChange === undefined ? {} : { onOpenChange })}
    groups={groups} above={above}
    below={<>
      {asking !== null && <AccessRequest key={asking.id} fixed peopleAsChips objectId={asking.id} rememberAs={`${asking.id}.provider.use`} missing={[missingUse(asking)]} />}
      {below}
    </>}
    empty={<p role="status" className="px-2 text-sm text-text-muted">{pending ? t('common.loading') : t('composer.model.none')}</p>}
    onChoose={(row, close) => {
      const parts: unknown = JSON.parse(row.id);
      if (!Array.isArray(parts)) return;
      const provider = providers.find((entry) => entry.provider.id === parts[1]);
      if (provider === undefined) return;
      if (parts[0] === 'ask') { setAsking(provider.provider); return; }
      const option = provider.models.find((candidate) => candidate.model_name === parts[2]);
      if (option === undefined || locked || !option.allowed) return;
      setAsking(null);
      onModel(provider.provider, option, close);
    }} />;
}
