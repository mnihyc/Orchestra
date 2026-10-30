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
import type { Choices } from '@/features/change/choices';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { useFocusFirst } from '@/features/change/Sentence';
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
  const profiles = profilesFor(provider, agents).filter(hasReported);
  const own = profiles.find((agent) => agent.id === thread.binding?.agent.id);
  const chosen = own ?? profiles[0];
  return chosen === undefined || chosen.id === thread.binding?.agent.id ? null : chosen.id;
}

/** Why a provider cannot be chosen, in words; `null` for one that can. */
function providerWhy(provider: Provider, usable: boolean, t: TFunction): string | null {
  if (provider.status !== 'ready') return t(`start.provider.status.${provider.status}`);
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
  const listed = (models.data ?? []).filter((candidate) => candidate.enabled);
  const modelNeeded = enabled && movesRuns(choices) && listed.length > 0 && (choices.model === null || !listed.some((candidate) => candidate.model_name === runs.model));
  useEffect(() => {
    if (!modelNeeded) return;
    const keep = listed.find((candidate) => candidate.model_name === thread.model);
    const next = keep ?? listed[0];
    if (next === undefined) return;
    const takes = next.parameters.efforts ?? [];
    const effort = takes.length === 0 ? null : takes.includes(thread.effort ?? '') ? thread.effort : (next.parameters.default_effort ?? takes[0] ?? null);
    onChange({ model: next.model_name, effort });
    // Materialize the displayed model in the request even when both providers list the same routing name.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [modelNeeded, runs.providerId, listed.length]);
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
  const ready = known.filter((provider) => usableIds.has(provider.id) && provider.status === 'ready');
  const unusable = known.filter((provider) => !ready.some((one) => one.id === provider.id));
  const provider = known.find((candidate) => candidate.id === runs.providerId);
  const offered = profilesFor(provider, agents.data ?? []);

  const models = useQuery({ ...providerModelsQuery(runs.providerId), enabled: runs.providerId !== '' });
  const listed = (models.data ?? []).filter((candidate) => candidate.enabled);
  const model = listed.find((candidate) => candidate.model_name === runs.model);
  const efforts = model?.parameters.efforts ?? [];
  const waiting = everything.isPending || agents.isPending || (runs.providerId !== '' && models.isPending);
  const ref = useFocusFirst<HTMLDivElement>(focus && !everything.isPending && !agents.isPending);
  const pairs = [...ready, ...unusable].flatMap((candidate) => profilesFor(candidate, agents.data ?? []).map((agent) => ({
    value: `${candidate.id}:${agent.id}`, provider: candidate, agent,
  })));
  const choosePair = (value: string) => {
    const pair = pairs.find((candidate) => candidate.value === value);
    if (pair === undefined) return;
    const providerId = pair.provider.id === thread.binding?.provider.id ? null : pair.provider.id;
    const agentId = pair.agent.id === thread.binding?.agent.id ? null : pair.agent.id;
    onChange({ providerId, agentId, model: null, effort: undefined, arrangement: null });
  };

  const choose = (providerId: string) => {
    const same = providerId === (thread.binding?.provider.id ?? '');
    if (same) {
      onChange({ providerId: null, agentId: null, model: null, effort: undefined, arrangement: null });
      return;
    }
    const next = known.find((candidate) => candidate.id === providerId);
    onChange({ providerId, agentId: agentFor(next, agents.data ?? [], thread), model: null, effort: undefined, arrangement: null });
  };

  return (
    <div ref={ref} className="flex flex-col gap-3" data-change-runs>
      {waiting && <p className="text-sm text-text-muted">{t('changeCompact.loadingRuntime')}</p>}
      {everything.isPending ? null : known.length === 0 ? (
        <InlineAlert tone="warning">{t('start.provider.none')}</InlineAlert>
      ) : compact ? (
        <Select label={t('switchChoice.picker')} value={`${runs.providerId}:${runs.agentId}`} onValueChange={choosePair}
          placeholder={thread.binding === null ? t('thread.blocked.unbound') : t('switchChoice.option', { agent: thread.binding.agent.name, provider: thread.binding.provider.name })}
          options={pairs.map((pair) => ({ value: pair.value,
            ...resourceChoice({ kind: 'provider', provider: pair.provider }),
            name: t('switchChoice.option', { agent: pair.agent.name, provider: names.get(pair.provider.id) ?? pair.provider.name }),
            disabled: !usableIds.has(pair.provider.id) || pair.provider.status !== 'ready' || !hasReported(pair.agent),
            description: <><ResourceHint subject={{ kind: 'provider', provider: pair.provider }} />{!hasReported(pair.agent) ? t('start.agent.unreported') : providerWhy(pair.provider, usableIds.has(pair.provider.id), t)}</>,
          }))} />
      ) : (
        <div className="grid gap-3 sm:grid-cols-2">
          <Select
            label={t('change.runs.provider')}
            value={runs.providerId}
            onValueChange={choose}
            options={[...ready, ...unusable].map((candidate) => ({
              value: candidate.id,
              preview: resourcePreview({ kind: 'provider', provider: candidate }),
              name: candidate.id === thread.binding?.provider.id ? t('change.runs.providerNow', { name: names.get(candidate.id) ?? candidate.name }) : names.get(candidate.id) ?? candidate.name,
              disabled: !usableIds.has(candidate.id) || candidate.status !== 'ready',
              description: <span className="flex flex-col gap-1">
                <ResourceHint subject={{ kind: 'provider', provider: candidate }} />
                {providerWhy(candidate, usableIds.has(candidate.id), t) !== null && <span>{providerWhy(candidate, usableIds.has(candidate.id), t)}</span>}
              </span>,
            }))}
          />
          <Select
            label={t('change.runs.agent')}
            value={runs.agentId}
            disabled={offered.length <= 1}
            onValueChange={(agentId) => onChange({ agentId: agentId === thread.binding?.agent.id ? null : agentId })}
            options={offered.map((agent) => ({ value: agent.id, name: agent.name, ...resourceChoice({ kind: 'agent', agent, provider, models: listed }), description: hasReported(agent) ? undefined : t('start.agent.unreported'), disabled: !hasReported(agent) }))}
          />
        </div>
      )}
      {!waiting && (ready.length === 0 || !offered.some(hasReported)) && <ResourceCreationActions onReadChoices={(kind) => { if (kind === 'providers') void usable.refetch(); else void agents.refetch(); }} />}
      {(!compact || moved) && (runs.providerId === '' || models.isPending ? null : listed.length === 0 ? (
        <InlineAlert tone="warning">{t('start.model.none')}</InlineAlert>
      ) : (
        <div className="grid gap-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <Select
            label={t(moved ? 'change.runs.model' : 'changeCompact.currentModel')}
            value={runs.model ?? ''}
            disabled={!moved}
            onValueChange={(next) => {
              const chosen = listed.find((candidate) => candidate.model_name === next);
              const takes = chosen?.parameters.efforts ?? [];
              onChange({ model: next, effort: takes.length === 0 ? null : takes.includes(runs.effort ?? '') ? runs.effort : (chosen?.parameters.default_effort ?? takes[0] ?? null) });
            }}
            options={listed.map((candidate) => ({ value: candidate.model_name, name: modelLabel(candidate.model_name, listed, runs.providerId === thread.binding?.provider.id && candidate.model_name === thread.model ? thread.model_display : null, t), ...resourceChoice({ kind: 'model', model: candidate, providerId: runs.providerId, usage: provider?.choice_usage, effort: runs.effort }) }))}
          />
          {efforts.length > 0 && <Select label={t(moved ? 'change.runs.effort' : 'changeCompact.currentEffort')} value={runs.effort ?? ''} disabled={!moved} onValueChange={(effort) => onChange({ effort })} options={efforts.map((value) => ({ value, name: effortLabel(value, t) ?? '' }))} />}
        </div>
      ))}
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
