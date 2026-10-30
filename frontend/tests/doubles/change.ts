/** What the fake API says of changing where and on what work runs: the plans of a fork, a switch, several switches, an update, a move and a recycle, the jobs that carry them out (and undo themselves when they fail or are stopped), and the side threads of a thread.
 *
 * Every plan is worked out from the world as it stands: what a thread runs on is the resources' to say (`Resources.bindingOf`), who may
 * use what is theirs, and a plan names what would change, what stays and what ends in the shapes the real API gives. A plan is kept
 * for a quarter of an hour. Carrying one out makes a job (one for each plan of a batch) whose lock on a thread or an environment is
 * shown wherever it is read: the row of the thread says it is held, the environment says it is being replaced.
 *
 * A job advances by itself, one step in a moment (`auto`), or only when a test says (`manual`: `/__double/change/step`), which is how
 * a test looks at each phase. A job fails, where a test says it will (`/__double/change/fail`), and is stopped by a cancel until its
 * commit; what it made is then taken away and the thread is as it was. The controls of a test are under `/__double/change/...`.
 */
import { randomUUID } from 'node:crypto';

import type {
  AgentRef,
  AppliedChangeOptions,
  ArrangementType,
  EnvironmentMovePlan,
  MachineEnvironmentData,
  MoveMachine,
  EnvironmentRef,
  EnvironmentUpdatePlan,
  Estimate,
  FilesPlan,
  FleetEntry,
  ForkBatch,
  ForkBatchPlan,
  ForkPlan,
  HistoryMethod,
  HistoryReport,
  HostRef,
  Item,
  Job,
  JobRef,
  MissingAccess,
  NodeRef,
  PendingUpdate,
  PlaceHost,
  PlanBlocker,
  PlanMount,
  ProviderRef,
  RecycleReply,
  Segment,
  Thread,
  ThreadInput,
  ThreadSync,
  Turn,
  UserRef,
} from '../../src/generated/api-types.ts';
import { ACME, id as fixtureId, KIND, RUNS_ON } from './fixtures.ts';
import { marker, userMessage } from './thread-rows.ts';
import { type Answer, type Part, type Person, Refusal, type Subscriber, type World } from './world.ts';

// ---------------------------------------------------------------- the numbers of the double

/** How long a plan stays good: a quarter of an hour, as the real system keeps one. */
const PLAN_TTL_MS = 15 * 60_000;
/** How long one step of a job takes when the double steps by itself: long enough to be seen. */
const STEP_MS = 450;
/** How many steps a phase takes. */
const STEPS_PER_PHASE = 2;
/** How many jobs of one batch run at once; the others wait their turn, their threads already locked. */
const BATCH_PARALLEL = 2;
/** How many side threads a thread may have open. */
const SIDE_MAX = 4;
/** How many threads one batch plans, at the most. */
/** How long the agent of a side thread takes to answer. */
const SIDE_ANSWER_MS = 700;
/** The share of a host's memory that its units declare, from which it counts as full. */
const FULL_PERCENT = 90;

const GB = 1_073_741_824;
const MB = 1_048_576;

const WEIGHTS: Record<string, number> = { notify: 5, lock: 5, provision: 15, copy: 50, history: 10, replace: 45, verify: 10, commit: 5, cleanup: 5 };

/** What each phase says beside the bar, in the double's English; the page has its own words for the names. */
const PHASE_LINES: Record<string, string> = {
  notify: 'telling the threads',
  lock: 'waiting for the running turn to end',
  provision: 'preparing the new side',
  copy: 'copying files',
  history: 'carrying the conversation over',
  replace: 'starting the new unit',
  verify: 'checking the new side',
  commit: 'making the new side the real one',
  cleanup: 'removing what was left behind',
};

/** The checks of the `verify` phase, which the double prints one by one into the job's log. */
const CHECKS = ['environment attached', 'files present', 'mounts as planned', 'session open with its history', 'tools reach it', 'model usable'];

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** What a move plan request asks of one mount, as the double reads the body. */
function askedMount(body: Record<string, unknown>, id: string): { fate: PlanMount['fate']; into: 'replace' | 'new_location' | null } | null {
  const asked: unknown = Array.isArray(body.mounts) ? body.mounts.find((entry: unknown) => isRecord(entry) && entry.mount_id === id) : undefined;
  if (!isRecord(asked) || (asked.fate !== 'detach' && asked.fate !== 'as_is' && asked.fate !== 'copy')) return null;
  return { fate: asked.fate, into: asked.into === 'replace' || asked.into === 'new_location' ? asked.into : null };
}

/** A mount of a move plan as the server answers it: Copy offered between hosts, measured when chosen, landing in a new
 * location named for the target, since the target has none. */
function planMount(mount: { id: string; name: string; address: string }, located: boolean, path: string, mode: 'private' | 'read_only', fallback: PlanMount['fate'],
  asked: ReturnType<typeof askedMount>, betweenHosts: boolean): PlanMount {
  const fates: PlanMount['fates'] = betweenHosts ? ['detach', 'as_is', 'copy'] : ['as_is'];
  const fate = asked !== null && fates.includes(asked.fate) ? asked.fate : fallback;
  const attached = fate === 'copy' || (fate === 'as_is' && located);
  return {
    mount, action: attached ? 'keep' : 'detach', path: attached ? path : null, mode: attached ? mode : null,
    detached_because: attached ? null : fate === 'detach' ? 'asked' : 'no_location',
    fate, default_fate: fallback, fates, copy_unavailable: betweenHosts ? null : 'unsupported',
    copy: fate === 'copy' ? { measured: true, file_count: 1_200, total_bytes: 2 * 1_073_741_824, location: { made: true, kind: 'volume', source: `orch-mount-${mount.id}-host2`, storage: null }, holds: null, into: asked?.into ?? null, replace: false } : null,
    data_kind: 'own', fingerprints_available: false, fingerprints: null,
  };
}
const asText = (value: unknown): string => (typeof value === 'string' ? value : '');
const asList = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
const iso = (ms: number): string => new Date(ms).toISOString();

type ThreadBindingLike = ReturnType<World['resources']['bindingOf']>;

/** What a request asks for once it is read: the members left out are what the source has. */
interface Asked {
  result: 'new' | 'replace';
  provider: string;
  agent: string;
  model: string;
  effort: string | null;
  arrangement: ArrangementType;
  providerChanged: boolean;
  agentChanged: boolean;
  environment: { kind: 'same' } | { kind: 'use'; id: string; files: Record<string, unknown> } | { kind: 'new'; spec: Record<string, unknown> };
  directory: string;
  history: { mode: 'all' | 'range' | 'summary' | 'none'; from: number | null; to: number | null };
  memory: boolean | null;
  name: string | null;
  topic: string | null;
  access: unknown;
  options: AppliedChangeOptions;
}

interface Stored {
  kind: 'fork' | 'batch' | 'update' | 'move' | 'node-update' | 'node-move' | 'provider-update' | 'provider-updates';
  by: string;
  expiresAt: number;
  /** What carrying it out needs: the thread, the threads, the environment, the provider. */
  subject: string;
  plan: unknown;
  asked?: Asked;
  askedFor?: Record<string, unknown>;
}

interface Failure {
  phase: string;
  code: string;
  message: string;
  undone: boolean;
}

type JobKind = 'thread_fork' | 'environment_recreate' | 'environment_move' | 'provider_recreate';

interface JobRecord {
  id: string;
  kind: JobKind;
  by: Person;
  phases: { name: string; weight: number }[];
  at: number;
  step: number;
  state: Job['state'];
  error: Job['error'];
  result: unknown;
  createdAt: number;
  startedAt: number | null;
  finishedAt: number | null;
  batch: string | null;
  thread: string | null;
  environment: string | null;
  provider: string | null;
  failure: Failure | null;
  cancelled: boolean;
  committed: boolean;
  log: string[];
  /** What changes in the world when the job begins, at its commit, when it is undone and when it is over. */
  effects: { begin?: () => void; commit?: () => void; undo?: () => void; over?: () => void };
  /** The bytes the copy phase moves, where it has one. */
  bytes: number;
  files: number;
  /** The threads this job holds, and what each stood as before: put back if the job does not commit. */
  threadsLocked: string[];
  before: Map<string, { state: Thread['state']; blocked: Thread['blocked'] }>;
}

interface BatchRecord {
  id: string;
  by: string;
  jobs: string[];
  hidden: Set<string>;
  /** The one environment shared by this batch's accepted new-environment plans. */
  environment?: string;
}

/** A thread that is a side of another: its own small conversation, kept here because it is not in the tree. */
interface SideRecord {
  parent: string;
  thread: Thread;
  maker: Person;
  state: 'open' | 'closed';
  closedWhy: 'closed' | 'idle' | 'kept' | 'parent' | null;
  keptAs: string | null;
  items: Map<string, Item>;
  turns: Map<string, Turn>;
  inputs: Map<string, ThreadInput>;
  segments: Segment[];
  seq: number;
  clientIds: Map<string, string>;
  followers: Map<Subscriber, { id: number; last: number; user: string }>;
}

/** Explicit wire tags stay beside the shared child schemas, as on the native endpoint. */
type MovePlace = (PlaceHost & { kind: 'host' }) | (MoveMachine & { kind: 'machine' });

export class Change implements Part {
  readonly world: World;
  private plans = new Map<string, Stored>();
  private jobs = new Map<string, JobRecord>();
  private measurements = new Map<string, Job>();
  private batches = new Map<string, BatchRecord>();
  private sides = new Map<string, SideRecord>();
  private timers = new Set<NodeJS.Timeout>();
  /** Whether jobs advance by themselves (`auto`) or only when a test steps them (`manual`). */
  mode: 'auto' | 'manual' = 'auto';
  /** The failure the next job that reaches that phase ends with. */
  private failing: Failure | null = null;
  /** Threads the caller is taken not to be able to see: counted and not named. */
  private hidden = new Set<string>();
  /** How many minutes each environment has been idle, for a recycle. */
  private idle = new Map<string, number>();
  /** What each host declares its units need, in megabytes, where a test says it is more than the units themselves declare. */
  private declared = new Map<string, number>();
  /** A turn that does not end: the job waits for it. */
  private jobNumber = 0;
  /** How far the clock that dates the plans is ahead of the real one, in milliseconds: a test that moves the page's clock moves this by as much. */
  private skewMs = 0;

  constructor(world: World) {
    this.world = world;
    // What the environments' pages say of a job under way and of how many threads are bound where comes from here.
    world.resources.jobRefOf = (environmentId: string) => this.refOfEnvironment(environmentId);
    world.resources.unseenThread = (threadId: string) => this.hidden.has(threadId);
  }

  reset(): void {
    for (const timer of this.timers) clearTimeout(timer);
    this.timers.clear();
    for (const side of this.sides.values()) for (const subscriber of side.followers.keys()) subscriber.close(1012);
    this.plans.clear();
    this.jobs.clear();
    this.measurements.clear();
    this.batches.clear();
    this.sides.clear();
    this.mode = 'auto';
    this.failing = null;
    this.hidden.clear();
    this.idle.clear();
    this.declared.clear();
    this.jobNumber = 0;
    this.skewMs = 0;
  }

  /** The moment, as the plans are dated by it. */
  private now(): number {
    return Date.now() + this.skewMs;
  }

  // ---------------------------------------------------------------- shorthands over the rest of the world

  private get resources() {
    return this.world.resources;
  }

  private threadRef(threadId: string, redacted = false): NodeRef {
    const side = this.sides.get(threadId);
    if (redacted) return { id: threadId, kind: 'thread', name: null, redacted: true };
    return { id: threadId, kind: 'thread', name: side?.thread.name ?? this.world.places.rowOf(threadId)?.name ?? 'Thread', redacted: false };
  }

  private nodeRef(nodeId: string): NodeRef {
    const row = this.world.places.rowOf(nodeId);
    return { id: nodeId, kind: row?.kind ?? 'topic', name: row?.name ?? null, redacted: false };
  }

  private user(username: string): UserRef {
    return this.world.places.userOfName(username);
  }

  private provider(providerId: string) {
    return this.resources.providers.find((record) => record.id === providerId);
  }

  private agent(agentId: string) {
    return this.resources.agents.find((record) => record.id === agentId);
  }

  private environment(environmentId: string) {
    return this.resources.environments.find((record) => record.id === environmentId);
  }

  private host(hostId: string) {
    return this.resources.hosts.find((record) => record.id === hostId);
  }

  private providerRef(providerId: string): ProviderRef {
    const ref = this.resources.refsOf({ provider: providerId, agent: '', environment: '', model: '', effort: null, directory: '' }).provider;
    if (ref === null) throw new Refusal(404, 'not_found', 'there is no such provider');
    return ref;
  }

  private agentRef(agentId: string): AgentRef {
    const ref = this.resources.refsOf({ provider: '', agent: agentId, environment: '', model: '', effort: null, directory: '' }).agent;
    if (ref === null) throw new Refusal(404, 'not_found', 'there is no such agent profile');
    return ref;
  }

  private environmentRef(environmentId: string): EnvironmentRef {
    const ref = this.resources.refsOf({ provider: '', agent: '', environment: environmentId, model: '', effort: null, directory: '' }).environment;
    if (ref === null) throw new Refusal(404, 'not_found', 'there is no such environment');
    return ref;
  }

  private hostRef(hostId: string): HostRef {
    const record = this.host(hostId);
    if (record === undefined) return { id: hostId, name: 'host', address: 'system.host' };
    return { id: hostId, name: record.name, address: record.home === null ? `system.${record.name}` : `acme.${record.owner ?? 'ada'}.${record.name}` };
  }

  /**
   * The one step of a permission page that a switch takes: the use of a provider given to the work in a place, which gives it to the
   * people who talk to the threads there. Only a manager of the provider may take it; the page is no other part's business here, so
   * what is answered is what the wizard reads of it (nothing).
   */
  private openProvider(person: Person, providerId: string, body: Record<string, unknown>): unknown {
    const provider = this.provider(providerId);
    if (provider === undefined) throw new Refusal(404, 'not_found', 'there is no such provider');
    if (!(person.me.is_system_admin || provider.owner === person.me.username || provider.manages.includes(person.me.username))) throw new Refusal(403, 'authz.denied', 'you may not give the use of this provider');
    for (const row of asList(body.rows)) {
      if (!isRecord(row) || row.permission !== 'provider.use') continue;
      for (const add of asList(row.add)) {
        const holder = isRecord(add) && isRecord(add.holder) ? add.holder : null;
        if (holder?.kind !== 'node' || typeof holder.id !== 'string') continue;
        for (const entry of this.world.people.values()) {
          if (this.world.places.access(entry, holder.id).permissions.some((permission) => permission.endsWith('.interact')) && !provider.uses.includes(entry.me.username)) provider.uses.push(entry.me.username);
        }
      }
    }
    return { rows: [] };
  }

