/** What the tests of starting a thread build their answers from: a provider, a profile, an arrangement, a model and an environment as the server would list them, and the answer that holds them. */
import type { StartOptions } from '@/api/start';
import type { Arrangement, ArrangementType, BindingAgent, BindingEnvironment, BindingProvider, ModelOption, PlaceEnvironment, PlaceHost } from '@/generated/api-types';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { harnessName } from '../doubles/harness-names';

const OWNER = { id: 'u1', kind: 'human', username: 'ada', display_name: 'Ada Lovelace', avatar: { text: 'A', color: 1 } } as const;


type PlacementFacts = Pick<PlaceHost, 'availability' | 'home' | 'home_path' | 'origin_machine'>;
/** These fixtures report no per-machine judgement or stored origin; each row owns fresh lists. */
function placementFacts(): PlacementFacts {
  return { availability: [], home: null, home_path: [], origin_machine: null };
}

export function arrangement(type: ArrangementType, rank: number, open = true, why: string | null = null): Arrangement {
  return {
    type,
    executor: null,
    tool_set: null,
    isolation: {
      type,
      harness_runs_in: type === 'separated' ? 'provider' : 'environment',
      executor: null,
      tools: type === 'separated' ? 'executor' : 'own',
      tool_set: null,
      orch_tools: { offered: true, reason: null },
      credential_path: type === 'separated' ? 'native' : 'forwarded',
      held_by: 'provider',
      form: null,
      lifetime_secs: null,
      model_enforcement: 'adapter',
      metering: 'harness',
    },
    rank,
    // What is not allowed can be had and is not permitted; what has no executor to work through cannot be had at all.
    available: open || why === 'arrangement_not_allowed',
    permitted: open || why !== 'arrangement_not_allowed',
    reason: open ? null : why,
    not_allowed_by: why === 'arrangement_not_allowed' ? 'provider' : null,
    scope: null,
    detail: open ? null : `${why ?? 'closed'} detail`,
  };
}

export function agent(id: string, options: { reason?: string | null; arrangements?: Arrangement[] } = {}): BindingAgent {
  const reason = options.reason ?? null;
  return {
    ...placementFacts(),
    owner: null,
    agent: { id, name: `Agent ${id}`, address: `acme.${id}`, disabled: false, harness_kind: 'codex', harness: harnessName('codex'), machine: null },
    usable: reason === null,
    reason,
    arrangements: options.arrangements ?? (reason === null ? [arrangement('separated', 1), arrangement('key_separated', 2)] : []),
  };
}

export function model(name: string, efforts: string[] = []): ModelOption {
  return { ...MODEL_ESTIMATE, allowed: true, display_name: name === 'plain' ? '' : name.toUpperCase(), model_name: name, parameters: efforts.length === 0 ? {} : { efforts, default_effort: efforts[0] ?? null }, reason: null };
}

export function provider(id: string, options: { usable?: boolean; reason?: string | null; status?: 'ready' | 'exhausted'; agents?: BindingAgent[]; models?: ModelOption[] } = {}): BindingProvider {
  return {
    ...placementFacts(),
    owner: null,
    provider: { id, name: `Provider ${id}`, address: `acme.${id}`, harness_kind: null, harness: null, kind: 'api', placement: 'managed', status: options.status ?? 'ready' },
    usable: options.usable ?? true,
    reason: options.reason ?? null,
    models: options.models ?? [model('m1', ['low', 'high']), model('m2')],
    agents: options.agents ?? [agent('a1'), agent('a2')],
  };
}

export function environment(id: string, options: { isDefault?: boolean; usable?: boolean } = {}): BindingEnvironment {
  return {
    ...placementFacts(),
    owner: OWNER,
    environment: { id, name: `Env ${id}`, address: `acme.${id}`, owner: OWNER, ports_exposed: false, placement: 'managed', status: 'running' },
    host: null,
    machine: null,
    threads: 0,
    usable: options.usable ?? true,
    reason: options.usable === false ? 'disabled' : null,
    is_default: options.isDefault ?? false,
    used: 'unused',
    topics: [],
    topics_hidden: 0,
    directory: '',
  };
}

/** A synthetic Linux placement; callers supply any measured capacity or recorded origin explicitly. */
export function placeEnvironment(id: string, more: Partial<PlaceEnvironment> = {}): PlaceEnvironment {
  return {
    ...placementFacts(),
    environment: environment(id).environment,
    host: null,
    machine: null,
    facts: { os: 'linux', cpus: null, memory_mb: null },
    threads: 0,
    topics: [],
    topics_hidden: 0,
    mounts: [],
    usable: true,
    reasons: [],
    current: false,
    ...more,
  };
}

export function host(id = 'h1', name = 'Available host', more: Partial<PlaceHost> = {}): PlaceHost {
  return {
    ...placementFacts(),
    host: { id, name, address: `acme.${id}` }, owner: null, labels: [], capacity: { cpu_millis: 4000, memory_mb: 8192, disk_gb: 100 },
    declared: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 10 }, free: { cpu_millis: 3000, memory_mb: 6144, disk_gb: 90 },
    used: { cpu_percent: 5, memory_mb: 1024, disk_gb: 8 }, environments: { running: 1, idle: 0 }, storages: [], mounts: [], when_full: 'warn',
    usable: true, reasons: [], warnings: [], current: false, automatic: false, ...more,
  };
}

/** A healthy default offer delegates to the one complete host builder. */
export function creationHost(): PlaceHost {
  return host('h1', 'Build host', { automatic: true, capacity: { cpu_millis: 8000, memory_mb: 16384, disk_gb: 64 },
    declared: { cpu_millis: 0, memory_mb: 0, disk_gb: 0 }, free: { cpu_millis: 8000, memory_mb: 16384, disk_gb: 64 },
    used: { cpu_percent: null, memory_mb: null, disk_gb: null }, environments: { running: 0, idle: 0 }, host: { id: 'h1', name: 'Build host', address: 'acme.build' } });
}

export function options(more: Partial<StartOptions> = {}): StartOptions {
  const summary = { custom: false, from: null, interact: [], manage: [], view: [] };
  return {
    viewer_default: {value:false,source:{scope:'default',node:null}},
    defaults: { provider_id: 'p1', agent_id: 'a1', model_name: 'm1', effort: null, locked: false },
    providers: [provider('p1'), provider('p2')],
    environments: [environment('e1', { isDefault: true }), environment('e2')],
    environment_default: 'shared',
    access: { managed: summary, connected: summary },
    new_environment: summary,
    can_create: { allowed: true, missing: [] },
    hosts: [creationHost()],
    machines: [],
    source: { repository_url: null, branch: null, secret_name: null, from: null },
    ...more,
  };
}
