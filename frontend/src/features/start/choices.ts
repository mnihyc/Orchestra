/** What a thread would be bound to, worked out from what the server says it could be and what the person chose by hand: the one place that decides what is shown as chosen, what is missing and what is sent.
 *
 * Nothing here asks anything of the server and nothing here knows how a screen draws. The server lists, for the place,
 * every provider with the reason one cannot be used, its profiles and models, every arrangement of a profile on a
 * provider in an environment with whether it can be had, and the environments the person may use or request. This turns that
 * and the person's own picks into one answer: the provider, profile, model and effort in force, the environment, the
 * arrangement that is ticked and the one the server would take if none were named, and what is in the way. A pick
 * that cannot be had any more (the provider was changed under it) is not kept; an arrangement that cannot be had is
 * never swapped for another one behind the person's back: it stays listed as closed, with the server's reason.
 */
import type {
  Arrangement,
  ArrangementType,
  BindingAgent,
  BindingEnvironment,
  BindingProvider,
  ModelOption,
  ProviderStatus,
} from '@/generated/api-types';

import type { StartOptions } from '@/api/start';
import { modelDisplayName } from '@/features/models/names';

/** What stands for "a new environment, made from the defaults of the place" among the environments that exist. */
export const NEW_ENVIRONMENT = 'new';

/** Permission alone cannot make a failed or removed workspace a valid start/default. */
export function canUseEnvironment(row: BindingEnvironment): boolean {
  return row.usable && !['failed', 'disabled', 'removed'].includes(row.environment.status);
}

/** What the person chose by hand. Everything left `null` comes from the defaults, or, in a dialog, from a suggestion. */
export interface Picks {
  provider: string | null;
  agent: string | null;
  model: string | null;
  effort: string | null;
  /** The id of an environment that exists, or `NEW_ENVIRONMENT`. */
  environment: string | null;
  arrangement: ArrangementType | null;
}

export const NO_PICKS: Picks = { provider: null, agent: null, model: null, effort: null, environment: null, arrangement: null };

/** Why a provider cannot be chosen: the server says the caller may not use it, or it is not in a state to run work. */
export type ProviderWhy = { kind: 'reason'; reason: string } | { kind: 'status'; status: ProviderStatus };

export interface ProviderRow {
  id: string;
  entry: BindingProvider;
  /** It may be used and is ready to run work. */
  choosable: boolean;
  why: ProviderWhy | null;
  /** The place's default provider. */
  isDefault: boolean;
}

export interface AgentRow {
  id: string;
  entry: BindingAgent;
  /** It may be used and its harness has said what it is, so that it can be placed. */
  startable: boolean;
  /** Its harness has not reported yet: nothing is known of what it could be placed in. */
  unreported: boolean;
  isDefault: boolean;
}

export interface ArrangementRow {
  type: ArrangementType;
  entry: Arrangement;
  /** It can be had and it is allowed. */
  open: boolean;
}

/** No provider is chosen. */
export interface MissingProvider {
  kind: 'provider';
  /** The place's default provider, when it is set and cannot be used. */
  unusable: ProviderRow | null;
  /** Whether the place names a default provider at all. */
  defaultSet: boolean;
  /** Whether there is no provider the person could choose. */
  none: boolean;
}

/** No profile is chosen. */
export interface MissingAgent {
  kind: 'agent';
  /** The profile the place defaults to has not reported what its harness is, or none that could be used has. */
  unreported: boolean;
  /** The profile that the place defaults to, when it cannot be used. */
  unusable: AgentRow | null;
  /** The provider offers no profile at all. */
  none: boolean;
}

/** No model is chosen. */
export interface MissingModel {
  kind: 'model';
  /** The provider lists no model the person may use. */
  none: boolean;
}

/** No environment is chosen, and none can be made. */
export interface MissingEnvironment {
  kind: 'environment';
}

/** Nothing can place the profile on the provider here. */
export interface MissingArrangement {
  kind: 'arrangement';
  /** The first of the arrangements in the server's order, with its reason. */
  first: ArrangementRow | null;
}

/** What is in the way of starting, in the order a person meets it: a provider, a profile, a model, an environment, an arrangement. */
export type Missing = MissingProvider | MissingAgent | MissingModel | MissingEnvironment | MissingArrangement;