  private maySee(threadId: string): boolean {
    return !this.hidden.has(threadId);
  }

  /** Whether the person manages an environment: they own it, it names them, or they administer the system. */
  private mayManage(person: Person, environmentId: string): boolean {
    const record = this.environment(environmentId);
    return record !== undefined && (record.owner === person.me.username || record.manages.includes(person.me.username) || person.me.is_system_admin);
  }

  private mayUse(person: Person, providerId: string): boolean {
    const record = this.provider(providerId);
    return record !== undefined && (record.owner === person.me.username || record.uses.includes(person.me.username) || person.me.is_system_admin);
  }

  private missing(permission: string, object: { kind: string; id: string; name: string; owner: string | null }): MissingAccess {
    const owner = this.user(object.owner ?? 'ada');
    return { ask: [owner], ask_more: 0, override: false, closed: null, object: { id: object.id, kind: object.kind, name: object.name, owner }, options: null, permissions: [permission] };
  }

  /** The thread as the person sees it, whatever kind it is. */
  private threadOf(person: Person, threadId: string): Thread {
    const side = this.sides.get(threadId);
    if (side !== undefined) {
      const parent = this.threadOf(person, side.parent);
      return { ...side.thread, seq: side.seq, access: parent.access, input_access: parent.input_access,
        environment_permissions: parent.environment_permissions,
        can: { archive: false, place: false, export: parent.can.export, fork: parent.can.fork } };
    }
    const row = this.world.places.rowOf(threadId);
    if (row?.kind !== 'thread' || row.deleted) throw new Refusal(404, 'not_found', 'there is no such thread');
    this.world.places.need(person, threadId, 'thread.view');
    return this.world.threads.thread(person, row);
  }

  private bindingOfAny(threadId: string): ThreadBindingLike {
    const side = this.sides.get(threadId);
    return this.resources.bindingOf(side === undefined ? threadId : side.parent);
  }

  // ---------------------------------------------------------------- reading a request

  private options(raw: unknown): AppliedChangeOptions {
    const given = isRecord(raw) ? raw : {};
    const number = (value: unknown, otherwise: number) => (typeof value === 'number' ? value : otherwise);
    return {
      active_turn: given.active_turn === 'interrupt' ? 'interrupt' : 'wait',
      keep_previous: given.keep_previous === true,
      stop_processes: given.stop_processes !== false,
      verify_timeout_secs: number(given.verify_timeout_secs, 120),
      wait_minutes: number(given.wait_minutes, 10),
    };
  }

  private ask(request: Record<string, unknown>, bound: ThreadBindingLike): Asked {
    const result = request.result === 'new' ? 'new' : 'replace';
    const to = isRecord(request.to) ? request.to : {};
    const provider = asText(to.provider_id) === '' ? bound.provider : asText(to.provider_id);
    const agent = asText(to.agent_id) === '' ? bound.agent : asText(to.agent_id);
    const model = asText(to.model) === '' ? bound.model : asText(to.model);
    const effort = 'effort' in to ? (typeof to.effort === 'string' ? to.effort : null) : bound.effort;
    const arrangement = (asText(to.arrangement) === '' ? 'separated' : asText(to.arrangement)) as ArrangementType;
    const environment = isRecord(request.environment) ? request.environment : { same: true };
    let where: Asked['environment'] = { kind: 'same' };
    if ('use' in environment) where = { kind: 'use', id: asText(environment.use), files: isRecord(environment.files) ? environment.files : { mode: 'all' } };
    else if ('new' in environment && isRecord(environment.new)) where = { kind: 'new', spec: environment.new };
    const history = isRecord(request.history) ? request.history : {};
    const mode = ['all', 'range', 'summary', 'none'].includes(asText(history.mode)) ? (asText(history.mode) as Asked['history']['mode']) : 'all';
    return {
      result,
      provider,
      agent,
      model,
      effort,
      arrangement,
      providerChanged: provider !== bound.provider,
      agentChanged: agent !== bound.agent,
      environment: where,
      directory: asText(request.directory) === '' ? bound.directory : asText(request.directory),
      history: { mode, from: typeof history.from_turn === 'number' ? history.from_turn : null, to: typeof history.to_turn === 'number' ? history.to_turn : null },
      memory: typeof request.memory === 'boolean' ? request.memory : null,
      name: asText(request.name) === '' ? null : asText(request.name),
      topic: asText(request.topic_id) === '' ? null : asText(request.topic_id),
      access: request.access ?? null,
      options: this.options(request.options),
    };
  }

  // ---------------------------------------------------------------- the pieces of a plan

  private estimate(seconds: number, assumed: boolean): Estimate {
    return { basis: assumed ? 'assumed' : 'measured', seconds_low: Math.round(seconds), seconds_high: Math.round(seconds * 2) };
  }

  /** A deterministic report of what a history holds, from the items the thread has, or from a plausible conversation when it has none. */
  private report(threadId: string, method: HistoryMethod, upTo: number | null): HistoryReport {
    const held = this.world.threads.snapshot(threadId) as { items: Item[] };
    const counts: Record<string, number> = {};
    for (const item of held.items.filter((entry) => entry.stream === 'main')) counts[item.kind] = (counts[item.kind] ?? 0) + 1;
    const seeded = Object.keys(counts).length === 0;
    const base = seeded ? { user_message: 6, agent_message: 7, reasoning: 9, tool_call: 21, plan: 2, approval: 3 } : counts;
    const by_kind: HistoryReport['by_kind'] = {};
    let total = 0;
    const dropped: HistoryReport['dropped'] = [];
    const warnings: string[] = [];
    for (const [kind, count] of Object.entries(base)) {
      const n = upTo === null ? count : Math.max(1, Math.ceil(count * Math.min(1, upTo / 8)));
      total += n;
      if (method === 'native_transplant' || method === 'native_fork' || method === 'same_session') by_kind[kind] = { kept: n, converted: 0, dropped: 0 };
      else if (method === 'summary_handoff') by_kind[kind] = { kept: 0, converted: n, dropped: 0 };
      else if (method === 'none') by_kind[kind] = { kept: 0, converted: 0, dropped: n };
      else if (kind === 'reasoning') {
        by_kind[kind] = { kept: 0, converted: 0, dropped: n };
        dropped.push({ count: n, kind, reason: 'the other harness keeps no reasoning of this kind' });
      } else if (kind === 'tool_call') {
        const convertedCount = Math.ceil(n * 0.8);
        by_kind[kind] = { kept: 0, converted: convertedCount, dropped: n - convertedCount };
        if (n - convertedCount > 0) dropped.push({ count: n - convertedCount, kind, reason: 'a tool the other harness does not have' });
      } else by_kind[kind] = { kept: n, converted: 0, dropped: 0 };
    }
    if (method === 'neutral_import') warnings.push('Tool calls are carried over as text the new agent can read.');
    return { by_kind, dropped, fidelity: method === 'summary_handoff' || method === 'none' ? 'summary' : method === 'neutral_import' ? 'structured' : 'lossless', items_total: total, warnings };
  }

  private methodFor(asked: Asked, sameHarness: boolean, firstTurn: boolean): HistoryMethod {
    if (asked.history.mode === 'none') return 'none';
    if (asked.history.mode === 'summary') return 'summary_handoff';
    if (asked.result === 'replace' && !asked.providerChanged && !asked.agentChanged) return 'same_session';
    if (asked.result === 'new' && !asked.providerChanged && !asked.agentChanged && firstTurn) return 'native_fork';
    if (sameHarness && firstTurn) return 'native_transplant';
    return 'neutral_import';
  }

  private harnessOf(agentId: string): string {
    return this.agent(agentId)?.harness ?? '';
  }

  private filesFor(selection: Record<string, unknown>, moves: boolean, home: boolean, privateViews: boolean): FilesPlan | null {
    const mode = asText(selection.mode);
    const none = mode === 'none';
    const some = mode === 'selection';
    const count = none ? 0 : some ? 38 : 1_240;
    const bytes = none ? 0 : some ? 41 * MB : Math.round(3.1 * GB);
    if (!moves) return null;
    return {
      bytes_to_download: bytes,
      bytes_to_upload: Math.round(bytes * 0.4),
      file_count: count,
      home: home ? { measured: true, file_count: 312, total_bytes: 160 * MB } : null,
      file_states: null,
      missing: { bytes: null, count: 0, sample: [] },
      path_map: { from: '/work/login', to: '/workspace/login' },
      private_views: privateViews ? [{ measured: true, carried: true, left_because: null, file_count: 4, mount: { id: fixtureId(0x2b, 1), name: 'src', address: 'acme.ada.src' }, total_bytes: 120_000 }] : [],
      source: 'live',
      source_as_of: iso(Date.now()),
      total_bytes: bytes,
    };
  }

  private placeHost(host: NonNullable<ReturnType<Change['host']>>, current: boolean, needsMb = 2_048): PlaceHost {
    const running = this.resources.environments.filter((environment) => environment.host === host.id && environment.status === 'running').length;
    const idle = this.resources.environments.filter((environment) => environment.host === host.id && environment.status !== 'running').length;
    const capacity = { cpu_millis: 16_000, memory_mb: 65_536, disk_gb: 1_000 };
    const own = (running + idle) * 4_096;
    const declaredMb = this.declared.get(host.id) ?? own;
    const declared = { cpu_millis: 2_000 * (running + idle), memory_mb: declaredMb, disk_gb: 20 * (running + idle) };
    const free = { cpu_millis: capacity.cpu_millis - declared.cpu_millis, memory_mb: Math.max(0, capacity.memory_mb - declaredMb), disk_gb: capacity.disk_gb - declared.disk_gb };
    const full = declaredMb >= (capacity.memory_mb * FULL_PERCENT) / 100;
    const noRoom = free.memory_mb < needsMb;
    const blocks = host.whenFull === 'block';
    const reasons: PlaceHost['reasons'] = [];
    if (!host.schedulable) reasons.push('unschedulable');
    if (blocks && full) reasons.push('full');
    else if (blocks && noRoom) reasons.push('no_room');
    const warnings: PlaceHost['warnings'] = [];
    if (!blocks && full) warnings.push('full');
    else if (!blocks && noRoom) warnings.push('no_room');
    return {
      automatic: false,
      capacity,
      current,
      declared,
      environments: { idle, running },
      free,
      host: this.hostRef(host.id),
      labels: host.labels,
      mounts: [
        { mount: { id: fixtureId(0x2b, 1), name: 'src', address: 'acme.ada.src' }, present: host.name === 'host-1' },
        { mount: { id: fixtureId(0x2b, 2), name: 'datasets', address: 'acme.ada.datasets' }, present: false },
      ],
      owner: host.owner === null ? null : this.user(host.owner),
      reasons,
      storages: [
        { name: 'fast', default: true, capacity_gb: 800, free_gb: 540, private_views: true },
        { name: 'bulk', default: false, capacity_gb: 4_000, free_gb: 3_100, private_views: false },
      ],
      usable: reasons.length === 0,
      used: { cpu_percent: 12, disk_gb: 60, memory_mb: 9_800 },
      warnings,
      when_full: host.whenFull ?? 'warn',
    };
  }

  private placeOf(person: Person, thread: string | null): unknown[] {
    const bound = thread === null ? null : this.bindingOfAny(thread);
    const hosts = this.resources.hosts
      .filter((host) => host.uses.includes(person.me.username) || host.owner === person.me.username || host.owner === null || person.me.is_system_admin)
      .map((host) => this.placeHost(host, bound !== null && this.environment(bound.environment)?.host === host.id));
    const first = hosts.find((host) => host.usable);
    if (first !== undefined) first.automatic = true;
    const environments = this.resources.environments
      .filter((record) => this.mayManage(person, record.id))
      .map((record) => {
        const bound = this.resources.refsOf({ provider: '', agent: '', environment: record.id, model: '', effort: null, directory: '' }).environment;
        const reasons: ('offline' | 'disabled' | 'removing')[] = record.status === 'offline' ? ['offline'] : record.status === 'disabled' ? ['disabled'] : record.removeAt === null ? [] : ['removing'];
        const topics = [...new Set([...this.resources.bindings].filter(([, binding]) => binding.environment === record.id).map(([thread]) => this.world.places.rowOf(thread)?.parent_id ?? ''))].filter((entry) => entry !== '');
        return {
          current: false,
          environment: bound!,
          facts: { cpus: record.machine === null ? null : 10, memory_mb: record.machine === null ? null : 20_000, os: record.machine === null ? 'linux' : (this.resources.machines.find((machine) => machine.id === record.machine)?.os ?? 'linux') },
          host: record.host === null ? null : this.hostRef(record.host),
          machine: record.machine === null ? null : (this.resources.refsOf({ provider: '', agent: '', environment: record.id, model: '', effort: null, directory: '' }).environment === null ? null : this.machineRefOf(record.machine)),
          mounts: [],
          reasons,
          threads: [...this.resources.bindings].filter(([, binding]) => binding.environment === record.id).length,
          topics: topics.map((topic) => this.nodeRef(topic)),
          topics_hidden: 0,
          usable: reasons.length === 0,
        };
      });
    for (const entry of environments) entry.current = bound !== null && entry.environment.id === bound.environment;
    return [...hosts, ...environments];
  }

  private machineRefOf(machineId: string) {
    const record = this.resources.machines.find((candidate) => candidate.id === machineId);
    return { id: machineId, name: record?.name ?? 'machine', online: record?.online ?? false, placement: 'connected' as const, platform_os: record?.os ?? 'linux' };
  }

  // ---------------------------------------------------------------- the plan of a fork or a switch

  /** Whether a job that has not ended holds a thread or an environment. */
  /** The job under way that holds a thread, an environment or a provider; the jobs of `ownBatch` do not count, for a batch does not hold its threads against itself. */
  private openJobFor(opts: { thread?: string | null; environment?: string | null; provider?: string | null }, ownBatch: string | null = null): JobRecord | undefined {
    return [...this.jobs.values()].find(
      (job) =>
        (job.state === 'queued' || job.state === 'running') &&
        (ownBatch === null || job.batch !== ownBatch) &&
        ((opts.thread !== undefined && opts.thread !== null && (job.thread === opts.thread || job.threadsLocked.includes(opts.thread))) ||
          (opts.environment !== undefined && opts.environment !== null && job.environment === opts.environment) ||
          (opts.provider !== undefined && opts.provider !== null && job.provider === opts.provider)),
    );
  }

