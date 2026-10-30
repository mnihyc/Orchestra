/** The permitted facts and short hints shared by resource choices, name cards and status items. */
import type { ObjectRef } from '@/generated/api-types';
import type { TFunction } from 'i18next';

import { formatContext } from '@/features/models/context';
import { listNames } from '@/features/models/patterns';
import { detailState, amountText, reportedList, windowName } from '@/features/resources/providers/details';
import { privateProvider } from '@/features/resources/providers/plan';
import { choiceObservation, speedText } from '@/features/usage/performance';
import { reportedHarnessName } from '@/features/resources/agents/descriptor';
import { programNameIn } from '@/features/resources/programNames';
import type { Agent, CapStatus, Environment, Host, HostRef, MachineRef, McpServer, McpState, ModelParameters, Mount, PlaceHost, Provider, ProviderChoiceUsage, ResourceSample, Skill, Template } from '@/generated/api-types';
import { formatBytes, formatNumber, formatPercent } from '@/lib/format';
import { formatSpan, formatTimeOfDay, type TimeContext } from '@/lib/time';

/** Resource counts use the binary megabytes and gigabytes already used by the places list. */
export const MEMORY_BYTES_PER_MB = 1024 * 1024;
export const DISK_BYTES_PER_GB = 1024 * 1024 * 1024;

export type ProviderFacts = Pick<Provider, 'id' | 'name'> & Partial<Provider>;
export type HostFacts = Pick<Host, 'id' | 'name'> & Partial<Host>;
export type EnvironmentFacts = Pick<Environment, 'id' | 'name'> & Partial<Environment>;
type Named<T extends { id: string; name: string }> = Pick<T, 'id' | 'name'> & Partial<T>;
export type MountFacts = Named<Mount>;
export type McpFacts = Named<McpServer> & { enabled_places?: number | null; offering_machines?: MachineRef[] };
export type AgentFacts = Named<Agent>;
export type SkillFacts = Named<Skill> & { enabled_places?: number | null };
export interface ModelFacts {
  provider_id?: string;
  model_name: string;
  display_name: string;
  parameters: ModelParameters;
  groups?: readonly string[];
}

export type ResourceSubject =
  | { kind: 'provider'; provider: ProviderFacts }
  | { kind: 'host'; host: HostFacts; place?: PlaceHost }
  | { kind: 'environment'; environment: EnvironmentFacts; host?: HostRef | null; sample?: ResourceSample | null }
  | { kind: 'mount'; mount: MountFacts }
  | { kind: 'mcp_server'; server: McpFacts; state?: McpState | undefined }
  | { kind: 'template'; template: Named<Template> }
  | { kind: 'agent'; agent: AgentFacts; provider?: ProviderFacts | undefined; models?: readonly ModelFacts[] | undefined }
  | { kind: 'skill'; skill: SkillFacts }
  | { kind: 'model'; model: ModelFacts; providerId: string; usage?: ProviderChoiceUsage | null | undefined; effort?: string | null | undefined; speed?: string | null | undefined; impact?: string | undefined };

export function modelResource(model: ModelFacts, providerId: string, options: Omit<Extract<ResourceSubject, { kind: 'model' }>, 'kind' | 'model' | 'providerId'> = {}): Extract<ResourceSubject, { kind: 'model' }> {
  return { kind: 'model', model, providerId, ...options };
}

export interface Hint { text: string; warning: boolean }
export interface FactContext extends TimeContext { t: TFunction; now: number }
const hint = (text: string, warning = false): Hint => ({ text, warning });

/** Only a fully readable provider can disclose vendor amounts or its caps. */
export function fullProvider(provider: ProviderFacts): provider is Provider {
  return provider.facts !== undefined && provider.facts_state !== undefined && provider.access !== undefined;
}

export function fullHost(host: HostFacts): host is Host { return host.capacity !== undefined && host.machine !== undefined && host.access !== undefined; }
export function fullEnvironment(environment: EnvironmentFacts): environment is Environment { return environment.access !== undefined && environment.exec !== undefined; }

/** Applicable provider caps retain server-side subject and replacement decisions; model groups use the shared pattern rule. */
export function modelCaps(subject: Extract<ResourceSubject, { kind: 'model' }>): CapStatus[] {
  return (subject.usage?.caps ?? []).filter(({ cap, replaced_by }) => cap.enabled && replaced_by === null
    && (cap.models.length === 0 || listNames(cap.models, subject.model.model_name, subject.model.groups ?? [])));
}