export interface Resolved {
  providers: ProviderRow[];
  provider: ProviderRow | null;
  agents: AgentRow[];
  agent: AgentRow | null;
  /** The models the person may use on the provider. */
  models: ModelOption[];
  /** The models the provider lists that the person may not use, each with its reason. */
  closedModels: ModelOption[];
  model: ModelOption | null;
  efforts: string[];
  effort: string | null;
  /** The environments the person may use, in the server's order. */
  environments: BindingEnvironment[];
  /** The environment that exists and is chosen. */
  environment: BindingEnvironment | null;
  /** A new environment is made for the thread, from the defaults of the place. */
  fresh: boolean;
  /** The environment that was chosen is not in the answer yet: it was made a moment ago and the answer is being asked again. */
  settling: boolean;
  /** Every arrangement of the profile on the provider, the ones that can be had first, in the server's order. */
  arrangements: ArrangementRow[];
  /** The one that is ticked, and so the one that is sent. */
  arrangement: ArrangementType | null;
  /** The one the server takes when none is named: the open one it ranks first. */
  defaultArrangement: ArrangementType | null;
  missing: Missing[];
  /** A harness that has not reported yet is in the way, which the next report puts right: the answer is asked again until it has. */
  waiting: boolean;
}

/** How the choices are filled in where the person has said nothing. */
export interface Mode {
  /** Fill a first setup from the usable choices when neither defaults nor a previous setup settle it. */
  suggest: boolean;
  /** An unavailable configured default needs a deliberate replacement, rather than a silent suggestion. */
  respectDefaults?: boolean;
}

/** A routing name remains a request value even when the provider reported no display name. */
export const modelName = (model: ModelOption): string => modelDisplayName(model.display_name);

/** The providers of an answer, those that can be chosen first, each with why not where it cannot. */
function providerRows(options: StartOptions): ProviderRow[] {
  const rows = options.providers.map((entry): ProviderRow => {
    const status = entry.provider.status;
    const why: ProviderWhy | null = !entry.usable ? { kind: 'reason', reason: entry.reason ?? 'no_use' } : status !== 'ready' ? { kind: 'status', status } : null;
    return { id: entry.provider.id, entry, choosable: why === null, why, isDefault: options.defaults.provider_id === entry.provider.id };
  });
  return [...rows.filter((row) => row.choosable), ...rows.filter((row) => !row.choosable)];
}

function agentRows(provider: ProviderRow | null, options: StartOptions): AgentRow[] {
  return (provider?.entry.agents ?? []).map((entry) => ({
    id: entry.agent.id,
    entry,
    startable: entry.usable && entry.reason === null && entry.arrangements.some((choice) => choice.available && choice.permitted),
    unreported: entry.reason === 'harness_unreported',
    isDefault: options.defaults.agent_id === entry.agent.id,
  }));
}

/** Every arrangement in the server's order: the rank it gave, which puts the ones that can be had before the others. */
function arrangementRows(agent: AgentRow | null): ArrangementRow[] {
  return [...(agent?.entry.arrangements ?? [])]
    .sort((one, other) => one.rank - other.rank)
    .map((entry) => ({ type: entry.type, entry, open: entry.available && entry.permitted }));
}

