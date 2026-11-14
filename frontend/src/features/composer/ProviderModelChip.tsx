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
  return options.providers.flatMap((entry, at) => { const provider = choices[at]; return provider === undefined ? [] : [{ provider, models: entry.models }]; });
}

const fold = (text: string): string => text.normalize('NFKC').toLocaleLowerCase();
const modelId = (provider: string, model: string): string => JSON.stringify(['model', provider, model]);
const askId = (provider: string): string => JSON.stringify(['ask', provider]);
const askModelId = (provider: string, model: string): string => JSON.stringify(['ask-model', provider, model]);
const asksForModel = (entry: ProviderModels, option: ModelOption): boolean => entry.provider.owner !== null && !option.allowed && option.reason === 'excluded_by_entry';

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
  /** A failed read has its own alert beside the control, rather than an empty-list claim. */
  failed?: boolean;
  /** What the control is called; the model's. */
  what: string;
  searchLabel: string;
}

/** The model list of a thread, a dialog or a switch, grouped under the providers' names. */
export function ProviderModelChip({ providers, current, name, onModel, disabled, reason, locked = false, open, onOpenChange, above, below, pending = false, failed = false, what, searchLabel }: ProviderModelChipProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [asking, setAsking] = useState<{ provider: ProviderChoice; model?: string } | null>(null);
  const shared = (choice: ProviderChoice) => providers.some((other) => other.provider.id !== choice.id && fold(other.provider.name) === fold(choice.name));
  const heading = (choice: ProviderChoice): string => [choice.name, shared(choice) ? choice.address : '', choice.status === 'ready' ? '' : t(`composer.pick.provider.why.${choice.status}`)]
    .filter((part) => part !== '').join(t('startFlow.separator'));
  const using = providers.filter((entry) => providerAccess(entry.provider) === 'use');
  const groups: PickGroup[] = using.map((entry) => ({
    id: `provider:${entry.provider.id}`,
    label: heading(entry.provider),
    rows: entry.models.filter((option) => option.allowed).map((option): PickRow => ({ id: modelId(entry.provider.id, option.model_name), name: optionName(option), keywords: `${option.model_name} ${entry.provider.name}`,
      checked: entry.provider.id === current.provider && option.model_name === current.model, disabled: locked,
      detail: modelDetail(option, '', locale, t) })),
  }));
  const aside = (access: 'ask' | 'off'): PickRow[] => providers.filter((entry) => providerAccess(entry.provider) === access).map((entry): PickRow => ({
    id: askId(entry.provider.id), name: entry.provider.name, disabled: access === 'off', keywords: entry.provider.address,
    place: [shared(entry.provider) ? entry.provider.address : '', access === 'off' ? providerWhy(entry.provider, t) : ''].filter((part) => part !== '').join(t('startFlow.separator')),
  }));
  const modelsToAsk = using.flatMap((entry) => entry.models.filter((option) => asksForModel(entry, option)).map((option): PickRow => ({
    id: askModelId(entry.provider.id, option.model_name), name: optionName(option), place: entry.provider.name,
    checked: entry.provider.id === current.provider && option.model_name === current.model,
    keywords: `${option.model_name} ${entry.provider.address}`, disabled: locked, ...(locked && reason !== undefined ? { detail: reason } : {}),
  })));
  const modelsOff = using.flatMap((entry) => entry.models.filter((option) => !option.allowed && !asksForModel(entry, option)).map((option): PickRow => ({
    id: modelId(entry.provider.id, option.model_name), name: optionName(option), disabled: true,
    checked: entry.provider.id === current.provider && option.model_name === current.model,
    keywords: `${option.model_name} ${entry.provider.name}`, detail: `${entry.provider.name}${t('startFlow.separator')}${t(`composer.model.reason.${option.reason ?? 'disabled'}`, { provider: entry.provider.name })}`,
  })));
  groups.push({ id: 'ask', label: t('composer.pick.provider.ask'), always: true, rows: [...aside('ask'), ...modelsToAsk] }, { id: 'off', label: t('composer.pick.provider.off'), always: true, rows: [...aside('off'), ...modelsOff] });
  return <ChoiceChip what={what} value={name} control="model" disabled={disabled === true} reason={reason} title={name} searchLabel={searchLabel}
    {...(open === undefined ? {} : { open })} {...(onOpenChange === undefined ? {} : { onOpenChange })}
    groups={groups} above={above}
    below={<>
      {asking !== null && <AccessRequest key={JSON.stringify([asking.provider.id, asking.model])} fixed peopleAsChips objectId={asking.provider.id}
        rememberAs={`${asking.provider.id}.provider.use${asking.model === undefined ? '' : `.${asking.model}`}`} missing={[missingUse(asking.provider, asking.model)]} />}
      {below}
    </>}
    empty={failed ? null : <p role="status" className="px-2 text-sm text-text-muted">{pending ? t('common.loading') : t('composer.model.none')}</p>}
    onChoose={(row, close) => {
      const parts: unknown = JSON.parse(row.id);
      if (!Array.isArray(parts)) return;
      const provider = providers.find((entry) => entry.provider.id === parts[1]);
      if (provider === undefined) return;
      if (parts[0] === 'ask') { setAsking({ provider: provider.provider }); return; }
      const option = provider.models.find((candidate) => candidate.model_name === parts[2]);
      if (parts[0] === 'ask-model') {
        if (option !== undefined && !locked && asksForModel(provider, option)) setAsking({ provider: provider.provider, model: option.model_name });
        return;
      }
      if (option === undefined || locked || !option.allowed) return;
      setAsking(null);
      onModel(provider.provider, option, close);
    }} />;
}