function atLimit(reset: string | null | undefined, context: FactContext): Hint {
  const { t } = context;
  return hint(reset === null || reset === undefined ? t('resourceFacts.atLimit')
    : t('resourceFacts.atLimitUntil', { time: formatTimeOfDay(reset, context, context.now) }), true);
}

export function providerHint(provider: ProviderFacts, context: FactContext, scope: 'choice' | 'window' = 'choice'): Hint {
  const { t, locale } = context;
  if (provider.status === undefined) return hint(t('providerPlan.notReported'));
  if (provider.status === 'exhausted') return atLimit(fullProvider(provider) ? provider.facts.limit_reached?.resets_at : null, context);
  if (provider.status !== 'ready') return hint(t('resourceFacts.unavailable'), true);
  if (!fullProvider(provider)) return hint(t('providerPlan.notReported'));
  if (privateProvider(provider)) return hint(t('providerPlan.roomHidden'));
  const caps = (provider.choice_usage?.caps ?? []).filter(({ cap }) => cap.models.length === 0 && cap.tiers.length === 0 && cap.harness_kinds.length === 0 && cap.node === null && (scope === 'choice' || cap.meter !== 'running'));
  const reached = caps.find((cap) => cap.state === 'reached');
  if (reached !== undefined) return atLimit(reached.period_ends_at, context);
  if (detailState(provider, 'limit_reached') === 'told' && provider.facts.limit_reached !== null) return atLimit(provider.facts.limit_reached.resets_at, context);
  const windows = detailState(provider, 'windows') === 'told' ? reportedList(provider.facts.windows).filter((window) => Number.isFinite(window.used_percent)) : [];
  const binding = windows.some((window) => window.is_active !== null) ? windows.filter((window) => window.is_active === true) : windows;
  const tightest = binding.reduce<(typeof binding)[number] | undefined>((picked, next) => picked === undefined || next.used_percent > picked.used_percent ? next : picked, undefined);
  const runningCap = caps.filter((cap) => cap.cap.meter === 'running' && cap.cap.amount !== null && Number(cap.cap.amount) > 0)
    .sort((one, other) => Number(other.used) / Number(other.cap.amount) - Number(one.used) / Number(one.cap.amount))[0];
  if (runningCap !== undefined && (tightest === undefined || Number(runningCap.used) / Number(runningCap.cap.amount) * 100 >= tightest.used_percent)) {
    return hint(t('resourceFacts.running', { count: Number(runningCap.used), value: formatNumber(Number(runningCap.used), locale) }), runningCap.state === 'warning');
  }
  if (tightest !== undefined) {
    const used = t('resourceFacts.windowUsed', { percent: formatNumber(tightest.used_percent, locale, { maximumFractionDigits: 1 }), window: windowName(tightest) });
    const reset = tightest.resets_at === null ? null : t('resourceFacts.resetsAt', { time: formatTimeOfDay(tightest.resets_at, context, context.now) });
    const threshold = tightest.warning_percent;
    return hint([used, reset].filter(Boolean).join(t('resourceFacts.separator')), threshold !== null && Number.isFinite(threshold) && tightest.used_percent >= threshold);
  }
  const balance = detailState(provider, 'balances') === 'told' ? provider.facts.balances.find((balance) => balance.remaining !== null) : undefined;
  if (balance !== undefined) return hint(t('resourceFacts.balanceLeft', { amount: amountText(balance.remaining, balance.unit, t('providerPlan.notReported')) }));
  if (scope === 'window') return hint('');
  const turns = runningCap === undefined ? provider.running_threads : Number(runningCap.used);
  return hint(t('resourceFacts.running', { count: turns, value: formatNumber(turns, locale) }), runningCap?.state === 'warning');
}

