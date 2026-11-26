/** The list of models, in groups under the name of the provider each runs on, so that choosing a model chooses its provider.
 *
 * A provider is no control of its own: none of the products puts the account in the box, and the one that has
 * several shows it as a heading in its list of models. Here the heading is the provider's name (and, where two
 * share a name, its place after it, with a small word where it is not ready). Under each heading stand its models, one row each.
 * The providers the person cannot use come last, in two groups: those to ask for (choosing one opens the request for its use,
 * in the list itself) and those that are unavailable, greyed, with the reason in a few words. Search stands
 * above eight rows, and the list has its own scroll.
 */
import { Link } from '@tanstack/react-router';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { AccessRequest } from '@/features/access/AccessRequest';
import { type ProviderChoice, providerAccess, providerChoicesOf, providerWhy, missingUse } from '@/features/composer/BindingLists';
import { modelDetail, optionName } from '@/features/composer/modelRows';
import { ChoiceChip, type PickGroup, type PickRow } from '@/features/composer/PickList';
import { usePoolTargetChoices } from '@/features/resources/pools/targetChoices';
import type { BindingOptions, ModelOption, Pool } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { modelPurpose, modelsForPurpose } from '@/features/models/purpose';
import { splitRecent, useModelHistory } from '@/features/models/history';
import type { ModelRecent } from '@/generated/api-types';
import { ReadFeedback } from '@/ui/ReadFeedback';

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
  onPool?: ((pool: Pool) => void) | undefined;
  selectedPool?: string | undefined;
  disabled?: boolean;
  reason?: string | undefined;
  /** The models cannot be chosen now: they are shown, off. */
  locked?: boolean;
  /** A surrounding setting can fix one part of a combined choice independently. */
  poolUnavailable?: (pool: Pool) => string | undefined;
  unavailable?: (provider: ProviderChoice, model?: ModelOption) => string | undefined;
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
export function ProviderModelChip({ providers, current, name, onModel, onPool, selectedPool, disabled, reason, locked = false, unavailable, poolUnavailable, open, onOpenChange, above, below, pending = false, failed = false, what, searchLabel }: ProviderModelChipProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [asking, setAsking] = useState<{ provider: ProviderChoice; model?: string } | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const expanded = open ?? historyOpen;
  const [search, setSearch] = useState('');
  const history = useModelHistory(expanded);
  const readPools = expanded && onPool !== undefined;
  const modelPools = usePoolTargetChoices('model', readPools, search);
  const providerPools = usePoolTargetChoices('provider', readPools, search);
  const shared = (choice: ProviderChoice) => providers.some((other) => other.provider.id !== choice.id && fold(other.provider.name) === fold(choice.name));
  const heading = (choice: ProviderChoice): string => [choice.name, shared(choice) ? choice.address : '', choice.status === 'ready' ? '' : t(`composer.pick.provider.why.${choice.status}`)]
    .filter((part) => part !== '').join(t('startFlow.separator'));
  const using = providers.filter((entry) => providerAccess(entry.provider) === 'use').map((entry) => ({ ...entry, models: modelsForPurpose(entry.models, 'generation') }));
  const groups: PickGroup[] = using.map((entry) => ({
    id: `provider:${entry.provider.id}`,
    label: heading(entry.provider),
    rows: entry.models.filter((option) => option.allowed).map((option): PickRow => ({ id: modelId(entry.provider.id, option.model_name), name: optionName(option), keywords: `${option.model_name} ${entry.provider.name}`,
      checked: entry.provider.id === current.provider && option.model_name === current.model, disabled: locked || unavailable?.(entry.provider, option) !== undefined,
      detail: unavailable?.(entry.provider, option) ?? modelDetail(option, '', locale, t) })),
  }));
  if (onPool !== undefined) {
    for (const [kind, list] of [['model', modelPools], ['provider', providerPools]] as const) {
      groups.push({ id: 'pools-' + kind, label: t('screens.pools.title') + t('startFlow.separator') + t('pools.kinds.' + kind),
        rows: list.rows.map((pool) => {
          const why = poolUnavailable?.(pool) ?? (!pool.enabled ? t('pools.off') : pool.policy === null ? t('pools.policy.choose') : undefined);
          return { id: JSON.stringify(['pool', pool.id]), name: pool.name, checked: pool.id === selectedPool,
            disabled: locked || why !== undefined, detail: why ?? (pool.policy && t('pools.policy.' + pool.policy.type)) ?? '' };
        }) });
    }
  }
  const identities = new Map<string, ModelRecent>();
  for (const entry of using) for (const option of entry.models) {
    if (option.allowed && unavailable?.(entry.provider, option) === undefined) identities.set(modelId(entry.provider.id, option.model_name), { provider_id: entry.provider.id, model_name: option.model_name });
  }
  const recent = splitRecent(groups.flatMap((group) => group.rows), history.history, (row) => identities.get(row.id) ?? null).recent;
  const promoted = new Set(recent.map((row) => row.id));
  for (const group of groups) group.rows = group.rows.filter((row) => !promoted.has(row.id));
  if (recent.length > 0) groups.unshift({ id: 'recent', label: t('modelPicker.recent'), always: true, rows: recent.map((row) => {
    const entry = providers.find((entry) => entry.provider.id === identities.get(row.id)?.provider_id);
    return { ...row, place: entry === undefined ? undefined : heading(entry.provider) };
  }) });
  const aside = (access: 'ask' | 'off'): PickRow[] => providers.filter((entry) => providerAccess(entry.provider) === access && (entry.models.length === 0 || modelsForPurpose(entry.models, 'generation').length > 0)).map((entry): PickRow => ({
    id: askId(entry.provider.id), name: entry.provider.name, disabled: access === 'off' || unavailable?.(entry.provider) !== undefined, keywords: entry.provider.address,
    action: entry.provider.setup?.reason === 'sign_in_lapsed' && entry.provider.setup.can_sign_in
      ? <Link className="self-start px-2 text-xs text-link hover:underline" to="/resources/providers/$id" params={{ id: entry.provider.id }} search={{ tab: 'key' }}>{t('resources.act.signInAgain')}</Link>
      : undefined,
    place: [shared(entry.provider) ? entry.provider.address : '', unavailable?.(entry.provider) ?? (access === 'off' || entry.provider.reason === 'shared_for_ask_only' ? providerWhy(entry.provider, t) : '')].filter((part) => part !== '').join(t('startFlow.separator')),
  }));
  const modelsToAsk = using.flatMap((entry) => entry.models.filter((option) => asksForModel(entry, option)).map((option): PickRow => ({
    id: askModelId(entry.provider.id, option.model_name), name: optionName(option), place: entry.provider.name,
    checked: entry.provider.id === current.provider && option.model_name === current.model,
    keywords: `${option.model_name} ${entry.provider.address}`, disabled: locked || unavailable?.(entry.provider, option) !== undefined,
    ...(unavailable?.(entry.provider, option) !== undefined ? { detail: unavailable(entry.provider, option) } : locked && reason !== undefined ? { detail: reason } : {}),
  })));
  const modelsOff = using.flatMap((entry) => entry.models.filter((option) => !option.allowed && !asksForModel(entry, option)).map((option): PickRow => ({
    id: modelId(entry.provider.id, option.model_name), name: optionName(option), disabled: true,
    checked: entry.provider.id === current.provider && option.model_name === current.model,
    keywords: `${option.model_name} ${entry.provider.name}`, detail: `${entry.provider.name}${t('startFlow.separator')}${t(`composer.model.reason.${option.reason ?? 'disabled'}`, { provider: entry.provider.name })}`,
  })));
  const represented = [...groups.flatMap((group) => group.rows), ...modelsToAsk, ...modelsOff].some((row) => row.checked === true);
  if (!represented && current.model !== null && name !== '') {
    const entry = providers.find((entry) => entry.provider.id === current.provider);
    const provider = entry?.provider;
    const held = entry?.models.find((option) => option.model_name === current.model);
    const usable = provider !== undefined && providerAccess(provider) === 'use';
    const why = provider !== undefined && !usable ? providerWhy(provider, t) : held !== undefined && modelPurpose(held, 'generation') === 'unsupported' ? t('modelPicker.notForWork') : pending ? t('common.loading') : t('providerOptions.unavailable');
    const kept: PickRow = { id: modelId(current.provider, current.model), name, checked: true, disabled: true,
      keywords: current.model, detail: provider === undefined ? why : `${provider.name}${t('startFlow.separator')}${why}` };
    const loading = pending && usable ? groups.find((group) => group.id === `provider:${provider.id}`) : undefined;
    if (loading !== undefined) loading.rows = [...loading.rows, kept];
    else modelsOff.push(kept);
  }
  groups.push({ id: 'ask', label: t('composer.pick.provider.ask'), always: true, rows: [...aside('ask'), ...modelsToAsk] }, { id: 'off', label: t('composer.pick.provider.off'), always: true, rows: [...aside('off'), ...modelsOff] });
  return <ChoiceChip what={what} value={name} control="model" disabled={disabled === true} reason={reason} title={name} searchLabel={searchLabel}
    {...(open === undefined ? {} : { open })} onOpenChange={(next) => { setHistoryOpen(next); if (!next) setSearch(''); onOpenChange?.(next); }}
    groups={groups} above={above} onSearchChange={onPool === undefined ? undefined : setSearch}
    below={<>
      <ReadFeedback reads={[history.read]} pending={history.read.isFetching && history.history === undefined} />
      {readPools && <>{modelPools.footer}{providerPools.footer}</>}
      {asking !== null && <AccessRequest key={JSON.stringify([asking.provider.id, asking.model])} fixed peopleAsChips objectId={asking.provider.id}
        rememberAs={`${asking.provider.id}.provider.use${asking.model === undefined ? '' : `.${asking.model}`}`} missing={[missingUse(asking.provider, asking.model)]} />}
      {below}
    </>}
    empty={failed ? null : <p role="status" className="px-2 text-sm text-text-muted">{pending ? t('common.loading') : t(providers.some((entry) => entry.models.length > 0) ? 'modelPicker.noSuitableModels' : 'composer.model.none')}</p>}
    onChoose={(row, close) => {
      const parts: unknown = JSON.parse(row.id);
      if (!Array.isArray(parts)) return;
      if (parts[0] === 'pool') {
        const pool = [...modelPools.rows, ...providerPools.rows].find((candidate) => candidate.id === parts[1]);
        if (pool !== undefined && pool.enabled && pool.policy !== null && poolUnavailable?.(pool) === undefined && !locked && onPool !== undefined) {
          setAsking(null); onPool(pool); close();
        }
        return;
      }
      const provider = providers.find((entry) => entry.provider.id === parts[1]);
      if (provider === undefined) return;
      if (parts[0] === 'ask') { if (unavailable?.(provider.provider) !== undefined) return; setAsking({ provider: provider.provider }); return; }
      const option = provider.models.find((candidate) => candidate.model_name === parts[2]);
      if (parts[0] === 'ask-model') {
        if (option !== undefined && !locked && unavailable?.(provider.provider, option) === undefined && asksForModel(provider, option)) setAsking({ provider: provider.provider, model: option.model_name });
        return;
      }
      if (option === undefined || modelPurpose(option, 'generation') === 'unsupported' || locked || !option.allowed || unavailable?.(provider.provider, option) !== undefined) return;
      setAsking(null);
      history.remember({ provider_id: provider.provider.id, model_name: option.model_name });
      onModel(provider.provider, option, close);
    }} />;
}