  private draftFork(person: Person, threadId: string, request: Record<string, unknown>, redacted = false, ownBatch: string | null = null): { plan: ForkPlan; asked: Asked } {
    const side = this.sides.get(threadId);
    const thread = this.threadOf(person, threadId);
    const bound = this.bindingOfAny(threadId);
    const asked = this.ask(request, bound);
    if (side !== undefined && asked.result === 'replace') throw new Refusal(409, 'thread.side', 'a side thread is not switched, moved, shared or archived');
    if (thread.archived) throw new Refusal(409, 'node.archived', 'the thread is archived, and so is read-only');
    const blockers: PlanBlocker[] = [];
    const block = (code: string, message: string, missing?: MissingAccess[]) => blockers.push({ code, message, ...(missing === undefined ? {} : { missing }) });

    const providerRecord = this.provider(asked.provider);
    const agentRecord = this.agent(asked.agent);
    const sourceEnvironment = this.environment(bound.environment);
    let environmentId = bound.environment;
    let newEnvironment: { name: string; hostId: string | null } | null = null;
    if (asked.environment.kind === 'use') environmentId = asked.environment.id;
    if (asked.environment.kind === 'new') {
      newEnvironment = { name: asText(asked.environment.spec.name) === '' ? `${thread.name} environment` : asText(asked.environment.spec.name), hostId: typeof asked.environment.spec.host_id === 'string' ? asked.environment.spec.host_id : null };
    }
    const targetEnvironment = this.environment(environmentId);
    const environmentChanges = asked.environment.kind !== 'same' && !(asked.environment.kind === 'use' && asked.environment.id === bound.environment);
    const directoryChanges = asked.directory !== bound.directory;
    const changesProvider = asked.providerChanged || asked.agentChanged || asked.arrangement !== 'separated';

    // Who may do what: for what the request changes, and for nothing else.
    const owner = this.user(this.world.places.metaOf(threadId).owner);
    const manages = person.me.is_system_admin || this.world.places.access(person, side === undefined ? threadId : side.parent).permissions.includes('thread.manage');
    if (asked.result === 'replace') {
      if (changesProvider && !manages) block('access', 'you may not manage this thread', [this.missing('thread.manage', { kind: 'thread', id: threadId, name: thread.name, owner: owner.username })]);
      if (environmentChanges || directoryChanges) {
        if (sourceEnvironment !== undefined && !this.mayManage(person, sourceEnvironment.id)) {
          block('access', 'you may not decide where this thread runs', [this.missing('environment.manage', { kind: 'environment', id: sourceEnvironment.id, name: sourceEnvironment.name, owner: sourceEnvironment.owner })]);
        }
      }
    } else {
      const mayFork = manages || this.world.places.access(person, side === undefined ? threadId : side.parent).permissions.includes('thread.view');
      if (!mayFork) block('access', 'you may neither fork nor manage this thread', [this.missing('thread.fork', { kind: 'thread', id: threadId, name: thread.name, owner: owner.username })]);
    }
    if ((asked.result === 'new' || changesProvider) && providerRecord !== undefined && !this.mayUse(person, providerRecord.id) && (asked.result === 'new' || asked.providerChanged)) {
      block('access', `you may not use ${providerRecord.name}`, [this.missing('provider.use', { kind: 'provider', id: providerRecord.id, name: providerRecord.name, owner: providerRecord.owner })]);
    }
    if (asked.environment.kind === 'use' && targetEnvironment !== undefined && !this.mayManage(person, targetEnvironment.id)) {
      block('access', `you may not manage ${targetEnvironment.name}`, [this.missing('environment.manage', { kind: 'environment', id: targetEnvironment.id, name: targetEnvironment.name, owner: targetEnvironment.owner })]);
    }

    // What cannot be done.
    if (providerRecord === undefined) block('unavailable', 'the provider is not there');
    else if (providerRecord.status !== 'ready') block('provider_not_ready', `${providerRecord.name} is not ready: ${providerRecord.statusDetail ?? providerRecord.status}`);
    if (agentRecord === undefined || agentRecord.disabled) block('unavailable', 'the agent profile is not there or is switched off');
    else if (!this.resources.agentReported(agentRecord.id)) block('binding.incompatible', `${agentRecord.name} has not reported what it is, so a thread cannot be placed on it`);
    const model = providerRecord?.models.find((entry) => entry.model_name === asked.model);
    if (providerRecord !== undefined && !model?.enabled) block('model_not_allowed', `${asked.model} is not one of the models ${providerRecord.name} offers`);
    else if (model !== undefined && asked.effort !== null && !(model.parameters.efforts ?? []).includes(asked.effort)) block('effort_not_listed', `${asked.effort} is not an effort of ${asked.model}`);
    if (targetEnvironment === undefined && asked.environment.kind === 'use') block('unavailable', 'the environment is not there');
    if (targetEnvironment?.removeAt != null) block('removal_scheduled', `${targetEnvironment.name} is to be removed`); // eslint-disable-line eqeqeq
    if (asked.result === 'replace' && !changesProvider && !environmentChanges && !directoryChanges) {
      block('nothing_to_change', 'this would change nothing: model and effort alone are settings, and need no switch');
    }
    const running = this.openJobFor({ thread: threadId, environment: asked.result === 'replace' ? environmentId : null }, ownBatch);
    if (running !== undefined) block('already_running', 'a fork, a switch, a move or an update is under way for this thread or its environment');
    let place: PlaceHost | null = null;
    if (newEnvironment !== null) {
      const hosts = this.resources.hosts.filter((host) => host.schedulable || host.id === newEnvironment.hostId);
      const chosen = newEnvironment.hostId === null ? hosts.map((host) => this.placeHost(host, false)).find((candidate) => candidate.usable) : this.placeHost(this.host(newEnvironment.hostId) ?? hosts[0] ?? this.resources.hosts[0]!, false);
      if (chosen === undefined) block('no_host', 'no host takes a new environment');
      else if (!chosen.usable) block('no_capacity', `${chosen.host.name} takes nothing more now: ${Math.round(chosen.declared.memory_mb / 1024)} of ${Math.round(chosen.capacity.memory_mb / 1024)} GB declared`);
      else place = chosen;
    }

    const sameHarness = this.harnessOf(asked.agent) === this.harnessOf(bound.agent);
    const firstTurn = asked.history.from === null || asked.history.from <= 1;
    const method = this.methodFor(asked, sameHarness, firstTurn);
    const moves = environmentChanges || directoryChanges;
    const fileSelection = asked.environment.kind === 'use' ? asked.environment.files : asked.environment.kind === 'new' && isRecord(asked.environment.spec.files) ? asked.environment.spec.files : { mode: 'all' };
    const files = this.filesFor(fileSelection, moves && (asked.environment.kind !== 'same' || directoryChanges), asked.environment.kind === 'new' && asked.environment.spec.home === true, !(asked.environment.kind === 'new' && asked.environment.spec.private_views === false));
    const turnRunning = side === undefined && thread.state === 'running';
    const leaves = this.leaves(person, asked, sourceEnvironment, environmentChanges, threadId);

    const toEnvironment: ForkPlan['to']['environment'] =
      newEnvironment !== null && place !== null
        ? { new: { name: newEnvironment.name, place } }
        : newEnvironment !== null
          ? { new: { name: newEnvironment.name, place: this.placeHost(this.host(newEnvironment.hostId ?? '') ?? this.resources.hosts[0]!, false) } }
          : this.environmentRef(environmentId);

    // What the people on its page need to go on talking to it.
    const depends: ForkPlan['depends'] = [];
    if (changesProvider && providerRecord !== undefined) {
      const audience = [...this.world.people.values()].filter((entry) => entry.me.username !== owner.username && this.world.places.access(entry, side === undefined ? threadId : side.parent).permissions.includes('thread.interact'));
      const lacking = audience.filter((entry) => !(providerRecord.owner === entry.me.username || providerRecord.uses.includes(entry.me.username)));
      if (audience.length > 0) {
        const topic = this.world.places.rowOf(thread.topic_id);
        const project = topic?.parent_id === null || topic?.parent_id === undefined ? null : this.world.places.rowOf(topic.parent_id);
        depends.push({
          can_give: person.me.is_system_admin || providerRecord.owner === person.me.username || providerRecord.manages.includes(person.me.username),
          choice: null,
          for: 'thread.interact',
          object: { id: providerRecord.id, kind: 'provider', name: providerRecord.name, owner: this.user(providerRecord.owner) },
          open: lacking.length === 0 ? 'all' : lacking.length === audience.length ? 'none' : 'some',
          options: { model: asked.model, effort: asked.effort },
          permission: 'provider.use',
          through: project === null || project === undefined ? null : this.nodeRef(project.id),
        });
      }
    }

    const memory = moves && asked.history.mode !== 'none' ? { bytes: 38_000, carried: asked.memory ?? true, files: 14 } : null;
    const managedToConnected = sourceEnvironment?.machine === null && (targetEnvironment?.machine ?? null) !== null && environmentChanges;
    const differs: ForkPlan['access']['differs'] = managedToConnected
      ? [{ permission: 'thread.manage', before: [{ kind: 'role', role: 'developer', count: 3, at: this.nodeRef(thread.topic_id) }], after: [{ kind: 'user', user: owner }] }]
      : [];
    const seconds = 14 + (files === null ? 0 : (files.total_bytes / GB) * 20) + (asked.result === 'replace' && newEnvironment !== null ? 25 : 0);
    const plan: ForkPlan = {
      worktree: null,
      files_at: null,
      access: {
        differs,
        profile: targetEnvironment !== undefined && targetEnvironment.machine !== null ? 'connected' : 'managed',
        summary: { custom: false, from: this.nodeRef(thread.topic_id), interact: [{ kind: 'role', role: 'developer', count: 3, at: this.nodeRef(thread.topic_id) }], manage: [{ kind: 'user', user: owner }], view: [{ kind: 'everyone' }] },
      },
      blockers,
      depends,
      estimate: this.estimate(seconds, files !== null),
      expires_at: iso(this.now() + PLAN_TTL_MS),
      files,
      from: { agent: this.agentRef(bound.agent), environment: sourceEnvironment === undefined ? null : this.environmentRef(sourceEnvironment.id), provider: this.providerRef(bound.provider) },
      history: {
        archive_bytes: method === 'native_transplant' || method === 'native_fork' ? 4_200_000 : null,
        method,
        mode: asked.history.mode,
        report: this.report(side === undefined ? threadId : side.parent, method, asked.history.to),
        source_available: true,
        turns: asked.history.to ?? 8,
      },
      leaves: asked.result === 'replace' ? leaves : null,
      locks: asked.result === 'replace' ? [this.threadRef(threadId)] : [],
      memory,
      mounts: moves
        ? [
            { action: targetEnvironment !== undefined && targetEnvironment.machine !== null ? 'detach' : 'keep', mount: { id: fixtureId(0x2b, 1), name: 'src', address: 'acme.ada.src' }, path: targetEnvironment !== undefined && targetEnvironment.machine !== null ? null : '/mnt/src', mode: targetEnvironment !== undefined && targetEnvironment.machine !== null ? null : 'private', detached_because: targetEnvironment !== undefined && targetEnvironment.machine !== null ? 'no_location' : null,
              fate: 'as_is', default_fate: 'as_is', fates: ['as_is'], copy_unavailable: 'unchanged', copy: null, data_kind: null, fingerprints_available: false, fingerprints: null },
          ]
        : [],
      mcp: [],
      options: asked.options,
      owner: { after: asked.result === 'new' ? this.user(person.me.username) : owner, before: asked.result === 'new' ? null : owner },
      preflight_id: randomUUID(),
      result: asked.result,
      source: this.threadRef(threadId, redacted),
      to: {
        agent: this.agentRef(asked.agent),
        arrangement: asked.arrangement,
        directory: asked.directory === '' ? '/work' : `/work/${asked.directory}`,
        effort: asked.effort,
        environment: toEnvironment,
        isolation: {
          credential_path: 'native',
          executor: null,
          form: null,
          harness_runs_in: 'provider',
          held_by: 'provider',
          lifetime_secs: null,
          metering: 'wire',
          model_enforcement: 'wire',
          orch_tools: { offered: true, reason: null },
          tool_set: null,
          tools: 'own',
          type: asked.arrangement,
        },
        model: asked.model,
        predicted: !('id' in toEnvironment),
        provider: this.providerRef(asked.provider),
      },
      turn_running: turnRunning,
    };
    return { plan, asked };
  }

  private leaves(person: Person, asked: Asked, source: ReturnType<Change['environment']>, changes: boolean, threadId: string): ForkPlan['leaves'] {
    if (asked.result !== 'replace') return null;
    if (source === undefined || !changes) return { environment: null, session: true };
    const others = [...this.resources.bindings].filter(([thread, binding]) => thread !== threadId && binding.environment === source.id).length;
    const action: 'remove' | 'keep' | 'in_use' | 'not_removable' = source.machine !== null ? 'not_removable' : others > 0 ? 'in_use' : asked.options.keep_previous ? 'keep' : this.mayManage(person, source.id) ? 'remove' : 'keep';
    return { environment: { action, environment: this.environmentRef(source.id), processes: 2 }, session: true };
  }

  // ---------------------------------------------------------------- carrying a fork or a switch out

  private take(person: Person, id: string, kind: Stored['kind']): Stored {
    const stored = this.plans.get(id);
    if (stored?.kind !== kind || stored.by !== person.me.username) throw new Refusal(404, 'not_found', 'there is no such plan');
    if (stored.expiresAt <= this.now()) throw new Refusal(409, 'preflight.expired', 'the plan is older than fifteen minutes: make a new one');
    return stored;
  }

  private keep(person: Person, kind: Stored['kind'], subject: string, plan: unknown, extra: Partial<Stored> = {}): void {
    const id = (plan as { preflight_id: string }).preflight_id;
    this.plans.set(id, { kind, by: person.me.username, expiresAt: this.now() + PLAN_TTL_MS, subject, plan, ...extra });
  }

  private phasesOf(kind: JobKind, plan: { files?: unknown; history?: { method: HistoryMethod }; result?: string; to?: { environment: unknown }; leaves?: unknown }): string[] {
    if (kind === 'environment_recreate' || kind === 'provider_recreate') return ['notify', 'lock', 'replace', 'verify', 'commit'];
    if (kind === 'environment_move') return ['lock', 'provision', 'copy', 'verify', 'commit', 'cleanup'];
    const names = ['lock'];
    const newEnvironment = isRecord(plan.to?.environment) && 'new' in plan.to.environment;
    if (newEnvironment) names.push('provision');
    if (plan.files !== null && plan.files !== undefined) names.push('copy');
    if (plan.history?.method !== 'none') names.push('history');
    names.push('verify', 'commit');
    if (plan.result === 'replace') names.push('cleanup');
    return names;
  }

