import { ResourcePicker } from '@/ui/ResourcePicker';
/** Provider, profile, model and effort the result runs on.
 *
 * What a thread runs on, chosen as the dialog that starts a thread chooses it, from the lists the person may use:
 * the providers that are ready and theirs first (the one the thread is on now marked), the others below with the
 * reason; the agent profile the provider offers; the model and its effort; and how agent and provider are placed.
 * A model and an effort alone are settings of the thread and need no change of what it runs on, so they are
 * offered only together with another provider or profile, where they have to be chosen again.
 */
import { useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { agentsQuery, providerModelsQuery, providersQuery } from '@/api/resources';
import type { Agent, Provider, ProviderModel, Thread } from '@/generated/api-types';
import { ProviderArrangementChoice } from '@/features/start/Arrangement';
import { ResourceHint, resourcePreview, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { providerLabels } from '@/features/resources/providers/labels';
import { ResourceCreationActions } from '@/features/resources/ResourceCreation';
import type { AutomaticRuns, Choices } from '@/features/change/choices';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { useFocusFirst } from '@/features/change/Sentence';
import { ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';

/** Whether the harness of a profile has said what it is: until it has, no thread can be placed on it. */
export const hasReported = (agent: Agent): boolean => {
  const descriptor: unknown = agent.descriptor;
  return typeof descriptor === 'object' && descriptor !== null && Object.keys(descriptor).length > 0;
};

/** What the thread runs on after the choices: the same as now wherever nothing was chosen. */
export interface Runs {
  providerId: string;
  agentId: string;
  model: string | null;
  effort: string | null;
}

export function runsOf(choices: Choices, thread: Thread): Runs {
  return {
    providerId: choices.providerId ?? thread.binding?.provider.id ?? '',
    agentId: choices.agentId ?? thread.binding?.agent.id ?? '',
    model: choices.model ?? thread.model,
    effort: choices.effort === undefined ? thread.effort : choices.effort,
  };
}

/** Whether what the thread runs on is another provider or profile than now: the model and effort are then chosen again. */
export const movesRuns = (choices: Choices): boolean => choices.providerId !== null || choices.agentId !== null;

/**
 * The agent profile that goes with another provider: the thread's own where the provider offers it and its harness has
 * reported what it is, else the first that can be placed; `null` keeps the thread's own.
 */
export function agentFor(provider: Provider | undefined, agents: readonly Agent[], thread: Thread): string | null {
  const chosen = chosenProfileFor(provider, agents, thread);
  return chosen === undefined || chosen.id === thread.binding?.agent.id ? null : chosen.id;
}

function chosenProfileFor(provider: Provider | undefined, agents: readonly Agent[], thread: Thread): Agent | undefined {
  const profiles = profilesFor(provider, agents).filter(hasReported);
  return profiles.find((agent) => agent.id === thread.binding?.agent.id) ?? profiles[0];
}

/** Facts about the profile default, recorded when the selector actually applies it. */
export function automaticProfileFor(provider: Provider | undefined, agents: readonly Agent[], thread: Thread): AutomaticRuns {
  const previous = agents.find((agent) => agent.id === thread.binding?.agent.id);
  const chosen = chosenProfileFor(provider, agents, thread);
  if (provider === undefined || chosen === undefined) return {};
  const keeper = provider.protocols.length === 0 && provider.agent?.id === chosen.id;
  return { profile: { providerId: provider.id, agentId: chosen.id,
    by: keeper ? 'keeper' : chosen.id === thread.binding?.agent.id ? 'retained' : 'first',
    previous: chosen.id === thread.binding?.agent.id ? null : previous?.disabled === true ? 'disabled' : previous !== undefined && !hasReported(previous) ? 'unreported' : keeper ? 'keeper' : 'unavailable' } };
}

/** Refused readable profiles stay in the picker; this does not replace native compatibility review. */
function profileWhy(provider: Provider | undefined, agent: Agent, t: TFunction): string | null {
  if (agent.disabled) return t('change.runs.unavailable.profileDisabled');
  if (!hasReported(agent)) return t('start.agent.unreported');
  const keeper = provider?.protocols.length === 0 ? provider.agent?.id : undefined;
  if (keeper !== undefined && agent.id !== keeper) return t('change.runs.unavailable.keeper');
  return null;
}

/** The same fallback the selector uses, together with its actual origin. */
function effortFor(model: ProviderModel, previous: string | null, providerId: string): { effort: string | null; origin: NonNullable<AutomaticRuns['effort']> } {
  const takes = model.parameters.efforts ?? [];
  const retained = previous !== null && takes.includes(previous);
  const effort = takes.length === 0 ? null : retained ? previous : (model.parameters.default_effort ?? takes[0] ?? null);
  return { effort, origin: { providerId, model: model.model_name, value: effort,
    by: model.parameters.efforts === null || model.parameters.efforts === undefined ? 'unreported' : takes.length === 0 ? 'none' : retained ? 'retained' : model.parameters.default_effort !== null && model.parameters.default_effort !== undefined ? 'default' : 'first', replaced: effort !== previous } };
}

/** One status beside the choice it explains, without repeating that control's label. */
function AutomaticChoice({ kind, origins, runs, caption = false }: { kind: keyof AutomaticRuns; origins: AutomaticRuns | undefined; runs: Runs; caption?: boolean }) {
  const { t } = useTranslation();
  let message;
  const profile = origins?.profile, model = origins?.model, effort = origins?.effort;
  if (kind === 'profile' && profile?.providerId === runs.providerId && profile.agentId === runs.agentId) {
    message = <>{t(`change.runs.automatic.profile.${profile.by}`)}{profile.previous !== null && <> · {t(`change.runs.automatic.previousProfile.${profile.previous}`)}</>}</>;
  } else if (kind === 'model' && model?.providerId === runs.providerId && model.model === runs.model) {
    message = <>{t(`change.runs.automatic.model.${model.by}`)}{model.replaced && <> · {t('change.runs.automatic.previousModel')}</>}</>;
  } else if (kind === 'effort' && effort?.providerId === runs.providerId && effort.model === runs.model && effort.value === runs.effort) {
    message = <>{t(`change.runs.automatic.effort.${effort.by}`)}{effort.replaced && <> · {t('change.runs.automatic.previousEffort')}</>}</>;
  } else return null;
  return <div data-automatic-runs={kind}><StateLine compact className="text-text-muted"
    title={caption ? t('change.runs.effort') : undefined} message={message} /></div>;
}

/** Why a provider cannot be chosen, in words; `null` for one that can. */
function providerWhy(provider: Provider, usable: boolean, t: TFunction): string | null {
  if (provider.status !== 'ready' && provider.status !== 'exhausted') return t(`start.provider.status.${provider.status}`);
  if (!usable) return t('start.provider.notYours', { owner: provider.owner.display_name });
  return null;
}

/** A native-only provider keeps its keeper profile; a forwarded declaration leaves compatibility to the ordinary arrangement review. */
export function profilesFor(provider: Provider | undefined, agents: readonly Agent[]): Agent[] {
  const live = agents.filter((agent) => !agent.disabled);
  const keeper = provider?.protocols.length === 0 ? provider.agent?.id : undefined;
  return keeper === undefined ? live : live.filter((agent) => agent.id === keeper);
}

/**
 * Another provider or profile takes a model of its own: the thread's where the provider lists it, else the first, and the effort
 * that goes with it. It is chosen here, in the dialog that holds the choices and not in the editor that shows them, so that a
 * provider that was chosen for the person (the next one, for a thread held back by its provider) has its model too while the
 * editor is closed.
 */
export function useModelFor(thread: Thread, choices: Choices, onChange: (patch: Partial<Choices>) => void, enabled = true): readonly ProviderModel[] {
  const runs = runsOf(choices, thread);
  const models = useQuery({ ...providerModelsQuery(runs.providerId), enabled: enabled && runs.providerId !== '' });
  useEffect(() => {
    if (!enabled || !movesRuns(choices) || models.isError) return;
    const listed = (models.data ?? []).filter((candidate) => candidate.enabled);
    if (listed.length === 0 || choices.model !== null && listed.some((candidate) => candidate.model_name === runs.model)) return;
    const keep = listed.find((candidate) => candidate.model_name === thread.model);
    const next = keep ?? listed[0];
    if (next === undefined) return;
    const { effort, origin } = effortFor(next, thread.effort, runs.providerId);
    // Materialize a retained routing name too; provenance stays local to this form.
    onChange({ model: next.model_name, effort, automaticRuns: { ...choices.automaticRuns,
      model: { providerId: runs.providerId, model: next.model_name, by: keep === undefined ? 'first' : 'retained', replaced: next.model_name !== runs.model },
      effort: origin } });
  }, [enabled, choices, models.data, models.isError, runs.model, runs.providerId, thread.model, thread.effort, onChange]);
  return models.data ?? [];
}

export interface RunsOnProps {
  thread: Thread;
  choices: Choices;
  onChange: (patch: Partial<Choices>) => void;
  focus?: boolean;
  /** One provider/profile picker for an ordinary switch; native planning still decides whether it can run. */
  compact?: boolean;
}

export function RunsOn({ thread, choices, onChange, focus = false, compact = false }: RunsOnProps) {
  const { t } = useTranslation();
  const usable = useQuery(providersQuery('usable'));
  const everything = useQuery(providersQuery('all'));
  const agents = useQuery(agentsQuery());
  const runs = runsOf(choices, thread);
  const moved = movesRuns(choices);
  const [arrangementOpen, setArrangementOpen] = useState(false);

  const usableIds = new Set((usable.data ?? []).map((provider) => provider.id));
  const known = [...new Map([...(everything.data ?? []), ...(usable.data ?? [])].map((provider) => [provider.id, provider])).values()];
  const names = providerLabels([...new Map([
    ...(everything.isFetchedAfterMount && !everything.isError ? everything.data ?? [] : []),
    ...(usable.isFetchedAfterMount && !usable.isError ? usable.data ?? [] : []),
  ].map((candidate) => [candidate.id, candidate])).values()], new Map());
  const ready = known.filter((provider) => usableIds.has(provider.id) && (provider.status === 'ready' || provider.status === 'exhausted'));
  const unusable = known.filter((provider) => !ready.some((one) => one.id === provider.id));
  const provider = known.find((candidate) => candidate.id === runs.providerId);
  const offered = profilesFor(provider, agents.data ?? []);

  const models = useQuery({ ...providerModelsQuery(runs.providerId), enabled: runs.providerId !== '' });
  const listed = models.data ?? [];
  const model = listed.find((candidate) => candidate.model_name === runs.model);
  const efforts = model?.parameters.efforts ?? [];
  const waiting = everything.isPending || agents.isPending || (runs.providerId !== '' && models.isPending);
  const ref = useFocusFirst<HTMLDivElement>(focus && !everything.isPending && !agents.isPending);
  const pairs = [...ready, ...unusable].flatMap((candidate) => (agents.data ?? []).map((agent) => ({
    value: `${candidate.id}:${agent.id}`, provider: candidate, agent,
  })));
  const choosePair = (value: string) => {
    const pair = pairs.find((candidate) => candidate.value === value);
    if (pair === undefined || !usableIds.has(pair.provider.id) || providerWhy(pair.provider, true, t) !== null || profileWhy(pair.provider, pair.agent, t) !== null) return;
    const providerId = pair.provider.id === thread.binding?.provider.id ? null : pair.provider.id;
    const agentId = pair.agent.id === thread.binding?.agent.id ? null : pair.agent.id;
    onChange({ providerId, agentId, model: null, effort: undefined, arrangement: null, automaticRuns: {} });
  };

  const choose = (providerId: string) => {
    const same = providerId === (thread.binding?.provider.id ?? '');
    if (same) {
      onChange({ providerId: null, agentId: null, model: null, effort: undefined, arrangement: null, automaticRuns: {} });
      return;
    }
    const next = known.find((candidate) => candidate.id === providerId);
    if (next === undefined || !usableIds.has(next.id) || providerWhy(next, true, t) !== null) return;
    onChange({ providerId, agentId: agentFor(next, agents.data ?? [], thread), model: null, effort: undefined, arrangement: null, automaticRuns: automaticProfileFor(next, agents.data ?? [], thread) });
  };

  return (
    <div ref={ref} className="flex flex-col gap-3" data-change-runs>
      {waiting && <p className="text-sm text-text-muted">{t('changeCompact.loadingRuntime')}</p>}
      {everything.isError && <ErrorState compact error={everything.error} onRetry={() => { void everything.refetch(); }} retrying={everything.isFetching} />}
      {usable.isError && <ErrorState compact error={usable.error} onRetry={() => { void usable.refetch(); }} retrying={usable.isFetching} />}
      {agents.isError && <ErrorState compact error={agents.error} onRetry={() => { void agents.refetch(); }} retrying={agents.isFetching} />}
      {everything.isPending || everything.isError && known.length === 0 ? null : known.length === 0 ? (
        <InlineAlert tone="warning">{t('start.provider.none')}</InlineAlert>
      ) : compact ? (
        <div className="flex min-w-0 flex-col gap-1"><ResourcePicker pickerKey="change.RunsOn.switchChoice.picker" label={t('switchChoice.picker')} value={`${runs.providerId}:${runs.agentId}`} onValueChange={choosePair}
          placeholder={thread.binding === null ? t('thread.blocked.unbound') : t('switchChoice.option', { agent: thread.binding.agent.name, provider: thread.binding.provider.name })}
          options={pairs.map((pair) => ({ value: pair.value,
            ...resourceChoice({ kind: 'provider', provider: pair.provider }),
            name: t('switchChoice.option', { agent: pair.agent.name, provider: names.get(pair.provider.id) ?? pair.provider.name }),
            disabled: !usableIds.has(pair.provider.id) || (pair.provider.status !== 'ready' && pair.provider.status !== 'exhausted') || profileWhy(pair.provider, pair.agent, t) !== null,
            description: <><ResourceHint subject={{ kind: 'provider', provider: pair.provider }} />{providerWhy(pair.provider, usableIds.has(pair.provider.id), t) ?? profileWhy(pair.provider, pair.agent, t)}</>,
          }))} />
          <AutomaticChoice kind="profile" origins={choices.automaticRuns} runs={runs} />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          <ResourcePicker pickerKey="change.RunsOn.change.runs.provider"
            label={t('change.runs.provider')}
            value={runs.providerId}
            onValueChange={choose}
            options={[...ready, ...unusable].map((candidate) => ({
              value: candidate.id,
              preview: resourcePreview({ kind: 'provider', provider: candidate }),
              name: candidate.id === thread.binding?.provider.id ? t('change.runs.providerNow', { name: names.get(candidate.id) ?? candidate.name }) : names.get(candidate.id) ?? candidate.name,
              disabled: !usableIds.has(candidate.id) || (candidate.status !== 'ready' && candidate.status !== 'exhausted'),
              description: <span className="flex flex-col gap-1">
                <ResourceHint subject={{ kind: 'provider', provider: candidate }} />
                {providerWhy(candidate, usableIds.has(candidate.id), t) !== null && <span>{providerWhy(candidate, usableIds.has(candidate.id), t)}</span>}
              </span>,
            }))}
          />
          <div className="flex min-w-0 flex-col gap-1"><ResourcePicker pickerKey="change.RunsOn.change.runs.agent"
            label={t('change.runs.agent')}
            value={runs.agentId}
            disabled={agents.isPending || (agents.data?.length ?? 0) === 0}
            onValueChange={(agentId) => {
              const agent = agents.data?.find((candidate) => candidate.id === agentId);
              if (agent === undefined || profileWhy(provider, agent, t) !== null) return;
              const { profile: _profile, ...automaticRuns } = choices.automaticRuns ?? {};
              onChange({ agentId: agentId === thread.binding?.agent.id ? null : agentId, automaticRuns });
            }}
            options={(agents.data ?? []).map((agent) => ({ value: agent.id, name: agent.name, ...resourceChoice({ kind: 'agent', agent, provider, models: listed }), description: profileWhy(provider, agent, t), disabled: profileWhy(provider, agent, t) !== null }))}
          />
            <AutomaticChoice kind="profile" origins={choices.automaticRuns} runs={runs} />
          </div>
        </div>
      )}
      {!waiting && (ready.length === 0 || !offered.some(hasReported)) && <ResourceCreationActions onReadChoices={(kind) => { if (kind === 'providers') void usable.refetch(); else void agents.refetch(); }} />}
      {(!compact || moved) && runs.providerId !== '' && (
        <div className="grid gap-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <div className="flex min-w-0 flex-col gap-1"><Select
            label={t(moved ? 'change.runs.model' : 'changeCompact.currentModel')}
            value={model === undefined ? '' : runs.model ?? ''}
            placeholder={t(models.isPending ? 'common.loading' : 'change.runs.modelUnknown')}
            disabled={!moved || models.isPending || listed.length === 0}
            onValueChange={(next) => {
              const chosen = listed.find((candidate) => candidate.model_name === next);
              if (!chosen?.enabled) return;
              const { effort, origin } = effortFor(chosen, runs.effort, runs.providerId);
              onChange({ model: next, effort, automaticRuns: { profile: choices.automaticRuns?.profile, effort: origin } });
            }}
            options={listed.map((candidate) => ({ value: candidate.model_name, name: modelLabel(candidate.model_name, listed, runs.providerId === thread.binding?.provider.id && candidate.model_name === thread.model ? thread.model_display : null, t), ...resourceChoice({ kind: 'model', model: candidate, providerId: runs.providerId, usage: provider?.choice_usage, effort: runs.effort }), disabled: !candidate.enabled, ...(!candidate.enabled ? { description: t('change.runs.unavailable.modelDisabled') } : {}) }))}
          />
            {models.isError && <ErrorState compact error={models.error} onRetry={() => { void models.refetch(); }} retrying={models.isFetching} />}
            {!models.isPending && !models.isError && listed.length === 0 && <StateLine compact message={t('start.model.none')} />}
            <AutomaticChoice kind="model" origins={choices.automaticRuns} runs={runs} />
          </div>
          <div className="flex min-w-0 flex-col gap-1">{efforts.length > 0 && <Select label={t(moved ? 'change.runs.effort' : 'changeCompact.currentEffort')} value={runs.effort ?? ''} disabled={!moved} onValueChange={(effort) => {
            const { effort: _effort, ...automaticRuns } = choices.automaticRuns ?? {};
            onChange({ effort, automaticRuns });
          }} options={efforts.map((value) => ({ value, name: effortLabel(value, t) ?? '' }))} />}
            <AutomaticChoice kind="effort" origins={choices.automaticRuns} runs={runs} caption={efforts.length === 0} />
          </div>
        </div>
      )}
      {moved && provider !== undefined && (
        <details className="text-sm" onToggle={(event) => setArrangementOpen(event.currentTarget.open)}>
          <summary className="cursor-default rounded-md text-text-muted select-none hover:text-text">{t('change.runs.arrangement')}</summary>
          <div className="mt-2">
            {arrangementOpen && <ProviderArrangementChoice provider={provider} value={choices.arrangement} onChange={(arrangement) => onChange({ arrangement })} />}
          </div>
        </details>
      )}
    </div>
  );
}
