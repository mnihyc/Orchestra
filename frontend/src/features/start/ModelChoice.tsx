/** What a thread that is about to start runs with, chosen with the controls of the box of a thread: the models of every provider the person may use, in groups under the providers' names (so that a model is the choice of its provider), the effort of the chosen model, and the agent that works with that provider.
 *
 * A change of model to one of another provider keeps the agent where it still works with that provider and otherwise
 * takes the first that does, and one line says so. A list of more than eight rows has search, and its own scroll.
 */
import { useTranslation } from 'react-i18next';

import type { StartOptions } from '@/api/start';
import { type AgentChoice, AgentChip, agentAfterProvider, agentChangedWords, agentChoicesOf, type ProviderChoice } from '@/features/composer/BindingLists';
import { effortGroups, optionName } from '@/features/composer/modelRows';
import { ChoiceChip } from '@/features/composer/PickList';
import { ProviderModelChip, providerModelsOf } from '@/features/composer/ProviderModelChip';
import { effortDisplayName } from '@/features/models/names';
import type { ModelOption } from '@/generated/api-types';

import type { Picks, Resolved } from './choices';

/** The agents of one provider as the dialog may start them: what the server lists for it, those that cannot be started marked. */
function agentsOf(options: StartOptions, resolved: Resolved, providerId: string): AgentChoice[] {
  return agentChoicesOf(options, providerId).map((choice) => {
    const row = resolved.agents.find((one) => one.id === choice.id && resolved.provider?.id === providerId);
    return row === undefined || !choice.fits ? choice : { ...choice, usable: row.startable };
  });
}

export function StartModelChoice({ options, resolved, names, disabled = false, onPick }: {
  options: StartOptions;
  resolved: Resolved;
  /** The providers' names as the person's lists show them. */
  names: ReadonlyMap<string, string>;
  disabled?: boolean;
  /** What the choice changes of the picks, and the line to say where it changed the agent. */
  onPick: (patch: Partial<Picks>, notice: string | null) => void;
}) {
  const { t } = useTranslation();
  const providers = providerModelsOf(options).map((entry) => ({ ...entry, provider: { ...entry.provider, name: names.get(entry.provider.id) ?? entry.provider.name } }));
  const model = resolved.model;
  const chooseModel = (provider: ProviderChoice, option: ModelOption, close: () => void) => {
    if (provider.id === resolved.provider?.id) onPick({ model: option.model_name, effort: null }, null);
    else {
      const was = resolved.agent?.id ?? '';
      const after = agentAfterProvider(agentChoicesOf(options, provider.id), was);
      onPick({ provider: provider.id, agent: after !== null && !after.changed ? after.agent.id : null, model: option.model_name, effort: null, arrangement: null },
        after !== null && after.changed && resolved.agent !== null ? agentChangedWords(after.agent, resolved.agent.entry.agent.name, provider.name, t) : null);
    }
    close();
  };
  return <>
    <ProviderModelChip what={t('start.model.model')} searchLabel={t('composer.model.find')} providers={providers}
      current={{ provider: resolved.provider?.id ?? '', model: model?.model_name ?? null }} name={model === null ? '' : optionName(model)} disabled={disabled} onModel={chooseModel} />
    {model !== null && resolved.efforts.length > 0 && <ChoiceChip what={t('composer.effort.title')} value={resolved.effort === null ? '' : effortDisplayName(resolved.effort, t)} control="effort" disabled={disabled}
      groups={effortGroups(model, resolved.effort, t)} onChoose={(row, close) => { onPick({ effort: row.id }, null); close(); }} />}
  </>;
}

/** The agent of a thread that is about to start: those that work with the provider of the chosen model, the rest greyed. */
export function StartAgentChoice({ options, resolved, names, disabled = false, onPick }: {
  options: StartOptions;
  resolved: Resolved;
  names: ReadonlyMap<string, string>;
  disabled?: boolean;
  onPick: (agent: string) => void;
}) {
  const provider = resolved.provider;
  if (provider === null) return null;
  return <AgentChip choices={agentsOf(options, resolved, provider.id)} value={resolved.agent?.id ?? ''} name={resolved.agent?.entry.agent.name ?? ''}
    provider={names.get(provider.id) ?? provider.entry.provider.name} disabled={disabled} onChoose={(choice) => onPick(choice.id)} />;
}