  private makeJob(person: Person, kind: JobKind, names: string[], subject: Partial<Pick<JobRecord, 'thread' | 'environment' | 'provider' | 'batch'>>, effects: JobRecord['effects'], size: { bytes: number; files: number }): JobRecord {
    const total = names.reduce((sum, name) => sum + (WEIGHTS[name] ?? 5), 0);
    const failure = this.failing;
    const record: JobRecord = {
      id: randomUUID(),
      kind,
      by: person,
      phases: names.map((name) => ({ name, weight: Math.round(((WEIGHTS[name] ?? 5) / total) * 100) })),
      at: 0,
      step: 0,
      state: 'queued',
      error: null,
      result: null,
      createdAt: Date.now(),
      startedAt: null,
      finishedAt: null,
      batch: subject.batch ?? null,
      thread: subject.thread ?? null,
      environment: subject.environment ?? null,
      provider: subject.provider ?? null,
      failure: failure !== null && names.includes(failure.phase) ? failure : null,
      cancelled: false,
      committed: false,
      log: [],
      effects,
      bytes: size.bytes,
      files: size.files,
      threadsLocked: [],
      before: new Map(),
    };
    if (record.failure !== null) this.failing = null;
    this.jobs.set(record.id, record);
    this.jobNumber += 1;
    return record;
  }

  /** Starts a job running: by itself, a step at a time, when the double is on its own; at a test's word otherwise. */
  private run(record: JobRecord): void {
    if (this.mode === 'manual') return;
    const timer = setTimeout(() => {
      this.timers.delete(timer);
      this.advance(record);
      if (record.state === 'queued' || record.state === 'running') this.run(record);
    }, STEP_MS);
    this.timers.add(timer);
  }

  /** The jobs of a batch that run at this moment. */
  private runningOf(batch: string | null): number {
    return batch === null ? 0 : [...this.jobs.values()].filter((job) => job.batch === batch && job.state === 'running').length;
  }

  private advance(record: JobRecord): void {
    if (record.state !== 'queued' && record.state !== 'running') return;
    if (record.state === 'queued') {
      if (record.batch !== null && this.runningOf(record.batch) >= BATCH_PARALLEL) return;
      record.state = 'running';
      record.startedAt = Date.now();
      record.effects.begin?.();
    }
    if (record.cancelled && !record.committed) {
      this.end(record, 'cancelled', null);
      return;
    }
    const phase = record.phases[record.at];
    if (phase === undefined) {
      this.end(record, 'succeeded', null);
      return;
    }
    if (record.failure !== null && record.failure.phase === phase.name && record.step + 1 >= STEPS_PER_PHASE) {
      this.end(record, 'failed', { code: record.failure.code, message: record.failure.message, phase: phase.name, undone: record.failure.undone });
      return;
    }
    record.step += 1;
    if (phase.name === 'verify') record.log = CHECKS.slice(0, Math.min(CHECKS.length, Math.ceil((record.step / STEPS_PER_PHASE) * CHECKS.length)));
    if (record.step >= STEPS_PER_PHASE) {
      record.at += 1;
      record.step = 0;
      if (phase.name === 'commit') {
        record.committed = true;
        record.effects.commit?.();
      }
      if (record.at >= record.phases.length) {
        this.end(record, 'succeeded', null);
        return;
      }
    }
    this.show(record);
  }

  private end(record: JobRecord, state: 'succeeded' | 'failed' | 'cancelled', error: Job['error']): void {
    record.state = state;
    record.error = error;
    record.finishedAt = Date.now();
    if (state === 'succeeded') record.at = record.phases.length;
    else record.effects.undo?.();
    record.effects.over?.();
    this.show(record);
  }

  private percentOf(record: JobRecord): number {
    if (record.state === 'succeeded') return 100;
    let sum = 0;
    for (let index = 0; index < record.phases.length; index += 1) {
      const phase = record.phases[index];
      if (phase === undefined) continue;
      if (index < record.at) sum += phase.weight;
      else if (index === record.at) sum += phase.weight * (record.step / STEPS_PER_PHASE);
    }
    return Math.min(99, Math.round(sum));
  }

  private peak = new Map<string, number>();

  /** The job as the API gives it. The percentage never goes back. */
  wire(record: JobRecord): Job {
    const percent = Math.max(this.percentOf(record), this.peak.get(record.id) ?? 0, record.state === 'succeeded' ? 100 : 0);
    if (record.state === 'running') this.peak.set(record.id, percent);
    const phase = record.phases[record.at];
    const copying = phase?.name === 'copy';
    const fraction = copying ? record.step / STEPS_PER_PHASE : 0;
    const doneBytes = record.state === 'succeeded' || (phase !== undefined && record.phases.findIndex((entry) => entry.name === 'copy') < record.at) ? record.bytes : Math.round(record.bytes * fraction);
    return {
      batch_id: record.batch,
      created_at: iso(record.createdAt),
      created_by: this.user(record.by.me.username),
      environment_id: record.environment,
      error: record.error,
      finished_at: record.finishedAt === null ? null : iso(record.finishedAt),
      id: record.id,
      kind: record.kind,
      log: record.log.length === 0 ? null : record.log.join('\n'),
      node_id: record.thread,
      progress: {
        bytes_done: doneBytes,
        bytes_total: record.bytes === 0 ? null : record.bytes,
        eta_seconds: record.state === 'running' && copying ? Math.round((1 - fraction) * 180) : null,
        files_done: record.bytes === 0 ? 0 : Math.round((doneBytes / record.bytes) * record.files),
        files_total: record.files === 0 ? null : record.files,
        message: record.state === 'running' && phase !== undefined ? PHASE_LINES[phase.name] ?? null : null,
        percent,
        phase: phase?.name ?? record.phases.at(-1)?.name ?? '',
        phases: record.phases.map((entry, index) => ({
          name: entry.name,
          state: record.state === 'succeeded' || index < record.at ? 'done' : index === record.at && record.state === 'running' ? 'running' : 'pending',
          weight: entry.weight,
        })),
        rate_bps: record.state === 'running' && copying ? 38 * MB : null,
      },
      provider_id: record.provider,
      result: record.result,
      started_at: record.startedAt === null ? null : iso(record.startedAt),
      state: record.state,
    };
  }

  private refOf(record: JobRecord): JobRef {
    const wire = this.wire(record);
    return { id: record.id, kind: record.kind, percent: wire.progress.percent, phase: wire.progress.phase, state: record.state };
  }

  /** The job that is under way on an environment, as the page of the environment names it. */
  refOfEnvironment(environmentId: string): JobRef | null {
    const record = [...this.jobs.values()].find((job) => job.environment === environmentId && (job.state === 'queued' || job.state === 'running') && job.kind !== 'thread_fork');
    return record === undefined ? null : this.refOf(record);
  }

  /** Tells whoever follows a thread what holds it now, or that nothing does. */
  private show(record: JobRecord): void {
    for (const threadId of this.locked(record)) {
      if (this.sides.has(threadId)) continue;
      const held = record.state === 'queued' || record.state === 'running';
      this.world.threads.emit(threadId, { thread: { lock: held ? this.refOf(record) : null } });
    }
  }

  /** The threads a job holds. */
  private locked(record: JobRecord): string[] {
    return record.threadsLocked;
  }

  private lock(record: JobRecord, threads: string[]): void {
    record.threadsLocked = threads;
    for (const threadId of threads) {
      const row = this.world.places.rowOf(threadId);
      if (row?.kind !== 'thread') continue;
      const owner = this.world.people.get(this.world.places.metaOf(threadId).owner);
      const before = owner === undefined ? null : this.world.threads.thread(owner, row);
      record.before.set(threadId, { state: before?.state ?? 'idle', blocked: before?.blocked ?? null });
      const running = before?.state === 'running';
      this.world.threads.emit(threadId, { thread: { lock: this.refOf(record), ...(running ? {} : { state: 'blocked' as const, blocked: { reason: 'switching' as const, detail: null } }) } });
    }
  }

  private release(record: JobRecord, ok: boolean): void {
    for (const threadId of record.threadsLocked) {
      const before = record.before.get(threadId);
      if (this.world.places.rowOf(threadId)?.kind !== 'thread') continue;
      if (ok) this.world.threads.emit(threadId, { thread: { lock: null, state: 'idle', blocked: null } });
      else this.world.threads.emit(threadId, { thread: { lock: null, state: before?.state ?? 'idle', blocked: before?.blocked ?? null } });
    }
  }

  // ---------------------------------------------------------------- the requests

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    if ((found = /^POST \/threads\/([^/]+)\/forks\/preflight$/.exec(route))) return { status: 200, body: this.preflightFork(person, found[1] ?? '', body) };
    if ((found = /^POST \/threads\/([^/]+)\/forks$/.exec(route))) return { status: 202, body: this.confirmFork(person, found[1] ?? '', body) };
    if ((found = /^PATCH \/providers\/([^/]+)\/permissions$/.exec(route))) return { status: 200, body: this.openProvider(person, found[1] ?? '', body) };
    if (route === 'POST /forks/preflight') return { status: 200, body: this.preflightBatch(person, body) };
    if (route === 'POST /forks') return { status: 202, body: this.confirmBatch(person, body) };
    if ((found = /^GET \/forks\/([^/]+)$/.exec(route))) return { status: 200, body: this.batchOf(person, found[1] ?? '') };
    if ((found = /^POST \/forks\/([^/]+)\/cancel$/.exec(route))) return { status: 200, body: this.cancelBatch(person, found[1] ?? '') };
    if ((found = /^GET \/jobs\/([^/]+)$/.exec(route))) return { status: 200, body: this.resources.jobOf(person, found[1] ?? '') ?? this.jobAnswer(found[1] ?? '') };
    if ((found = /^POST \/jobs\/([^/]+)\/cancel$/.exec(route))) return { status: 200, body: this.cancelJob(person, found[1] ?? '') };
    if ((found = /^GET \/nodes\/([^/]+)\/places$/.exec(route))) {
      const thread = this.world.places.rowOf(found[1] ?? '')?.kind === 'thread' ? (found[1] ?? '') : null;
      return { status: 200, body: this.placeOf(person, thread ?? this.firstThreadOf(found[1] ?? '')) };
    }
    if ((found = /^POST \/environments\/([^/]+)\/updates\/preflight$/.exec(route))) return { status: 200, body: this.preflightEnvironmentUpdate(person, found[1] ?? '', body) };
    if ((found = /^POST \/environments\/([^/]+)\/updates$/.exec(route))) return { status: 202, body: this.confirmEnvironmentUpdate(person, found[1] ?? '', body) };
    if ((found = /^GET \/environments\/([^/]+)\/moves\/places$/.exec(route))) return { status: 200, body: this.movePlaces(person, this.mustEnvironment(person, found[1] ?? '')) };
    if ((found = /^POST \/environments\/([^/]+)\/moves\/preflight$/.exec(route))) return { status: 200, body: this.preflightEnvironmentMove(person, found[1] ?? '', body) };
    if ((found = /^POST \/environments\/([^/]+)\/moves$/.exec(route))) return { status: 202, body: this.confirmEnvironmentMove(person, found[1] ?? '', body) };
    if ((found = /^POST \/nodes\/([^/]+)\/environments\/(updates|moves)\/preflight$/.exec(route))) return { status: 200, body: this.preflightNodeBatch(person, found[1] ?? '', found[2] === 'moves', body) };
    if ((found = /^POST \/nodes\/([^/]+)\/environments\/(updates|moves)$/.exec(route))) return { status: 202, body: this.confirmNodeBatch(person, found[2] === 'moves', body) };
    if ((found = /^POST \/nodes\/([^/]+)\/environments\/recycle$/.exec(route))) return { status: 200, body: this.recycle(person, found[1] ?? '', body) };
    if ((found = /^POST \/hosts\/([^/]+)\/recycle$/.exec(route))) return { status: 200, body: this.recycle(person, found[1] ?? '', body, true) };