export function modelHint(subject: Extract<ResourceSubject, { kind: 'model' }>, context: FactContext): Hint {
  const limited = modelCaps(subject).find(({ state, cap }) => state === 'reached' && cap.tiers.length === 0 && cap.harness_kinds.length === 0 && cap.node === null);
  if (limited !== undefined) return atLimit(limited.period_ends_at, context);
  const row = choiceObservation(subject.usage?.performance ?? [], subject.model.model_name, subject.effort ?? subject.model.parameters.default_effort ?? null, null, null);
  const speed = subject.speed ?? (row === null ? null : speedText(row.performance.speed_milli === null ? null : row.performance.speed_milli / 1000, context.locale, context.t));
  const window = subject.model.parameters.context_window;
  return hint([window === undefined || window === null ? null : formatContext(window, context.locale), speed].filter(Boolean).join(context.t('resourceFacts.separator')));
}

export function hostHint(subject: Extract<ResourceSubject, { kind: 'host' }>, context: FactContext): Hint {
  const { host, place } = subject;
  const { t, locale } = context;
  if (host.machine?.online === false) return hint(host.offline_since === null || host.offline_since === undefined
    ? t('resourceFacts.offline') : t('resourceFacts.offlineSince', { time: formatTimeOfDay(host.offline_since, context, context.now) }), true);
  const capacity = place?.capacity ?? host.capacity;
  const declared = place?.declared ?? host.declared;
  const full = place?.warnings.includes('full') === true || place?.reasons.includes('full') === true || host.full === true;
  if (full) return hint(t('resourceFacts.full'), true);
  if (place?.usable === false || host.schedulable === false) return hint(t('resourceFacts.unavailable'), true);
  const count = place?.environments.running ?? host.running?.environment;
  const memory = place?.used.memory_mb ?? host.usage?.memory_used_mb;
  const storages = place?.storages ?? host.storages;
  const free = storages === undefined || storages.length === 0 ? null : storages.reduce((sum, storage) => sum + storage.free_gb, 0);
  return hint([
    count === undefined ? null : t('resourceFacts.running', { count, value: formatNumber(count, locale) }),
    capacity === undefined || capacity.memory_mb <= 0 ? null : memory !== undefined && memory !== null
      ? t('resourceFacts.memoryPercent', { percent: formatPercent(memory / capacity.memory_mb, locale) })
      : declared === undefined ? null : t('resourceFacts.declaredPercent', { percent: formatPercent(declared.memory_mb / capacity.memory_mb, locale) }),
    free === null ? null : t('resourceFacts.free', { size: formatBytes(free * DISK_BYTES_PER_GB, locale) }),
  ].filter(Boolean).join(t('resourceFacts.separator')) || t('providerPlan.notReported'));
}

export function environmentHint(subject: Extract<ResourceSubject, { kind: 'environment' }>, context: FactContext): Hint {
  const { environment } = subject;
  const { t, now } = context;
  if (environment.status === undefined) return hint(t('providerPlan.notReported'));
  if (environment.status === 'offline') return hint(t('resourceFacts.hostOffline'), true);
  if (environment.status === 'failed' || environment.status === 'removed') return hint(t(`environments.status.${environment.status}`), true);
  if (environment.status === 'disabled') return hint(t('environments.status.disabled'), true);
  const host = subject.host ?? environment.host;
  if (environment.status === 'stopped') return hint([t('environments.status.stopped'), environment.last_activity_at === null || environment.last_activity_at === undefined ? null
    : t('resourceFacts.idleFor', { time: formatSpan(Math.max(0, now - Date.parse(environment.last_activity_at)), context) })].filter(Boolean).join(t('resourceFacts.separator')));
  const running = environment.activity?.running_threads;
  return hint([running === undefined ? t('resourceFacts.environmentState', { state: t(`environments.status.${environment.status}`) }) : t('resourceFacts.running', { count: running, value: formatNumber(running, context.locale) }), host?.name].filter(Boolean).join(t('resourceFacts.separator')));
}

