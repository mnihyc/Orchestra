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
import { Input } from '@/ui/Input';
import { FactDisclosureBody } from '@/ui/FactDisclosure';
import { offeredChoice } from '@/i18n/choices';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import type { ApprovalDefaultResolution, ModelOption } from '@/generated/api-types';

import { type Picks, type Resolved, poolDefaultLock } from './choices';

/** The agents of one provider as the dialog may start them: what the server lists for it, those that cannot be started marked. */
function agentsOf(options: StartOptions, resolved: Resolved, providerId: string): AgentChoice[] {
  return agentChoicesOf(options, providerId).map((choice) => {
    const row = resolved.agents.find((one) => one.id === choice.id && resolved.provider?.id === providerId);
    return row === undefined || !choice.fits ? choice : { ...choice, usable: row.startable };
  });
}

export function StartModelChoice({ options, resolved, names, allowPools = false, disabled = false, onPick }: {
  options: StartOptions;
  resolved: Resolved;
  /** The providers' names as the person's lists show them. */
  names: ReadonlyMap<string, string>;
  allowPools?: boolean;
  disabled?: boolean;
  /** What the choice changes of the picks, and the line to say where it changed the agent. */
  onPick: (patch: Partial<Picks>, notice: string | null) => void;
}) {
  const { t } = useTranslation();
  const providers = providerModelsOf(options).map((entry) => ({ ...entry, provider: { ...entry.provider, name: names.get(entry.provider.id) ?? entry.provider.name } }));
  const model = resolved.model;
  const selectedPool = resolved.bindingPool;
  const defaults = options.defaults;
  const unavailable = (provider: ProviderChoice, option?: ModelOption): string | undefined => {
    if (defaults.locks.provider && provider.id !== defaults.provider_id) return t('start.provider.locked');
    const agents = agentChoicesOf(options, provider.id);
    if (defaults.locks.agent && defaults.agent_id !== null && agents.length > 0 && !agents.some((agent) => agent.id === defaults.agent_id && agent.fits && agent.usable)) return t('start.locks.agent');
    if (option !== undefined && defaults.locks.model && option.model_name !== defaults.model_name) return t('start.locks.model');
    if (option !== undefined && defaults.locks.effort && defaults.effort !== null && !(option.parameters.efforts ?? []).includes(defaults.effort)) return t('start.locks.effortValue', { effort: effortDisplayName(defaults.effort, t) });
    return undefined;
  };
  const chooseModel = (provider: ProviderChoice, option: ModelOption, close: () => void) => {
    if (provider.id === resolved.provider?.id) onPick({ bindingPool: null, ...(defaults.locks.model ? {} : { model: option.model_name }), ...(defaults.locks.effort ? {} : { effort: null }) }, null);
    else {
      const was = resolved.agent?.id ?? '';
      const after = agentAfterProvider(agentChoicesOf(options, provider.id), was);
      onPick({ bindingPool: null, provider: provider.id, ...(defaults.locks.agent ? {} : { agent: after !== null && !after.changed ? after.agent.id : null }), ...(defaults.locks.model ? {} : { model: option.model_name }), ...(defaults.locks.effort ? {} : { effort: null }), arrangement: null },
        after !== null && after.changed && resolved.agent !== null ? agentChangedWords(after.agent, resolved.agent.entry.agent.name, provider.name, t) : null);
    }
    close();
  };
  return <>
    <ProviderModelChip what={t('start.model.model')} searchLabel={t('composer.model.find')} providers={providers}
      current={{ provider: resolved.provider?.id ?? '', model: resolved.bindingPool === undefined ? model?.model_name ?? null : null }}
      name={resolved.bindingPool?.name ?? (model === null ? '' : optionName(model))} selectedPool={resolved.bindingPool?.id}
      poolUnavailable={(pool) => {
        const lock = poolDefaultLock(defaults, pool);
        return lock === 'provider' ? t('start.provider.locked') : lock === 'model' ? t('start.locks.model') : undefined;
      }}
      onPool={!allowPools ? undefined : (pool) => {
        if (pool.kind === 'model' || pool.kind === 'provider') onPick({
          bindingPool: pool.kind === 'model' ? { id: pool.id, kind: 'model', name: pool.name }
            : { id: pool.id, kind: 'provider', name: pool.name, model: selectedPool?.kind === 'provider' ? selectedPool.model : model?.model_name ?? '' },
          agent: resolved.agent?.id ?? null,
        }, null);
      }} disabled={disabled || defaults.locks.provider && defaults.locks.model}
      reason={defaults.locks.provider && defaults.locks.model ? t('start.locks.model') : undefined} unavailable={unavailable} onModel={chooseModel} />
    {selectedPool?.kind === 'provider' && <Input hideLabel label={t('start.model.model')} placeholder={t('start.model.model')}
      disabled={disabled || defaults.locks.model} value={selectedPool.model}
      onChange={(event) => onPick({bindingPool:{...selectedPool, model:event.target.value}}, null)} />}
    {resolved.bindingPool?.kind !== 'model' && model !== null && resolved.efforts.length > 0 && <ChoiceChip what={t('composer.effort.title')} value={resolved.effort === null ? '' : effortDisplayName(resolved.effort, t)} control="effort" disabled={disabled || defaults.locks.effort} reason={defaults.locks.effort ? t('start.locks.effort') : undefined}
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
  const { t } = useTranslation();
  const provider = resolved.provider;
  const approval = resolved.agent?.entry.approval_resolution;
  const facts = <ApprovalDefaultFacts approval={approval} />;
  if (provider === null) return null;
  return <AgentChip choices={agentsOf(options, resolved, provider.id)} value={resolved.agent?.id ?? ''} name={resolved.agent?.entry.agent.name ?? ''}
    facts={facts} fragment={approval?.reason === undefined || approval.reason === null ? undefined : <span className="text-sm text-text-muted">{t('start.approvalFacts.replaced', { mode: approval.effective === null ? t('factDisclosure.notReported') : offeredChoice(t, 'approvalMode', approval.effective)?.name ?? approval.effective })}</span>}
    provider={names.get(provider.id) ?? provider.entry.provider.name} disabled={disabled || options.defaults.locks.agent} reason={options.defaults.locks.agent ? t('start.locks.agent') : undefined} onChoose={(choice) => onPick(choice.id)} />;
}

export function ApprovalDefaultFacts({ approval }: { approval?: ApprovalDefaultResolution | null }) {
  const { t } = useTranslation();
  const name = (value: string) => offeredChoice(t, 'approvalMode', value)?.name ?? value;
  return <FactDisclosureBody groups={[{ key: 'approval-default', kind: 'current_setting', facts: [
    { key: 'preferred', label: t('start.approvalFacts.preferred'), value: approval === undefined || approval === null ? undefined : name(approval.preferred) },
    { key: 'effective', label: t('start.approvalFacts.effective'), value: approval?.effective === undefined || approval.effective === null ? undefined : name(approval.effective) },
    ...(approval?.reason === undefined || approval.reason === null ? [] : [{ key: 'reason', label: t('start.approvalFacts.reason'), value: t(approval.reason === 'not_declared' ? 'start.approvalFacts.notDeclared' : 'start.approvalFacts.noOrdinary') }]),
    { key: 'source', label: t('start.approvalFacts.source'), value: approval === undefined || approval === null ? undefined : <SourceSettingLink source={approval.source} def={{ key: 'approval.mode', class: 'policy' }} fallback={t('factDisclosure.notReported')} /> },
  ] }]} />;
}