    // Side threads: made from a thread, read and written like a thread, closed like a node.
    if ((found = /^POST \/threads\/([^/]+)\/side$/.exec(route))) return { status: 201, body: this.startSide(person, found[1] ?? '', body) };
    if ((found = /^GET \/threads\/([^/]+)\/side$/.exec(route))) return { status: 200, body: this.listSides(person, found[1] ?? '', query.get('state') === 'closed' ? 'closed' : 'open') };
    if ((found = /^GET \/threads\/([^/]+)\/sync$/.exec(route)) && this.sides.has(found[1] ?? '')) return { status: 200, body: this.sideSync(person, found[1] ?? '', query.get('since') === null ? null : Number(query.get('since'))) };
    if ((found = /^POST \/threads\/([^/]+)\/inputs$/.exec(route)) && this.sides.has(found[1] ?? '')) return { status: 201, body: this.sideInput(person, found[1] ?? '', body) };
    if ((found = /^POST \/threads\/([^/]+)\/interrupt$/.exec(route)) && this.sides.has(found[1] ?? '')) return { status: 200, body: this.sideInterrupt(person, found[1] ?? '') };
    if ((found = /^POST \/threads\/([^/]+)\/read$/.exec(route)) && this.sides.has(found[1] ?? '')) return { status: 200, body: { last_read_seq: Number(body.seq ?? 0) } };
    if ((found = /^GET \/nodes\/([^/]+)$/.exec(route)) && this.sides.has(found[1] ?? '')) return { status: 200, body: this.threadOf(person, found[1] ?? '') };
    if ((found = /^DELETE \/nodes\/([^/]+)$/.exec(route)) && this.sides.has(found[1] ?? '')) return this.closeSide(person, found[1] ?? '', 'closed');
    return null;
  }

  private firstThreadOf(nodeId: string): string | null {
    const row = this.world.places.rowOf(nodeId);
    if (row === undefined) return null;
    const threads = [...this.resources.bindings.keys()].filter((threadId) => this.world.places.rowOf(threadId)?.parent_id === nodeId);
    return threads[0] ?? null;
  }

  private preflightFork(person: Person, threadId: string, body: Record<string, unknown>): ForkPlan {
    const { plan, asked } = this.draftFork(person, threadId, body);
    this.keep(person, 'fork', threadId, plan, { asked, askedFor: body });
    return plan;
  }

  private confirmFork(person: Person, threadId: string, body: Record<string, unknown>): Job {
    const stored = this.take(person, asText(body.preflight_id), 'fork');
    if (stored.subject !== threadId) throw new Refusal(404, 'not_found', 'there is no such plan');
    // The plan is worked out again from what was asked, and what is true now is what is carried out.
    const options = isRecord(body.options) ? { ...(stored.askedFor?.options as Record<string, unknown> | undefined), ...body.options } : stored.askedFor?.options;
    const { plan, asked } = this.draftFork(person, threadId, { ...stored.askedFor, ...(options === undefined ? {} : { options }) });
    if (plan.blockers.length > 0) {
      const held = plan.blockers.find((blocker) => blocker.code === 'already_running');
      if (held !== undefined) {
        const job = this.openJobFor({ thread: threadId });
        throw new Refusal(409, 'conflict.exists', 'a fork or a switch holds this thread already', { job_id: job?.id ?? null });
      }
      throw new Refusal(409, 'plan.blocked', plan.blockers[0]?.message ?? 'the plan cannot be carried out', { blockers: plan.blockers });
    }
    const record = this.makeFork(person, threadId, plan, asked, null);
    this.plans.delete(plan.preflight_id);
    this.plans.delete(asText(body.preflight_id));
    return this.wire(record);
  }

  private makeFork(person: Person, threadId: string, plan: ForkPlan, asked: Asked, batch: string | null): JobRecord {
    const bound = this.bindingOfAny(threadId);
    const side = this.sides.get(threadId);
    const names = this.phasesOf('thread_fork', plan);
    const replace = plan.result === 'replace';
    const target = { ...bound, provider: asked.provider, agent: asked.agent, model: asked.model, effort: asked.effort, directory: asked.directory };
    const before = { ...bound };
    const bytes = plan.files?.total_bytes ?? 0;
    const files = plan.files?.file_count ?? 0;
    let madeEnvironment: string | null = null;
    let madeThread: string | null = null;
    const record = this.makeJob(person, 'thread_fork', names, { thread: threadId, batch, environment: replace ? bound.environment : null }, {}, { bytes, files });
    record.result = replace ? { thread_id: threadId } : null;
    record.effects = {
      begin: () => {
        // A new environment is made when the provision phase begins, and taken away again if the job does not commit.
        if (isRecord(plan.to.environment) && 'new' in plan.to.environment) {
          const shared = batch === null ? undefined : this.batches.get(batch);
          const made = shared?.environment ?? this.makeEnvironment(person, plan.to.environment.new.name, plan.to.environment.new.place.host?.id ?? '');
          if (shared !== undefined) shared.environment = made;
          madeEnvironment = made;
        }
      },
      commit: () => {
        const environmentId = madeEnvironment ?? (asked.environment.kind === 'use' ? asked.environment.id : bound.environment);
        const finalTarget = { ...target, environment: environmentId };
        const segmentId = randomUUID();
        if (replace) {
          this.resources.bind(threadId, finalTarget);
          const segment: Segment = this.segmentFor(person, finalTarget, plan.history.method, plan.history.report, 2, segmentId);
          this.world.threads.emit(threadId, {
            thread: { lock: null, state: 'idle', blocked: null },
            segments: [segment],
            items: [
              marker(randomUUID(), {
                marker: 'switched',
                data: {
                  by: this.user(person.me.username),
                  from: { agent: plan.from.agent, directory: before.directory, environment: plan.from.environment, provider: plan.from.provider },
                  report: plan.history.report,
                  to: { agent: plan.to.agent, directory: asked.directory, environment: this.environmentRef(environmentId), provider: plan.to.provider },
                },
              }),
            ],
          });
        } else {
          const topicId = asked.topic ?? (side === undefined ? (this.world.places.rowOf(threadId)?.parent_id ?? ACME.fixLogin) : (this.world.places.rowOf(side.parent)?.parent_id ?? ACME.fixLogin));
          const source = side === undefined ? threadId : side.parent;
          const name = asked.name ?? `${this.threadOf(person, threadId).name.replace(/ \(side[^)]*\)$/, '')} (fork)`;
          const row = this.world.places.makeThreadRow(person, topicId, name);
          madeThread = row.id;
          this.resources.bind(row.id, finalTarget);
          this.world.threads.emit(row.id, {
            items: [marker(randomUUID(), { marker: 'forked_from', data: { by: this.user(person.me.username), report: plan.history.report, thread: this.threadRef(source), turn_id: randomUUID() } })],
          });
          this.world.threads.emit(source, { items: [marker(randomUUID(), { marker: 'forked', data: { by: this.user(person.me.username), into: this.threadRef(row.id), turn_id: randomUUID() } })] });
          record.result = { thread_id: row.id };
          if (side !== undefined) this.closeSide(person, threadId, 'kept', row.id);
        }
      },
      undo: () => {
        if (madeEnvironment !== null && batch === null) this.removeEnvironment(madeEnvironment);
        if (madeThread !== null) this.world.places.rowOf(madeThread);
        if (replace) this.release(record, false);
      },
      over: () => {
        if (replace && record.state === 'succeeded') this.release(record, true);
        if (replace && record.state === 'succeeded' && plan.leaves?.environment?.action === 'remove') {
          const old = this.environment(plan.leaves.environment.environment.id);
          if (old !== undefined) old.removeAt = iso(Date.now() + 30_000);
        }
        if (batch !== null) this.cleanupBatchEnvironment(batch);
      },
    };
    if (replace) this.lock(record, [threadId]);
    this.run(record);
    return record;
  }

  private segmentFor(person: Person, target: ThreadBindingLike, method: HistoryMethod, report: HistoryReport, ordinal: number, id: string): Segment {
    const refs = this.resources.refsOf(target);
    return {
      agent: refs.agent!,
      arrangement: 'separated',
      credential_path: 'native',
      end_reason: null,
      ended_at: null,
      env_generation: 1,
      environment: refs.environment!,
      executor: null,
      id,
      import_fidelity: method === 'none' ? null : report.fidelity,
      import_report: report,
      ordinal,
      provider: refs.provider!,
      seq_updated: 0,
      started_at: iso(Date.now()),
      started_by: this.user(person.me.username),
      tool_set: null,
    };
  }

  private makeEnvironment(person: Person, name: string, hostId: string): string {
    const record = {
      id: fixtureId(KIND.environment, 100 + this.resources.environments.length),
      name,
      owner: person.me.username,
      home: ACME.org,
      machine: null,
      host: hostId,
      template: fixtureId(KIND.template, 1),
      status: 'running' as const,
      root: '/workspace',
      remoteWrite: true,
      terminal: true,
      revision: 1,
      uses: [],
      manages: [],
      updates: 0,
      removeAt: null,
    };
    this.resources.environments.push(record);
    return record.id;
  }

  private removeEnvironment(environmentId: string): void {
    const at = this.resources.environments.findIndex((record) => record.id === environmentId);
    if (at >= 0) this.resources.environments.splice(at, 1);
  }

  private cleanupBatchEnvironment(batchId: string): void {
    const batch = this.batches.get(batchId);
    if (batch?.environment === undefined) return;
    const environment = batch.environment;
    const unused = batch.jobs.every((id) => {
      const job = this.jobs.get(id);
      return job !== undefined && !job.committed && job.state !== 'queued' && job.state !== 'running';
    }) && ![...this.resources.bindings.values()].some((binding) => binding.environment === environment);
    if (unused) { this.removeEnvironment(environment); delete batch.environment; }
  }

  // ---------------------------------------------------------------- jobs and batches

  private jobAnswer(id: string): Job {
    const measurement = this.measurements.get(id);
    if (measurement !== undefined) return measurement;
    const record = this.jobs.get(id);
    if (record === undefined) throw new Refusal(404, 'not_found', 'there is no such job');
    return this.wire(record);
  }

  private cancelJob(_person: Person, id: string): Job {
    const measurement = this.measurements.get(id);
    if (measurement !== undefined) {
      if (measurement.state === 'queued' || measurement.state === 'running') {
        const activity = measurement.progress.activity;
        measurement.progress.activity = { updated_at: activity?.updated_at ?? new Date().toISOString(), stall_after_seconds: activity?.stall_after_seconds ?? 30, directories_done: activity?.directories_done ?? null, waiting_for: activity?.waiting_for ?? null, stopping: true };
      }
      return measurement;
    }
    const record = this.jobs.get(id);
    if (record === undefined) throw new Refusal(404, 'not_found', 'there is no such job');
    if (record.state === 'queued' || record.state === 'running') {
      if (record.committed) throw new Refusal(409, 'plan.blocked', 'the job is past the point after which nothing is undone');
      record.cancelled = true;
      // The job is stopped at its next step, or at once when the double does not step by itself.
      if (record.state === 'queued') this.advance(record);
      else if (this.mode === 'auto') this.advance(record);
    }
    return this.wire(record);
  }

  /** What a step of a job is asked to do by a test: a job advances by one step, or all the way. */
  step(id: string | null, count: number): void {
    const records = id === null ? [...this.jobs.values()] : [this.jobs.get(id)].filter((record): record is JobRecord => record !== undefined);
    for (let n = 0; n < count; n += 1) for (const record of records) this.advance(record);
  }

  finish(id: string | null): void {
    for (let guard = 0; guard < 400; guard += 1) {
      const open = [...this.jobs.values()].filter((record) => (id === null || record.id === id) && (record.state === 'queued' || record.state === 'running'));
      if (open.length === 0) return;
      for (const record of open) this.advance(record);
    }
  }

  private batchOf(_person: Person, id: string): ForkBatch {
    const batch = this.batches.get(id);
    if (batch === undefined) throw new Refusal(404, 'not_found', 'there is no such batch');
    const all = batch.jobs.map((jobId) => this.jobs.get(jobId)).filter((record): record is JobRecord => record !== undefined);
    const counts = { cancelled: 0, failed: 0, lost: 0, queued: 0, running: 0, succeeded: 0 };
    for (const record of all) counts[record.state] += 1;
    const visible = all.filter((record) => record.thread === null || !batch.hidden.has(record.thread));
    const percent = all.length === 0 ? 0 : Math.round(all.reduce((sum, record) => sum + this.wire(record).progress.percent, 0) / all.length);
    return {
      batch_id: id,
      counts,
      hidden: all.length - visible.length,
      jobs: visible.map((record) => ({ job: this.wire(record), thread: this.threadRef(record.thread ?? '') })),
      percent,
    };
  }

  private cancelBatch(person: Person, id: string): ForkBatch {
    const batch = this.batches.get(id);
    if (batch === undefined) throw new Refusal(404, 'not_found', 'there is no such batch');
    if (batch.by !== person.me.username) throw new Refusal(403, 'authz.denied', 'only the one who started a batch stops it', { required: 'batch.creator', object: { kind: 'thread', id }, requestable: false, override: false, missing: [] });
    for (const jobId of batch.jobs) {
      const record = this.jobs.get(jobId);
      if (record === undefined || record.committed) continue;
      if (record.state === 'queued' || record.state === 'running') {
        record.cancelled = true;
        if (record.state === 'queued') {
          record.state = 'running';
          this.advance(record);
        } else if (this.mode === 'auto') this.advance(record);
      }
    }
    return this.batchOf(person, id);
  }

  // ---------------------------------------------------------------- several threads at once

  private preflightBatch(person: Person, body: Record<string, unknown>): ForkBatchPlan {
    const ids = asList(body.thread_ids).filter((entry): entry is string => typeof entry === 'string');
    const environment = asText(body.all_of_environment);
    const provider = asText(body.all_of_provider);
    const agent = asText(body.all_of_agent);
    const sourceModel=asText(body.source_model_id);
    if(sourceModel!==''&&provider==='')throw new Refusal(400,'request.invalid','source_model_id goes with all_of_provider');
    if ([ids.length > 0, environment !== '', provider !== '', agent !== ''].filter(Boolean).length !== 1) throw new Refusal(400, 'request.invalid', 'name exactly one thread selector', { errors: [{ path: 'thread_ids', message: 'exactly one of thread_ids, all_of_environment, all_of_provider and all_of_agent is given' }] });
    const maximum=this.world.admin.settingValue('switch.batch_threads');
    if(typeof maximum!=='number'||!Number.isInteger(maximum)||maximum<1)throw new Refusal(400,'request.invalid','switch.batch_threads is invalid');
    if(provider!==''||agent!=='')this.resources.handle(person,'POST','/resources/impact',new URLSearchParams(),{object:{kind:provider!==''?'provider':'agent',id:provider||agent},model_id:sourceModel||null});
    const model=sourceModel===''?null:this.resources.providers.find((row)=>row.id===provider)?.models.find((row)=>row.id===sourceModel)?.model_name;
    if(sourceModel!==''&&model===undefined)throw new Refusal(404,'not_found','there is no source model');
    const threads = environment === '' && provider === '' && agent==='' ? ids : [...this.resources.bindings].filter(([thread, binding]) => (environment !== '' ? binding.environment === environment : agent!==''?binding.agent===agent:binding.provider === provider) && (model===null||binding.model===model) && this.world.places.rowOf(thread)?.deleted !== true).map(([thread]) => thread);
    const eligible=provider!==''||agent!==''?threads.filter((id)=>this.world.places.access(person,id).permissions.includes('thread.manage')):threads;
    if(eligible.length>maximum)throw new Refusal(400,'request.invalid',`at most ${maximum} threads, as set by switch.batch_threads`,{errors:[{path:'thread_ids',message:`at most ${maximum} threads`}]});
    if (environment !== '' && !this.mayManage(person, environment)) throw new Refusal(403, 'authz.denied', 'managing the environment is needed', { required: 'environment.manage', object: { kind: 'environment', id: environment }, requestable: true, override: false, missing: [this.missing('environment.manage', { kind: 'environment', id: environment, name: this.environment(environment)?.name ?? '', owner: this.environment(environment)?.owner ?? 'ada' })] });
    const request = { result: 'replace', ...(isRecord(body.to) ? { to: body.to } : {}), ...(isRecord(body.environment) ? { environment: body.environment } : {}), ...(isRecord(body.options) ? { options: body.options } : {}) };
    const plans: ForkPlan[] = [];
    const skipped: ForkBatchPlan['skipped'] = [];
    let applied: AppliedChangeOptions = this.options(body.options);
    for (const threadId of threads) {
      const row = this.world.places.rowOf(threadId);
      if (row?.kind !== 'thread') {
        skipped.push({ reason: 'not_found', thread: this.threadRef(threadId) });
        continue;
      }
      try {
        if ((provider !== ''||agent!=='') && !this.world.places.access(person, threadId).permissions.includes('thread.manage')) {
          skipped.push({ reason: 'no_permission', thread: { ...this.threadRef(threadId), name: null, redacted: true } });
          continue;
        }
        const redacted = !this.maySee(threadId);
        const { plan } = this.draftFork(redacted ? this.admin(person) : person, threadId, request, redacted);
        applied = plan.options;
        plans.push(plan);
        const location = request.environment;
        if (plans.length === 1 && isRecord(location) && isRecord(location.new) && asText(location.new.name) === '' && 'new' in plan.to.environment) {
          request.environment = { ...location, new: { ...location.new, name: plan.to.environment.new.name } };
        }
      } catch (error) {
        if (error instanceof Refusal && error.answer.status === 409) skipped.push({ reason: 'archived', thread: this.threadRef(threadId) });
        else throw error;
      }
    }
    const executable = plans.filter((plan) => plan.blockers.length === 0).length;
    const to = isRecord(body.to) ? body.to : {};
    const made: ForkBatchPlan = {
      estimate: this.estimate(plans.length * 12, true),
      executable,
      expires_at: iso(this.now() + PLAN_TTL_MS),
      options: applied,
      plans,
      preflight_id: randomUUID(),
      skipped,
      to: {
        agent: asText(to.agent_id) === '' ? null : this.agentRef(asText(to.agent_id)),
        effort: typeof to.effort === 'string' ? to.effort : null,
        model: typeof to.model === 'string' ? to.model : null,
        provider: asText(to.provider_id) === '' ? null : this.providerRef(asText(to.provider_id)),
      },
      turns_running: plans.filter((plan) => plan.turn_running).length,
    };
    this.keep(person, 'batch', '', made, { askedFor: request });
    return made;
  }

  /** The system's own eyes: a thread the caller cannot see is planned all the same, and says only where it goes. */
  private admin(person: Person): Person {
    return this.world.people.get('ada') ?? person;
  }

  private confirmBatch(person: Person, body: Record<string, unknown>): ForkBatch {
    const stored = this.take(person, asText(body.preflight_id), 'batch');
    const made = stored.plan as ForkBatchPlan;
    const options = isRecord(body.options) ? body.options : null;
    if (made.executable === 0) throw new Refusal(409, 'plan.blocked', 'none of the plans can be carried out', { blockers: made.plans[0]?.blockers ?? [] });
    const batch: BatchRecord = { id: randomUUID(), by: person.me.username, jobs: [], hidden: new Set(made.plans.filter((plan) => !this.maySee(plan.source.id)).map((plan) => plan.source.id)) };
    this.batches.set(batch.id, batch);
    for (const plan of made.plans) {
      if (plan.blockers.length > 0) continue;
      const threadId = plan.source.id;
      const redacted = !this.maySee(threadId);
      const { plan: fresh, asked } = this.draftFork(redacted ? this.admin(person) : person, threadId, { ...(stored.askedFor ?? {}), ...(options === null ? {} : { options: { ...(isRecord(stored.askedFor?.options) ? stored.askedFor.options : {}), ...options } }) }, redacted, batch.id);
      if (fresh.blockers.length > 0) continue;
      const record = this.makeFork(person, threadId, fresh, asked, batch.id);
      batch.jobs.push(record.id);
    }
    this.plans.delete(made.preflight_id);
    return this.batchOf(person, batch.id);
  }

  // ---------------------------------------------------------------- an environment: update, recreate, move

  private environmentUpdates(record: NonNullable<ReturnType<Change['environment']>>, reason: 'update' | 'recreate'): PendingUpdate[] {
    if (reason === 'recreate') return [];
    return Array.from({ length: record.updates }, (_, n) => ({ name: n === 0 ? 'Image' : n === 1 ? 'Exec node' : 'Template Default', what: n === 0 ? ('image' as const) : n === 1 ? ('executor' as const) : ('template' as const), from: n === 0 ? 'orch-env-base:0.1.0' : '0.1.0', to: n === 0 ? 'orch-env-base:0.1.1' : '0.1.1' }));
  }

  private threadsOfEnvironment(environmentId: string): { thread: NodeRef | null; turn_running: boolean }[] {
    return [...this.resources.bindings]
      .filter(([thread, binding]) => binding.environment === environmentId && this.world.places.rowOf(thread)?.deleted !== true)
      .map(([thread]) => ({ thread: this.maySee(thread) ? this.threadRef(thread) : null, turn_running: this.world.places.rowOf(thread)?.thread?.state === 'running' }));
  }

  private mustEnvironment(person: Person, environmentId: string) {
    const record = this.environment(environmentId);
    if (record === undefined) throw new Refusal(404, 'not_found', 'there is no such environment');
    if (!this.mayManage(person, environmentId)) {
      throw new Refusal(403, 'authz.denied', 'environment.manage is required', { required: 'environment.manage', object: { kind: 'environment', id: environmentId }, requestable: true, override: false, missing: [this.missing('environment.manage', { kind: 'environment', id: environmentId, name: record.name, owner: record.owner })] });
    }
    return record;
  }

  private updatePlan(_person: Person, record: NonNullable<ReturnType<Change['environment']>>, reason: 'update' | 'recreate', options: unknown): EnvironmentUpdatePlan {
    const blockers: PlanBlocker[] = [];
    if (record.machine !== null) blockers.push({ code: 'unavailable', message: 'a directory on a person’s machine has no unit to replace' });
    if (this.openJobFor({ environment: record.id }) !== undefined) blockers.push({ code: 'already_running', message: 'a fork, a switch, a move or an update is under way for this environment' });
    const threads = this.threadsOfEnvironment(record.id);
    const applied = this.options(options);
    return {
      blockers,
      environment: this.environmentRef(record.id),
      estimate: this.estimate(55, true),
      expires_at: iso(this.now() + PLAN_TTL_MS),
      kept: ['The workspace', 'The home', 'Mounts and capabilities', 'Every conversation'],
      lost: ['Running processes', 'Anything installed outside the workspace and the home'],
      options: applied,
      preflight_id: randomUUID(),
      reason: reason === 'recreate' ? 'recreate' : 'update',
      rollback: 'unit',
      network: { mode: 'open', required_mode: 'open', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: '', always: [], always_revision: '', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
      threads,
      updates: this.environmentUpdates(record, reason),
    };
  }

  private preflightEnvironmentUpdate(person: Person, environmentId: string, body: Record<string, unknown>): EnvironmentUpdatePlan {
    const record = this.mustEnvironment(person, environmentId);
    const plan = this.updatePlan(person, record, body.reason === 'recreate' ? 'recreate' : 'update', null);
    this.keep(person, 'update', environmentId, plan, { askedFor: { reason: plan.reason } });
    return plan;
  }

  private confirmEnvironmentUpdate(person: Person, environmentId: string, body: Record<string, unknown>): Job {
    const stored = this.take(person, asText(body.preflight_id), 'update');
    const record = this.mustEnvironment(person, environmentId);
    const reason = stored.askedFor?.reason === 'recreate' ? 'recreate' : 'update';
    const fresh = this.updatePlan(person, record, reason, body.options);
    if (fresh.blockers.length > 0) {
      if (fresh.blockers.some((blocker) => blocker.code === 'already_running')) throw new Refusal(409, 'conflict.exists', 'an update holds this environment already', { job_id: this.openJobFor({ environment: environmentId })?.id ?? null });
      throw new Refusal(409, 'plan.blocked', fresh.blockers[0]?.message ?? 'the plan cannot be carried out', { blockers: fresh.blockers });
    }
    this.plans.delete(asText(body.preflight_id));
    return this.wire(this.makeEnvironmentJob(person, record, 'environment_recreate', reason === 'update' ? 'update' : 'recreate', null));
  }

  private makeEnvironmentJob(person: Person, record: NonNullable<ReturnType<Change['environment']>>, kind: 'environment_recreate' | 'environment_move', reason: 'update' | 'recreate' | 'move', batch: string | null, toHost: string | null = null, toMachine: string | null = null, toTemplate: string | null = null): JobRecord {
    const before = { status: record.status, host: record.host, machine: record.machine, template: record.template, root: record.root, config: record.config, updates: record.updates, revision: record.revision };
    const threads = [...this.resources.bindings].filter(([thread, binding]) => binding.environment === record.id && this.world.places.rowOf(thread)?.deleted !== true).map(([thread]) => thread);
    const names = this.phasesOf(kind, {});
    const job = this.makeJob(person, kind, names, { environment: record.id, batch }, {}, kind === 'environment_move' ? { bytes: Math.round(3.1 * GB), files: 1_240 } : { bytes: 0, files: 0 });
    job.effects = {
      begin: () => {
        record.status = kind === 'environment_move' ? 'moving' : 'recreating';
        record.revision += 1;
      },
      commit: () => {
        if (kind === 'environment_move') {
          record.host = toMachine === null ? toHost : null;
          record.machine = toMachine;
          record.generation = (record.generation ?? 0) + 1;
          record.config = { ...record.config, 'env.data_sources': { workspace: { kind: 'made_by_orch' }, home: { kind: 'made_by_orch' } } };
          if (toMachine !== null) { record.template = null; record.root = `/orch-data/${record.id}/generation-${record.generation}/workspace`; }
          else record.template = toTemplate ?? record.template ?? this.resources.templates.find((template) => template.kind === 'environment')?.id ?? null;
        }
        if (kind === 'environment_recreate') record.updates = 0;
        record.status = before.status === 'stopped' ? 'stopped' : 'running';
        record.revision += 1;
        for (const thread of threads) {
          this.world.threads.emit(thread, {
            items: [
              kind === 'environment_move'
                ? marker(randomUUID(), { marker: 'environment_moved', data: { by: this.user(person.me.username), from: this.environmentRef(record.id), mounts_detached: [], path_map: { from: '/work', to: '/work' }, to: this.environmentRef(record.id) } })
                : marker(randomUUID(), { marker: 'environment_recreated', data: { generation: 2, reason: reason === 'update' ? 'update' : 'recreate', template: { id: record.template ?? fixtureId(KIND.template, 1), name: 'Default' } } }),
            ],
          });
        }
      },
      undo: () => {
        record.status = before.status;
        record.host = before.host;
        record.machine = before.machine;
        record.template = before.template;
        record.root = before.root;
        if (before.config === undefined) delete record.config;
        else record.config = before.config;
        record.updates = before.updates;
        record.revision += 1;
        this.release(job, false);
      },
      over: () => {
        if (job.state === 'succeeded') this.release(job, true);
      },
    };
    this.lock(job, threads);
    this.run(job);
    return job;
  }

  private movePlaces(person: Person, record: NonNullable<ReturnType<Change['environment']>>): MovePlace[] {
    const places: MovePlace[] = this.resources.hosts.filter((host) => host.id !== record.host).map((host) => ({ kind: 'host' as const, ...this.placeHost(host, false) }));
    for (const machine of this.resources.machines) {
      if (machine.id === record.machine) continue;
      let data: MachineEnvironmentData;
      try {
        const answer = this.resources.handle(person, 'GET', `/machines/${machine.id}/environment-data`, new URLSearchParams(), {});
        if (answer === null) continue;
        data = answer.body as MachineEnvironmentData;
      } catch (error) { if (error instanceof Refusal) continue; throw error; }
      if (data.direct === null) continue;
      const reasons: string[] = [];
      if (!machine.online || machine.status !== 'active') reasons.push('The machine is offline.');
      if (data.direct.made_by_orch === null) reasons.push(data.direct.reason ?? 'The machine has not offered making data.');
      if (machine.policy['environment.remote_write'] !== true) reasons.push('The machine policy has remote writes off.');
      if (data.roots.length === 0) reasons.push('The machine policy offers no environment.roots.');
      places.push({ kind: 'machine', machine: this.machineRefOf(machine.id), host: null, usable: reasons.length === 0, reasons });
    }
    return places;
  }

  private preflightEnvironmentMove(person: Person, environmentId: string, body: Record<string, unknown>): EnvironmentMovePlan {
    const record = this.mustEnvironment(person, environmentId);
    const plan = this.moveRequestPlan(person, record, body);
    this.keep(person, 'move', environmentId, plan, { askedFor: { ...body } });
    return plan;
  }

  private moveRequestPlan(person: Person, record: NonNullable<ReturnType<Change['environment']>>, body: Record<string, unknown>): EnvironmentMovePlan {
    const machineId = typeof body.machine_id === 'string' ? body.machine_id : null;
    if (machineId === null && record.template !== null && typeof body.template_id === 'string' && body.template_id !== record.template) throw new Refusal(400, 'request.invalid', 'a move keeps the current unit template');
    if (machineId !== null && typeof body.host_id === 'string') throw new Refusal(400, 'request.invalid', 'choose one machine or host');
    const choices = this.movePlaces(person, record);
    const selected = machineId === null
      ? typeof body.host_id === 'string' || choices.some((place) => place.kind === 'host' && place.usable) ? null : choices.find((place) => place.kind === 'machine' && place.usable)
      : choices.find((place) => place.kind === 'machine' && place.machine.id === machineId);
    if (machineId !== null && selected?.kind !== 'machine') throw new Refusal(404, 'not_found', 'the destination machine is not available');
    return this.movePlan(record, typeof body.host_id === 'string' ? body.host_id : null, selected?.kind === 'machine' ? selected : null, body.copy_directories === true, body);
  }

  private movePlan(record: NonNullable<ReturnType<Change['environment']>>, hostId: string | null, machine: MoveMachine | null = null, copyDirectories = false, options: Record<string, unknown> = {}): EnvironmentMovePlan {
    const blockers: PlanBlocker[] = [];
    const sources = record.config?.['env.data_sources'];
    const borrowed = typeof sources === 'object' && sources !== null
      ? Object.values(sources).some((source: unknown) => typeof source === 'object' && source !== null && 'kind' in source && source.kind === 'existing_directory') : record.machine !== null;
    if (borrowed && !copyDirectories) blockers.push({ code: 'existing_directory_on_source_machine', message: 'Existing directories stay on their machine. Choose Copy existing directories into new storage made by Orch; the originals are kept.' });
    if (machine !== null) blockers.push(...machine.reasons.map((message: string) => ({ code: 'machine_policy', message })));
    const hosts = this.resources.hosts.filter((host) => host.id !== record.host);
    const chosenHost = hostId === null ? hosts.find((host) => this.placeHost(host, false).usable) : this.host(hostId);
    if (chosenHost === undefined && machine === null) blockers.push({ code: 'no_host', message: 'no other host takes this environment' });
    const offeredHost = chosenHost ?? this.host(record.host ?? '') ?? this.resources.hosts[0];
    const place = machine !== null || offeredHost === undefined ? null : this.placeHost(offeredHost, false, 4_096);
    if (machine === null && place === null) throw new Refusal(409, 'plan.blocked', 'no permitted destination is available');
    if (machine === null && chosenHost !== undefined && place !== null && !place.usable) blockers.push({ code: 'no_capacity', message: `${place.host.name} takes nothing more now` });
    if (this.openJobFor({ environment: record.id }) !== undefined) blockers.push({ code: 'already_running', message: 'a fork, a switch, a move or an update is under way for this environment' });
    const threads = this.threadsOfEnvironment(record.id).map((entry) => ({ action: 'continue' as const, note: 'Its harness runs with the provider: the conversation continues.', thread: entry.thread, turn_running: entry.turn_running }));
    const borrowedHome = isRecord(sources) && isRecord(sources.home) && sources.home.kind === 'existing_directory';
    const carryHome = (typeof options.home === 'boolean' ? options.home : record.host !== null || isRecord(sources)) || (copyDirectories && borrowedHome);
    const files = this.filesFor(isRecord(options.files) ? options.files : { mode: 'all' }, true, carryHome, options.private_views !== false)!;
    if (machine !== null) files.private_views = files.private_views.map((view) => ({ ...view, carried: false, left_because: 'no_private_views' }));
    const destination: MovePlace = machine === null ? { kind: 'host', ...place! } : { kind: 'machine', ...machine };
    files.file_states = { roots: 2, objects: 6, total_bytes: 4 * MB, journals: 1, complete: true };
    files.bytes_to_upload += files.file_states.total_bytes;
    files.bytes_to_download += files.file_states.total_bytes;
    const extra = (files.home?.total_bytes ?? 0) + files.private_views.filter((view) => view.carried).reduce((sum, view) => sum + view.total_bytes, 0);
    files.bytes_to_upload += extra;
    files.bytes_to_download += extra;
    return {
      blockers,
      environment: this.environmentRef(record.id),
      notification: { people: 0, default_on: false },
      estimate: this.estimate(150, true),
      expires_at: iso(this.now() + PLAN_TTL_MS),
      files,
      from: record.host === null ? null : this.hostRef(record.host),
      ...(record.machine === null ? {} : { from_machine: this.machineRefOf(record.machine) }),
      network: { mode: 'open', required_mode: 'open', network_mode: machine === null ? 'bridge' : 'connected', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: '', always: [], always_revision: '', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
      held_still: record.host !== null,
      mounts: [
        planMount({ id: fixtureId(0x2b, 1), name: 'src', address: 'acme.ada.src' }, place?.mounts.find((entry) => entry.mount.name === 'src')?.present === true, '/mnt/src', 'private', 'as_is', askedMount(options, fixtureId(0x2b, 1)), machine === null),
        planMount({ id: fixtureId(0x2b, 2), name: 'datasets', address: 'acme.ada.datasets' }, false, '/mnt/datasets', 'read_only', 'as_is', askedMount(options, fixtureId(0x2b, 2)), machine === null),
      ],
      mcp: [{ id: fixtureId(0x2c, 1), name: 'tmux', own: false, imported: false, fate: 'kept', reason: null }],
      options: this.options(null),
      preflight_id: randomUUID(),
      threads,
      to: destination,
    };
  }

  private confirmEnvironmentMove(person: Person, environmentId: string, body: Record<string, unknown>): Job {
    const stored = this.take(person, asText(body.preflight_id), 'move');
    const record = this.mustEnvironment(person, environmentId);
    const fresh = this.moveRequestPlan(person, record, stored.askedFor ?? {});
    const machineId = 'machine' in fresh.to ? fresh.to.machine.id : null;
    const hostId = 'machine' in fresh.to ? null : fresh.to.host.id;
    if (fresh.blockers.length > 0) {
      if (fresh.blockers.some((blocker) => blocker.code === 'already_running')) throw new Refusal(409, 'conflict.exists', 'a move holds this environment already', { job_id: this.openJobFor({ environment: environmentId })?.id ?? null });
      throw new Refusal(409, 'plan.blocked', fresh.blockers[0]?.message ?? 'the plan cannot be carried out', { blockers: fresh.blockers });
    }
    this.plans.delete(asText(body.preflight_id));
    return this.wire(this.makeEnvironmentJob(person, record, 'environment_move', 'move', null, hostId, machineId, typeof stored.askedFor?.template_id === 'string' ? stored.askedFor.template_id : null));
  }

  private preflightNodeBatch(person: Person, nodeId: string, moves: boolean, body: Record<string, unknown>): unknown {
    const ids = asList(body.environment_ids).filter((entry): entry is string => typeof entry === 'string');
    const plans: (EnvironmentUpdatePlan | EnvironmentMovePlan)[] = [];
    const skipped: { environment: EnvironmentRef; reason: string }[] = [];
    for (const id of ids) {
      const record = this.environment(id);
      if (record === undefined) continue;
      if (!this.mayManage(person, id)) {
        skipped.push({ environment: this.environmentRef(id), reason: 'no_permission' });
        continue;
      }
      if (!moves && record.updates === 0 && record.machine === null) {
        skipped.push({ environment: this.environmentRef(id), reason: 'up_to_date' });
        continue;
      }
      if (!moves && record.machine !== null) {
        skipped.push({ environment: this.environmentRef(id), reason: 'not_updatable' });
        continue;
      }
      plans.push(moves ? this.moveRequestPlan(person, record, body) : this.updatePlan(person, record, 'update', null));
    }
    const made = {
      estimate: this.estimate(plans.length * 60, true),
      executable: plans.filter((plan) => plan.blockers.length === 0).length,
      expires_at: iso(this.now() + PLAN_TTL_MS),
      options: this.options(null),
      plans,
      preflight_id: randomUUID(),
      skipped,
    };
    this.keep(person, moves ? 'node-move' : 'node-update', nodeId, made, { askedFor: { ...body } });
    return made;
  }

  private confirmNodeBatch(person: Person, moves: boolean, body: Record<string, unknown>): { batch_id: string; jobs: { environment: EnvironmentRef; job: Job }[] } {
    const stored = this.take(person, asText(body.preflight_id), moves ? 'node-move' : 'node-update');
    const made = stored.plan as { plans: (EnvironmentUpdatePlan | EnvironmentMovePlan)[]; executable: number };
    if (made.executable === 0) throw new Refusal(409, 'plan.blocked', 'none of the plans can be carried out', { blockers: made.plans[0]?.blockers ?? [] });
    if (moves) {
      made.plans = made.plans.filter((plan) => plan.blockers.length === 0).map((plan) => this.moveRequestPlan(person, this.mustEnvironment(person, plan.environment.id), stored.askedFor ?? {}));
      const blocked = made.plans.find((plan) => plan.blockers.length > 0);
      if (blocked !== undefined) throw new Refusal(409, 'plan.blocked', blocked.blockers[0]?.message ?? 'the plan cannot be carried out', { blockers: blocked.blockers });
    }
    const batch: BatchRecord = { id: randomUUID(), by: person.me.username, jobs: [], hidden: new Set() };
    this.batches.set(batch.id, batch);
    const jobs: { environment: EnvironmentRef; job: Job }[] = [];
    for (const plan of made.plans) {
      if (plan.blockers.length > 0) continue;
      const record = this.environment(plan.environment.id);
      if (record === undefined) continue;
      const destination = moves ? (plan as EnvironmentMovePlan).to : null;
      const hostId = destination !== null && !('machine' in destination) ? destination.host.id : null;
      const machineId = destination !== null && 'machine' in destination ? destination.machine.id : null;
      const job = moves ? this.makeEnvironmentJob(person, record, 'environment_move', 'move', batch.id, hostId, machineId, typeof stored.askedFor?.template_id === 'string' ? stored.askedFor.template_id : null) : this.makeEnvironmentJob(person, record, 'environment_recreate', 'update', batch.id);
      batch.jobs.push(job.id);
      jobs.push({ environment: plan.environment, job: this.wire(job) });
    }
    this.plans.delete(asText(body.preflight_id));
    return { batch_id: batch.id, jobs };
  }

  // ---------------------------------------------------------------- recycle

  private fleetEntry(record: NonNullable<ReturnType<Change['environment']>>, person: Person): FleetEntry {
    const idle = this.idle.get(record.id) ?? 0;
    return {
      active_turns: 0,
      cpu_percent: 2,
      disk_used_bytes: 12 * GB,
      environment: this.environmentRef(record.id),
      generation: 1,
      holds: [],
      home: this.nodeRef(ACME.org),
      host_name: this.host(record.host ?? '')?.name ?? null,
      idle_minutes: idle,
      idle_stop_at: null,
      last_activity_at: iso(Date.now() - idle * 60_000),
      manageable: this.mayManage(person, record.id),
      memory_bytes: 1.4 * GB,
      memory_limit_bytes: 4 * GB,
      open_shells: 0,
      processes: 3,
      template: null,
      threads: [...this.resources.bindings].filter(([, binding]) => binding.environment === record.id).length,
      updates: record.updates,
    };
  }

  private recycle(person: Person, scope: string, body: Record<string, unknown>, host = false): RecycleReply {
    const minutes = typeof body.idle_minutes === 'number' ? body.idle_minutes : 60;
    const dry = body.dry_run !== false;
    const withProcesses = body.include_with_processes === true;
    const all = this.resources.environments.filter((record) => record.status === 'running' && record.host !== null && (!host || record.host === scope));
    const idle = all.filter((record) => (this.idle.get(record.id) ?? 0) >= minutes);
    const allowed = idle.filter((record) => this.mayManage(person, record.id));
    const stopping = allowed.filter((record) => withProcesses || (this.idle.get(record.id) ?? 0) >= minutes);
    const reply: RecycleReply = {
      candidates: stopping.map((record) => this.fleetEntry(record, person)),
      skipped_active: all.length - idle.length,
      skipped_no_permission: idle.length - allowed.length,
      stopped: dry ? 0 : stopping.length,
    };
    if (!dry) for (const record of stopping) record.status = 'stopped';
    return reply;
  }

  // ---------------------------------------------------------------- side threads

  private startSide(person: Person, parentId: string, body: Record<string, unknown>): { input: ThreadInput; seq: number; thread: Thread } {
    if (this.sides.has(parentId)) throw new Refusal(409, 'thread.side', 'a side thread has no side threads of its own');
    const parent = this.threadOf(person, parentId);
    this.world.places.need(person, parentId, 'thread.interact');
    const clientId = asText(body.client_id);
    const repeated = [...this.sides.values()].find((side) => side.parent === parentId && side.clientIds.has(`${person.me.username}:${clientId}`));
    if (repeated !== undefined) {
      const inputId = repeated.clientIds.get(`${person.me.username}:${clientId}`) ?? '';
      return { input: repeated.inputs.get(inputId)!, seq: repeated.seq, thread: this.threadOf(person, repeated.thread.id) };
    }
    if (parent.sides?.allowed !== true || parent.sides.mode === null) throw new Refusal(409, 'capability.unsupported', parent.sides?.reason ?? 'This agent has no safe side mode.');
    const open = [...this.sides.values()].filter((side) => side.parent === parentId && side.state === 'open');
    if (open.length >= SIDE_MAX) throw new Refusal(409, 'thread.side_limit', `${parent.name} has ${open.length} side threads open`, { open: open.length, max: SIDE_MAX });
    const everMade = [...this.sides.values()].filter((side) => side.parent === parentId).length;
    const id = fixtureId(0x2c, this.sides.size + 1);
    const toTurn = typeof body.to_turn === 'number' ? body.to_turn : null;
    const name = `${parent.name} (side${everMade === 0 ? '' : ` ${everMade + 1}`})`;
    const thread: Thread = {
      ...parent,
      id,
      name,
      kind: 'thread',
      parent_id: parentId,
      side_of: this.threadRef(parentId),
      side_from_turn: toTurn,
      sides: null,
      side_reading: parent.sides.mode,
      state: 'idle',
      blocked: null,
      lock: null,
      current_turn_id: null,
      pending_inputs: 0,
      pending_approvals: 0,
      owner: this.user(person.me.username),
      my_last_read_seq: null,
      seq: 100,
    };
    const side: SideRecord = { parent: parentId, thread, maker: person, state: 'open', closedWhy: null, keptAs: null, items: new Map(), turns: new Map(), inputs: new Map(), segments: [], seq: 100, clientIds: new Map(), followers: new Map() };
    this.sides.set(id, side);
    const input = this.sideInput(person, id, { content: body.content, client_id: clientId }).input;
    side.clientIds.set(`${person.me.username}:${clientId}`, input.id);
    this.world.threads.emit(parentId, { items: [marker(randomUUID(), { marker: 'side_started', data: { by: this.user(person.me.username), from_turn: toTurn ?? 0, side: this.threadRef(id) } })], thread: { sides: { allowed: true, max: SIDE_MAX, open: open.length + 1, mode: 'read_only', reason: null } } });
    return { input, seq: side.seq, thread: this.threadOf(person, id) };
  }

  private listSides(person: Person, parentId: string, state: 'open' | 'closed'): Thread[] {
    this.world.places.need(person, parentId, 'thread.view');
    return [...this.sides.values()].filter((side) => side.parent === parentId && side.state === state).map((side) => ({ ...side.thread, seq: side.seq }));
  }

  private sideOf(id: string): SideRecord {
    const side = this.sides.get(id);
    if (side === undefined) throw new Refusal(404, 'not_found', 'there is no such thread');
    return side;
  }

  private sideSync(person: Person, id: string, since: number | null): ThreadSync {
    const side = this.sideOf(id);
    this.world.places.need(person, side.parent, 'thread.view');
    return {
      complete: true,
      has_older: false,
      inputs: [...side.inputs.values()].filter((entry) => ['pending', 'delivering', 'failed'].includes(entry.state)),
      items: [...side.items.values()],
      segments: side.segments,
      seq: side.seq,
      since,
      thread: this.threadOf(person, id),
      turns: [...side.turns.values()],
    };
  }

  private sideInput(person: Person, id: string, body: Record<string, unknown>): { input: ThreadInput; seq: number } {
    const side = this.sideOf(id);
    this.world.places.need(person, side.parent, 'thread.interact');
    if (side.state === 'closed') throw new Refusal(409, 'node.archived', 'the side thread is closed');
    const content = asList(body.content) as ThreadInput['content'];
    const text = (content as { type: string; text?: string }[]).map((part) => part.text ?? '').join(' ');
    const ordinal = side.turns.size + 1;
    const input: ThreadInput = {
      native_action: null,
      attachment_issues: [],
      author: this.user(person.me.username),
      client_id: asText(body.client_id) === '' ? null : asText(body.client_id),
      content,
      created_at: iso(Date.now()),
      delivered_at: iso(Date.now()),
      delivery: 'queue',
      id: randomUUID(),
      limit_bypass: [],
      plan: false,
      position: String(ordinal),
      queue_held: false,
      run_as: null,
      seq_updated: 0,
      source: 'user',
      state: 'delivered',
      state_detail: null,
      thread_id: id,
      turn_id: null,
    };
    side.inputs.set(input.id, input);
    const turnId = randomUUID();
    const bound = this.bindingOfAny(id);
    const turn = (state: Turn['state'], phase: Turn['phase']): Turn => ({
      before_snapshot_id: null,
      command_cleanup: null,
      usage_detail: null,
      context: null,
      completed_at: state === 'completed' ? iso(Date.now()) : null,
      effort: bound.effort,
      error: null,
      id: turnId,
      input_id: input.id,
      interrupted_by: null,
      limit_bypass: [],
      model: bound.model,
      model_display: bound.model,
      ordinal,
      params: {},
      phase,
      plan: false,
      segment_id: '',
      seq_updated: 0,
      started_at: iso(Date.now()),
      waiting_since: null,
      waited_ms: 0,
      state,
      thread_id: id,
      usage: null,
    });
    const first = userMessage(randomUUID(), text, { author: this.user(person.me.username), inputId: input.id, turn: turnId });
    this.sideEmit(side, { inputs: [{ ...input, turn_id: turnId }], items: [first], turns: [turn('running', { kind: 'working', since: iso(Date.now()) })], thread: { state: 'running', current_turn_id: turnId } });
    const timer = setTimeout(() => {
      this.timers.delete(timer);
      if (side.state === 'closed' || side.turns.get(turnId)?.state !== 'running') return;
      const answer = `On the side, about “${text.slice(0, 60)}”: the same files, the same agent. Nothing here is added to ${side.thread.name.replace(/ \(side[^)]*\)$/, '')}.`;
      this.sideEmit(side, {
        items: [{ ...marker(randomUUID(), { marker: 'compacted', data: { duration_ms: null } }), id: `${turnId}-a`, kind: 'agent_message', turn_id: turnId, payload: { phase: 'final', text: answer, text_len: new TextEncoder().encode(answer).length } } as unknown as Item],
        turns: [turn('completed', null)],
        thread: { state: 'idle', current_turn_id: null },
      });
    }, SIDE_ANSWER_MS);
    this.timers.add(timer);
    return { input, seq: side.seq };
  }

  private sideInterrupt(person: Person, id: string): Turn {
    const side = this.sideOf(id);
    this.world.places.need(person, side.parent, 'thread.interact');
    const running = [...side.turns.values()].find((entry) => entry.state === 'running');
    if (running === undefined) throw new Refusal(409, 'thread.not_running', 'nothing runs in this side thread');
    const stopped: Turn = { ...running, state: 'interrupted', completed_at: iso(Date.now()), phase: null, interrupted_by: this.user(person.me.username) };
    this.sideEmit(side, { turns: [stopped], thread: { state: 'idle', current_turn_id: null } });
    return stopped;
  }

  private sideEmit(side: SideRecord, rows: { items?: Item[]; turns?: Turn[]; inputs?: ThreadInput[]; thread?: Partial<Thread> }): void {
    for (const item of rows.items ?? []) {
      side.seq += 1;
      side.items.set(item.id, { ...item, thread_id: side.thread.id, segment_id: item.segment_id === '' ? SEGMENT_OF(side.thread.id) : item.segment_id, seq_created: side.items.get(item.id)?.seq_created ?? side.seq, seq_updated: side.seq });
    }
    for (const turn of rows.turns ?? []) {
      side.seq += 1;
      side.turns.set(turn.id, { ...turn, thread_id: side.thread.id, segment_id: SEGMENT_OF(side.thread.id), seq_updated: side.seq });
    }
    for (const input of rows.inputs ?? []) {
      side.seq += 1;
      side.inputs.set(input.id, { ...input, seq_updated: side.seq });
    }
    if (side.segments.length === 0) {
      const refs = this.resources.refsOf(this.bindingOfAny(side.thread.id));
      side.segments.push({ agent: refs.agent!, arrangement: 'separated', credential_path: 'native', end_reason: null, ended_at: null, env_generation: 1, environment: refs.environment!, executor: null, id: SEGMENT_OF(side.thread.id), import_fidelity: 'lossless', import_report: null, ordinal: 1, provider: refs.provider!, seq_updated: side.seq, started_at: iso(Date.now()), started_by: null, tool_set: null });
      side.thread = { ...side.thread, current_segment_id: SEGMENT_OF(side.thread.id) };
    }
    if (rows.thread !== undefined) side.thread = { ...side.thread, ...rows.thread };
    side.seq += 1;
    for (const [subscriber, follower] of side.followers) {
      const person = this.world.people.get(follower.user);
      if (person === undefined) continue;
      let thread: Thread;
      try { thread = this.threadOf(person, side.thread.id); }
      catch (error) {
        if (!(error instanceof Refusal) || ![403, 404].includes(error.answer.status)) throw error;
        subscriber.send({ op: 'closed', id: follower.id, feed: `thread:${side.thread.id}`, reason: 'forbidden' });
        side.followers.delete(subscriber);
        continue;
      }
      subscriber.send({
        op: 'sync',
        id: follower.id,
        feed: `thread:${side.thread.id}`,
        prev: follower.last,
        seq: side.seq,
        data: { items: rows.items?.map((item) => side.items.get(item.id)), turns: rows.turns?.map((turn) => side.turns.get(turn.id)), inputs: rows.inputs?.map((input) => side.inputs.get(input.id)), thread, segments: side.segments },
      });
      follower.last = side.seq;
    }
  }

  private closeSide(person: Person, id: string, why: 'closed' | 'idle' | 'kept' | 'parent', into: string | null = null): Answer {
    const side = this.sideOf(id);
    if (side.state === 'closed') return { status: 204 };
    const maker = side.maker.me.username === person.me.username;
    const manager = this.world.places.access(person, side.parent).permissions.includes('thread.manage');
    if (!maker && !manager) this.world.places.need(person, side.parent, 'thread.manage');
    side.state = 'closed';
    side.closedWhy = why;
    side.keptAs = into;
    const turns = side.turns.size;
    const open = [...this.sides.values()].filter((entry) => entry.parent === side.parent && entry.state === 'open').length;
    this.world.threads.emit(side.parent, {
      items: [marker(randomUUID(), { marker: 'side_closed', data: { by: this.user(person.me.username), into: into === null ? null : this.threadRef(into), side: this.threadRef(id), turns, why } })],
      thread: { sides: { allowed: true, max: SIDE_MAX, open, mode: 'read_only', reason: null } },
    });
    for (const [subscriber, follower] of side.followers) subscriber.send({ op: 'closed', id: follower.id, feed: `thread:${id}`, reason: 'deleted' });
    side.followers.clear();
    return { status: 204 };
  }

  // ---------------------------------------------------------------- the live feed of a side thread

  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const feed = asText(frame.feed);
    const id = Number(frame.id);
    const sideId = /^thread:(.+)$/.exec(feed)?.[1];
    switch (frame.op) {
      case 'sub': {
        if (sideId === undefined || !this.sides.has(sideId)) return false;
        const side = this.sideOf(sideId);
        if (person === undefined || !this.world.places.access(person, side.parent).permissions.includes('thread.view')) {
          subscriber.send({ op: 'error', id, code: 'forbidden', detail: feed });
          return true;
        }
        const since = typeof frame.since === 'number' ? frame.since : null;
        const reload = since !== null && since > side.seq;
        side.followers.set(subscriber, { id, last: reload ? side.seq : (since ?? side.seq), user: person.me.username });
        subscriber.send({ op: 'subscribed', id, feed, seq: side.seq, mode: reload ? 'reload' : 'delta' });
        subscriber.send({ op: 'presence', id, feed, users: [] });
        if (!reload && since !== null && since < side.seq) {
          const rows = this.sideSync(person, sideId, since);
          subscriber.send({ op: 'sync', id, feed, prev: since, seq: side.seq,
            data: { thread: rows.thread, items: rows.items, turns: rows.turns, inputs: rows.inputs, segments: rows.segments } });
          const follower = side.followers.get(subscriber);
          if (follower !== undefined) follower.last = side.seq;
        }
        return true;
      }
      case 'unsub': {
        for (const [key, side] of this.sides) {
          if (side.followers.get(subscriber)?.id !== id) continue;
          side.followers.delete(subscriber);
          subscriber.send({ op: 'closed', id, feed: `thread:${key}`, reason: 'unsubscribed' });
          return true;
        }
        return false;
      }
      case 'visible':
        return sideId !== undefined && this.sides.has(sideId);
      default:
        return false;
    }
  }

  gone(subscriber: Subscriber): void {
    for (const side of this.sides.values()) side.followers.delete(subscriber);
  }

  // ---------------------------------------------------------------- what only a test can do

  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/measurement': {
        const now = new Date().toISOString();
        const state = body.state === 'succeeded' ? 'succeeded' : body.state === 'failed' ? 'failed' : body.state === 'cancelled' ? 'cancelled' : 'running';
        const phase = typeof body.phase === 'string' ? body.phase : 'fingerprint_source';
        const files = typeof body.files === 'number' ? body.files : 0;
        const bytes = typeof body.bytes === 'number' ? body.bytes : 0;
        const checked = typeof body.checkedAt === 'string' ? body.checkedAt : now;
        const job: Job = {
          id: asText(body.id), kind: 'measurement', state, node_id: null, environment_id: null, provider_id: null, batch_id: null, created_by: null,
          created_at: typeof body.startedAt === 'string' ? body.startedAt : now,
          started_at: typeof body.startedAt === 'string' ? body.startedAt : now,
          finished_at: state === 'running' ? null : checked, log: null,
          error: state === 'failed' ? { code: 'read_failed', message: asText(body.message), phase, undone: true } : null,
          result: state === 'succeeded' ? phase.startsWith('fingerprint_') ? { matches: body.matches === true, checked_at: checked } : { file_count: files, total_bytes: bytes, file_states: null } : null,
          progress: { phase, phases: [{ name: phase, state: state === 'running' ? 'running' : 'done', weight: 100 }], percent: state === 'succeeded' ? 100 : 0,
            files_done: files, files_total: null, bytes_done: bytes, bytes_total: null, rate_bps: null, eta_seconds: null, message: null,
            activity: { updated_at: typeof body.updatedAt === 'string' ? body.updatedAt : now, stall_after_seconds: typeof body.stall === 'number' ? body.stall : 30, directories_done: 2, stopping: false, waiting_for: null } },
        };
        this.measurements.set(job.id, job);
        return { status: 200, body: job };
      }
      case '/mode':
        this.mode = body.mode === 'manual' ? 'manual' : 'auto';
        return { status: 204 };
      case '/step':
        this.step(typeof body.job === 'string' ? body.job : null, typeof body.count === 'number' ? body.count : 1);
        return { status: 200, body: { jobs: [...this.jobs.values()].map((record) => this.wire(record)) } };
      case '/finish':
        this.finish(typeof body.job === 'string' ? body.job : null);
        return { status: 200, body: { jobs: [...this.jobs.values()].map((record) => this.wire(record)) } };
      case '/fail':
        // The next job that reaches this phase ends there with this code, and what it made is taken away again.
        this.failing = body.phase === undefined ? null : { phase: asText(body.phase), code: asText(body.code) === '' ? 'verify_failed' : asText(body.code), message: asText(body.message) === '' ? 'tools could not be reached' : asText(body.message), undone: body.undone !== false };
        return { status: 204 };
      case '/expire':
        for (const stored of this.plans.values()) stored.expiresAt = 0;
        return { status: 204 };
      case '/rebind': {
        // A thread is bound to another environment, as a switch that was made earlier would have left it.
        const threadId = asText(body.thread);
        const bound = this.resources.bindings.get(threadId);
        const environment = this.environment(asText(body.environment));
        if (bound === undefined || environment === undefined) return { status: 404, body: { detail: 'no such thread or environment' } };
        this.resources.bind(threadId, { ...bound, environment: environment.id });
        return { status: 204 };
      }
      case '/uses':
        // Who may use a provider, as the step that opens it to the work left it.
        return { status: 200, body: { uses: [...(this.provider(asText(body.provider))?.uses ?? [])] } };
      case '/clock':
        // The clock the plans are dated by runs this many minutes ahead, as the clock of a page that was moved forward does.
        this.skewMs = (typeof body.minutes === 'number' ? body.minutes : 0) * 60_000;
        return { status: 204 };
      case '/jobs':
        return { status: 200, body: [...this.jobs.values()].map((record) => this.wire(record)) };
      case '/hide':
        for (const thread of asList(body.threads)) if (typeof thread === 'string') this.hidden.add(thread);
        return { status: 204 };
      case '/idle':
        for (const [environment, minutes] of Object.entries(body)) if (typeof minutes === 'number') this.idle.set(environment, minutes);
        return { status: 204 };
      case '/declare':
        for (const [host, megabytes] of Object.entries(body)) if (typeof megabytes === 'number') this.declared.set(host, megabytes);
        return { status: 204 };
      case '/environment': {
        const record = this.environment(asText(body.id));
        if (record === undefined) return { status: 404, body: { detail: 'no such environment' } };
        if (typeof body.status === 'string') record.status = body.status as typeof record.status;
        if (typeof body.updates === 'number') record.updates = body.updates;
        if (typeof body.host === 'string') record.host = body.host;
        if (Array.isArray(body.manages)) record.manages = body.manages.filter((entry): entry is string => typeof entry === 'string');
        return { status: 204 };
      }
      case '/host': {
        const record = this.host(asText(body.id));
        if (record === undefined) return { status: 404, body: { detail: 'no such host' } };
        if (typeof body.schedulable === 'boolean') record.schedulable = body.schedulable;
        if (body.when_full === 'warn' || body.when_full === 'block') record.whenFull = body.when_full;
        return { status: 204 };
      }
      case '/hold': {
        // Somebody else's switch of a thread is under way: the thread is held, and any other plan for it is blocked.
        const ada = this.world.people.get('ada');
        const threadId = asText(body.thread);
        if (ada === undefined || this.world.places.rowOf(threadId)?.kind !== 'thread') return { status: 404, body: { detail: 'no such thread' } };
        const providerId = asText(body.provider);
        const model = asText(body.model) === '' ? (this.provider(providerId)?.models.find((entry) => entry.enabled)?.model_name ?? '') : asText(body.model);
        const { plan, asked } = this.draftFork(ada, threadId, { result: 'replace', to: { provider_id: providerId, model, effort: null } });
        if (plan.blockers.length > 0) return { status: 409, body: { detail: plan.blockers.map((blocker) => blocker.message).join('; ') } };
        const record = this.makeFork(ada, threadId, plan, asked, null);
        return { status: 200, body: { job: record.id } };
      }
      case '/sides': {
        // Open side threads, made at once: for the limit of how many a thread may have.
        const ada = this.world.people.get('ada');
        const threadId = asText(body.thread);
        if (ada === undefined) return { status: 404, body: { detail: 'no such person' } };
        const count = typeof body.count === 'number' ? body.count : 1;
        for (let n = 0; n < count; n += 1) this.startSide(ada, threadId, { content: [{ type: 'text', text: `A question on the side, number ${n + 1}` }], client_id: randomUUID() });
        return { status: 204 };
      }
      case '/model': {
        // A model of a provider is switched on or off behind the back of a list that was read before: what a stale list still offers.
        const provider = this.provider(asText(body.provider));
        const model = provider?.models.find((entry) => entry.model_name === asText(body.model));
        if (model === undefined) return { status: 404, body: { detail: 'no such model' } };
        model.enabled = body.enabled !== false;
        return { status: 204 };
      }
      case '/close-idle-sides':
        for (const side of this.sides.values()) if (side.state === 'open') this.closeSide(side.maker, side.thread.id, 'idle');
        return { status: 204 };
      default:
        return null;
    }
  }
}

/** The segment every row of a side thread belongs to. */
function SEGMENT_OF(threadId: string): string {
  return `00000000-0000-4000-8${(0x41).toString(16).padStart(3, '0')}-${threadId.slice(-12)}`;
}

/** The identifiers a plan names by number, for a test that wants to find them. */
export const CHANGE_IDS = { team: RUNS_ON } as const;