/** Works out what a thread would be bound to. */
export function resolve(options: StartOptions, picks: Picks, mode: Mode, last: Picks = NO_PICKS): Resolved {
  const missing: Missing[] = [];
  const defaults = options.defaults;

  const providers = providerRows(options);
  const choosable = providers.filter((row) => row.choosable);
  const provider = choosable.find((row) => row.id === picks.provider) ?? choosable.find((row) => row.id === defaults.provider_id) ?? (!defaults.locked ? choosable.find((row) => row.id === last.provider) : undefined) ?? (mode.suggest && (!mode.respectDefaults || defaults.provider_id === null) ? choosable[0] : undefined) ?? null;
  // Do not borrow another provider's profile/model/consent when a place default wins.
  const previous = provider?.id === last.provider ? last : NO_PICKS;
  if (provider === null) {
    missing.push({
      kind: 'provider',
      unusable: providers.find((row) => row.isDefault && !row.choosable) ?? null,
      defaultSet: defaults.provider_id !== null,
      none: choosable.length === 0,
    });
  }

  const agents = agentRows(provider, options);
  const startable = agents.filter((row) => row.startable);
  // A profile the place defaults to that cannot be used is not swapped for another behind its back: the inline starter asks, and waits where it has not reported.
  const defaultBlocked = agents.some((row) => row.isDefault && !row.startable);
  // A provider that offers one profile only has nothing to choose: that profile is the one.
  const agent =
    provider === null
      ? null
      : (startable.find((row) => row.id === picks.agent) ?? startable.find((row) => row.id === defaults.agent_id) ?? startable.find((row) => row.id === previous.agent) ?? ((startable.length === 1 && !defaultBlocked) || (mode.suggest && (!mode.respectDefaults || !defaultBlocked)) ? startable[0] : undefined) ?? null);
  if (provider !== null && agent === null) {
    const unusable = agents.find((row) => row.isDefault && !row.startable) ?? null;
    missing.push({
      kind: 'agent',
      // The profile the place defaults to has not reported, or none of them has: what is waited for, with nothing offered that cannot run.
      unreported: unusable?.unreported === true || (startable.length === 0 && agents.some((row) => row.unreported)),
      unusable,
      none: agents.length === 0,
    });
  }

  const models = (provider?.entry.models ?? []).filter((candidate) => candidate.allowed);
  const closedModels = (provider?.entry.models ?? []).filter((candidate) => !candidate.allowed);
  // The same for the model: one the place names that cannot be had is not replaced by the only one that can.
  const modelBlocked = defaults.model_name !== null && !models.some((candidate) => candidate.model_name === defaults.model_name);
  const model =
    provider === null
      ? null
      : (models.find((candidate) => candidate.model_name === picks.model) ?? models.find((candidate) => candidate.model_name === defaults.model_name) ?? models.find((candidate) => candidate.model_name === previous.model) ?? ((models.length === 1 && !modelBlocked) || (mode.suggest && (!mode.respectDefaults || !modelBlocked)) ? models[0] : undefined) ?? null);
  if (provider !== null && model === null) missing.push({ kind: 'model', none: models.length === 0 });

  const efforts = model?.parameters.efforts ?? [];
  const effort =
    efforts.length === 0
      ? null
      : efforts.includes(picks.effort ?? '')
        ? picks.effort
        : efforts.includes(defaults.effort ?? '')
          ? defaults.effort
          : model?.model_name === previous.model && efforts.includes(previous.effort ?? '') ? previous.effort : (model?.parameters.default_effort ?? efforts[0] ?? null);

  const environments = options.environments.filter(canUseEnvironment);
  const mayMake = options.can_create.allowed;
  const named = picks.environment !== null && picks.environment !== NEW_ENVIRONMENT;
  const kept = named ? (environments.find((candidate) => candidate.environment.id === picks.environment) ?? null) : null;
  // An environment that was made a moment ago is the person's choice before the answer lists it.
  const settling = named && !options.environments.some((candidate) => candidate.environment.id === picks.environment);
  const unavailable = named && !settling && kept === null;
  const shared = options.environment_default === 'shared' ? (environments.find((candidate) => candidate.is_default) ?? null) : null;
  const previousEnvironment = picks.environment === null && shared === null && options.environment_default === 'shared' ? environments.find((candidate) => candidate.environment.id === last.environment) ?? null : null;
  // What is pre-selected follows the place's own rule: its default environment where the person may use it, otherwise a new one.
  const fresh = !settling && !unavailable && kept === null && previousEnvironment === null && (picks.environment === NEW_ENVIRONMENT ? mayMake : shared === null && mayMake);
  const environment = unavailable ? null : kept ?? previousEnvironment ?? (fresh || settling ? null : (shared ?? (mode.suggest && !mode.respectDefaults ? (environments[0] ?? null) : null)));
  if (!fresh && !settling && environment === null) missing.push({ kind: 'environment' });

  const arrangements = arrangementRows(agent);
  const open = arrangements.filter((row) => row.open);
  const defaultArrangement = open[0]?.type ?? null;
  // Optional placement follows the current place/provider preference; an old setup is not new credential consent.
  const arrangement = open.find((row) => row.type === picks.arrangement)?.type ?? defaultArrangement;
  if (provider !== null && agent !== null && arrangement === null) missing.push({ kind: 'arrangement', first: arrangements[0] ?? null });

  const waiting = missing.some((entry) => entry.kind === 'agent' && entry.unreported);
  return {
    providers,
    provider,
    agents,
    agent,
    models,
    closedModels,
    model,
    efforts,
    effort,
    environments,
    environment,
    fresh,
    settling,
    arrangements,
    arrangement,
    defaultArrangement,
    missing,
    waiting,
  };
}

/** The settings a person's choice would write as the defaults of a place: those that differ from what the place has already. */
export function settingsToKeep(options: StartOptions, resolved: Resolved): { key: string; value: string }[] {
  const defaults = options.defaults;
  const wanted: { key: string; value: string | null; was: string | null }[] = [
    { key: 'agent.default_provider_id', value: resolved.provider?.id ?? null, was: defaults.provider_id },
    { key: 'agent.default_agent_id', value: resolved.agent?.id ?? null, was: defaults.agent_id },
    { key: 'agent.model', value: resolved.model?.model_name ?? null, was: defaults.model_name },
    { key: 'agent.effort', value: resolved.effort, was: defaults.effort },
  ];
  return wanted.flatMap((entry) => (entry.value === null || entry.value === entry.was ? [] : [{ key: entry.key, value: entry.value }]));
}

/** Whether something is yet to be chosen that the place could have decided: a provider, a profile or a model. */
export function missesDefault(resolved: Resolved): boolean {
  return resolved.missing.some((entry) => entry.kind === 'provider' || entry.kind === 'agent' || entry.kind === 'model');
}