/** This registry owns the four resource hints; cards and arranged status items read the same definitions. */
export const resourceDefinitions = {
  provider: { hint: (subject: Extract<ResourceSubject, { kind: 'provider' }>, context: FactContext) => providerHint(subject.provider, context) },
  model: { hint: modelHint },
  host: { hint: hostHint },
  environment: { hint: environmentHint },
  mount: { hint: (subject: Extract<ResourceSubject, { kind: 'mount' }>, context: FactContext) => subject.mount.enabled === false ? hint(context.t('resourceFacts.unavailable'), true) : hint(context.t(subject.mount.read_only === true ? 'resourceFacts.readOnly' : 'resourceFacts.mount')) },
  mcp_server: { hint: (subject: Extract<ResourceSubject, { kind: 'mcp_server' }>, context: FactContext) => subject.server.enabled === false ? hint(context.t('resourceFacts.unavailable'), true) : hint(subject.server.machine === null ? context.t('resourceFacts.noMachine') : subject.server.machine?.name ?? '') },
  template: { hint: (subject: Extract<ResourceSubject, { kind: 'template' }>, context: FactContext) => subject.template.enabled === false ? hint(context.t('resourceFacts.unavailable'), true) : hint(subject.template.units === undefined ? '' : context.t('resourceFacts.usedBy', { value: formatNumber(subject.template.units, context.locale) })) },
  agent: { hint: (subject: Extract<ResourceSubject, { kind: 'agent' }>, context: FactContext) => {
    if (subject.agent.disabled === true) return hint(context.t('resourceFacts.unavailable'), true);
    const name = programNameIn(subject.agent, context.locale) ?? reportedHarnessName(subject.agent.descriptor, context.locale);
    return hint(name ?? (subject.agent.harness_kind === undefined ? '' : context.t('resources.harnessNameUnknown')));
  } },
  skill: { hint: (subject: Extract<ResourceSubject, { kind: 'skill' }>, context: FactContext) => subject.skill.enabled === false ? hint(context.t('resourceFacts.unavailable'), true) : hint(subject.skill.latest === undefined ? '' : subject.skill.latest === null ? context.t('resourceFacts.noVersion') : context.t('resourceFacts.version', { value: formatNumber(subject.skill.latest.number, context.locale) })) },
};

export function resourceHint(subject: ResourceSubject, context: FactContext): Hint {
  switch (subject.kind) {
    case 'provider': return resourceDefinitions.provider.hint(subject, context);
    case 'model': return resourceDefinitions.model.hint(subject, context);
    case 'host': return resourceDefinitions.host.hint(subject, context);
    case 'environment': return resourceDefinitions.environment.hint(subject, context);
    case 'mount': return resourceDefinitions.mount.hint(subject, context);
    case 'mcp_server': return resourceDefinitions.mcp_server.hint(subject, context);
    case 'template': return resourceDefinitions.template.hint(subject, context);
    case 'agent': return resourceDefinitions.agent.hint(subject, context);
    case 'skill': return resourceDefinitions.skill.hint(subject, context);
  }
}

export function resourceName(subject: ResourceSubject): string {
  switch (subject.kind) {
    case 'provider': return subject.provider.name;
    case 'host': return subject.host.name;
    case 'environment': return subject.environment.name;
    case 'model': return subject.model.display_name || subject.model.model_name;
    case 'mount': return subject.mount.name;
    case 'mcp_server': return subject.server.name;
    case 'template': return subject.template.name;
    case 'agent': return subject.agent.name;
    case 'skill': return subject.skill.name;
  }
}

export function resourceAddress(subject: ResourceSubject): string {
  switch (subject.kind) {
    case 'model': return `/resources/providers/${subject.providerId}?tab=models`;
    case 'provider': return `/resources/providers/${subject.provider.id}`;
    case 'host': return `/resources/hosts/${subject.host.id}`;
    case 'environment': return `/resources/environments/${subject.environment.id}`;
    case 'mount': return `/resources/mounts/${subject.mount.id}`;
    case 'mcp_server': return `/resources/mcp/${subject.server.id}`;
    case 'template': return `/resources/templates/${subject.template.id}`;
    case 'agent': return `/resources/agents/${subject.agent.id}`;
    case 'skill': return `/resources/skills/${subject.skill.id}`;
  }
}

/** A server-projected resource name uses its existing card without reading a second name. */
export function resourceSubjectOf(object: Pick<ObjectRef,'kind'|'id'>, name: string): ResourceSubject | null {
  const named = { id: object.id, name };
  switch (object.kind) {
    case 'provider': return { kind: 'provider', provider: named };
    case 'agent': return { kind: 'agent', agent: named };
    case 'environment': return { kind: 'environment', environment: named };
    case 'host': return { kind: 'host', host: named };
    case 'template': return { kind: 'template', template: named };
    case 'mount': return { kind: 'mount', mount: named };
    case 'mcp_server': return { kind: 'mcp_server', server: named };
    case 'skill': return { kind: 'skill', skill: named };
    default: return null;
  }
}

