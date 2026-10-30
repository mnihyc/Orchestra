/** What the fake API says of resources: machines with their enrollment and policy, providers with their models and key, agent profiles, environments, hosts and templates, and the acts that register, change and remove them.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same shapes,
 * the same refusals with what is missing and whom to ask, the same revisions. Its people are those of the
 * world (`ada` administers Acme and owns most of what is here, `dora` is a developer, `vera` only reads);
 * the requests under `/__double/resources` steer it the way no real system can: a machine that enrolls,
 * one that goes away.
 */
import { randomUUID } from 'node:crypto';
import { seedPlan,planReply,type PlanSeed } from './plan-usage.ts';
import { readFileSync } from 'node:fs';

import type {
  EverythingResource,
  ActivityGroup,
  ResourceImpact,
  AccessSummary,
  AgentReachOverview,
  Agent,
  AgentRef,
  Arrangement,
  ArrangementConsent,
  ArrangementType,
  BindingAgent,
  BindingEnvironment,
  BindingOptions,
  BindingProvider,
  EnrollmentToken,
  EnrollmentProgress,
  ComponentUpdate,
  FactMember,
  Environment,
  EnvironmentMcp,
  Job,
  EnvironmentRef,
  Host,
  Isolation,
  JobRef,
  Machine,
  MachinePolicy,
  MachinePolicySection,
  MachineProgram,
  MachineProgramKind,
  MachinePrograms,
  MachineInstallation,
  MachineRef,
  MachineEnvironmentData,
  EnvironmentDataSources,
  EnvironmentDataStorage,
  MissingAccess,
  ModelOption,
  ModelSuggestion,
  Provider,
  ProviderModel,
  ProviderRef,
  ProviderSetup,
  SourceChoice,
  SourceDescriptor,
  ResolvedSetting,
  ResourceAccess,
  Runtime,
  Template,
  UserRef,
} from '../../src/generated/api-types.ts';
import { ACME, id as fixtureId, KIND, LATER_AT, MADE_AT, MODEL_ESTIMATE, RUNS_ON, USUAL_MODEL, userRef } from './fixtures.ts';
import { acceptedEntry, checkEntry } from './model.ts';
import { type Answer, type Person, Refusal, type World } from './world.ts';
import { hostTemplateSchema, templateSchema, templateProblems } from './template-specs.ts';
import { EXECUTORS, MACHINE_EXECUTORS } from './executors.ts';
import { harnessName, maybeHarnessName } from './harness-names.ts';

/** A time that many milliseconds from now, written as the API writes it. */
const inMs = (ms: number): string => new Date(Date.now() + ms).toISOString();
/** How long a job that makes or starts an environment takes in the double: long enough to be seen, short enough to wait for. */
const FINISH_MS = 1_200;
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

const asText = (value: unknown): string => (typeof value === 'string' ? value : '');
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

function dataSources(value: unknown): EnvironmentDataSources | null {
  if (!isRecord(value)) return null;
  const source = (entry: unknown): EnvironmentDataSources['home'] => isRecord(entry) && entry.kind === 'existing_directory' && typeof entry.path === 'string'
    ? { kind: 'existing_directory', path: entry.path } : { kind: 'made_by_orch' };
  return { workspace: source(value.workspace), home: source(value.home) };
}
function directoryStorage(record: MachineRecord): EnvironmentDataStorage {
  const declared = Array.isArray(record.policy['environment.roots']) && record.policy['environment.roots'].length > 0;
  return { made_by_orch: declared ? 'directory' : null, existing_directory: true, reason: declared ? null : 'No storage has been declared under environment.roots.' };
}

/** The fake API exposes reported provider facts without legacy credit/use inputs. */
function emptyProviderFacts():Provider['facts'] {
  return {models:null,available_resets:null,extra_usage:null,balances:[],spend:[],ordinary_usage_allowed:null,credential_verified:null,field_times:{},plan:null,windows:[],limit_reached:null,identity:null,credential:null};
}
const SOURCE_MEMBERS:readonly FactMember[]=['identity','credential','credential_verified','plan','windows','limit_reached','ordinary_usage_allowed','balances','spend','extra_usage','available_resets'];
function sourceDescriptor(name:string,title:string,reports:FactMember[] | null,models=false):SourceDescriptor {
  return {name,title,summary:models ? 'Lists models from the endpoints.' : 'Reports the details its keeper supplies.',contract:1,facts:reports === null ? null : {reports,spends_resets:false,credential:'none',min_interval_secs:null,plans:{}},models:models ? {} : null,presents:null,fields:[],docs_url:null};
}
function builtinSourceOffers():ProviderSetup['sources'] {
  return [
    {by:'builtin',name:'adapter',descriptor:sourceDescriptor('adapter','Harness adapter',[...SOURCE_MEMBERS],true),default_for:[],machine:null,template:null,command:null},
    {by:'builtin',name:'endpoints',descriptor:sourceDescriptor('endpoints','Endpoints',null,true),default_for:[],machine:null,template:null,command:null},
    {by:'builtin',name:'deepseek',descriptor:sourceDescriptor('deepseek','DeepSeek',['balances','ordinary_usage_allowed']),default_for:['api.deepseek.com'],machine:null,template:null,command:null},
  ];
}

/** The local-policy keys in the order a machine lists them, with what each bounds. */
const POLICY_KEYS = [
  ['drivers', 'machine'],
  ['storages', 'machine'],
  ['resources.max', 'machine'],
  ['hooks', 'machine'],
  ['ports', 'machine'],
  ['environment.roots', 'environment'],
  ['environment.remote_write', 'environment'],
  ['environment.terminal', 'environment'],
  ['environment.after_up', 'environment'],
  ['environment.browser', 'environment'],
  ['environment.vnc', 'environment'],
  ['mcp.import', 'environment'],
  ['mcp.add', 'environment'],
  ['exec.offers', 'environment'],
  ['environment.deliver_files', 'environment'],
  ['agent.max_sandbox', 'environment'],
  ['provider.upstreams', 'provider'],
  ['provider.signin', 'machine'],
  ['provider.arrangements', 'provider'],
  ['agent.harnesses', 'machine'],
  ['agent.programs', 'machine'],
  ['provider.programs', 'machine'],
  ['machine.roles', 'machine'],
  ['tools.fetch', 'machine'],
  ['update.auto', 'machine'],
  ['policy.delegated', 'machine'],
] as const;

function shippedPolicy(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    drivers: {allowed:[],docker:{images:[],registries:[],capabilities:[],runtime_classes:[],privileged:false,builds:false,archives:false},host:{templates:[],accounts:[],programs:[]},configurations:{}},
    storages: [],
    'resources.max': {},
    hooks: {pre_up:null,post_up:null,pre_down:null,post_down:null,timeout_secs:30},
    ports: {allowed:[],publish:null},
    'environment.roots': ['/home/ada/work'],
    'environment.remote_write': true,
    'environment.terminal': false,
    'environment.after_up': false,
    'environment.browser': false,
    'environment.vnc': false,
    'mcp.import': true,
    'mcp.add': false,
    'exec.offers': ['codex-exec-server', 'claude-code', 'plain', 'custom'],
    'environment.deliver_files': true,
    'agent.max_sandbox': 'workspace',
    'provider.upstreams': ['https://api.openai.com/v1'],
    'provider.signin': false,
    'provider.arrangements': ['separated', 'key_separated'],
    'agent.harnesses': [],
    'agent.programs': false,
    'provider.programs': false,
    'machine.roles': false,
    'tools.fetch': false,
    'update.auto': true,
    'policy.delegated': [],
    ...overrides,
  };
}

function policySections(key: string): MachinePolicySection[] {
  if (key === 'environment.roots') return ['environment', 'node'];
  if (['drivers', 'storages', 'hooks', 'resources.max'].includes(key)) return ['node'];
  if (key === 'ports') return ['environment'];
  if (key === 'agent.harnesses' || key === 'agent.programs' || key === 'tools.fetch') return ['agent'];
  if (key.startsWith('provider.')) return ['provider'];
  if (key.startsWith('environment.') || key.startsWith('mcp.') || key === 'exec.offers' || key === 'agent.max_sandbox') return ['environment'];
  return ['machine'];
}

function policyDefinition(key: string): MachinePolicy['keys'][number]['definition'] {
  const cautious = shippedPolicy({'environment.roots':[], 'environment.remote_write':false, 'provider.upstreams':[]});
  const managed: Record<string,unknown> = {...cautious, ...Object.fromEntries(['environment.remote_write','environment.terminal','environment.after_up','environment.browser','environment.vnc','mcp.add','provider.signin','agent.programs','provider.programs','machine.roles'].map((name) => [name,true])),
    'agent.max_sandbox':'open', 'provider.upstreams':['*'], 'provider.arrangements':['separated','key_separated','credential_shared','handed_over'], ports:{allowed:['place','link','host'],publish:null}};
  return {key, title:key, meaning:`The machine's ${key} setting.`, sections:policySections(key), managed:managed[key] ?? null, cautious:cautious[key] ?? null, delegable:key !== 'policy.delegated'};
}

interface MachineRecord {
  programs?:MachineProgram[];
  dedicated?: boolean;
  id: string;
  name: string;
  owner: string;
  os: 'linux' | 'macos' | 'windows';
  arch: string;
  online: boolean;
  status: 'active' | 'revoked';
  connector: string;
  lastSeen: string | null;
  roundTripMs: number;
  harnesses: { kind: string; path: string; version: string; daemon: boolean }[];
  policy: Record<string, unknown>;
  roles?: Machine['roles'];
  /** When its certificate ends, as a distance from when the world began (negative: it has ended). Absent: a good certificate, or the one a revoked machine had. */
  certEnds?: number;
}

type ModelRecord = ProviderModel;

interface EnrollmentSelection { kind: 'connector' | 'node'; roles: string[]; delegate: string[]; dedicated: boolean; home?: string | null }

interface ProviderRecord {
  /** Native reports are shared by HTTP reads and the resource feed. */
  harnessReports?: Provider['harnesses'];
  planUsage?:PlanSeed;
  id: string;
  name: string;
  kind: 'api' | 'subscription';
  harness: string | null;
  owner: string;
  home: string;
  machine: string | null;
  keyMachine: string | null;
  keyHeldByOrch?:boolean;
  placement: 'managed' | 'connected';
  status: Provider['status'];
  statusDetail: string | null;
  arrangements: ArrangementType[];
  upstreams: Provider['upstreams'];
  models: ModelRecord[];
  keySet: boolean;
  secrets: string[];
  facts: Provider['facts'];
  sources?: Provider['sources'];
  runtimeConfig?: Record<string,unknown>;
  running: number;
  /** Who may use it besides its owner, and who may manage it. */
  uses: string[];
  manages: string[];
  agent: string | null;
  revision: number;
  createdAt: string;
  heldBy: string | null;
}

function defaultProviderSource(record:ProviderRecord,element:'facts' | 'models',offers:ProviderSetup['sources']):SourceChoice {
  const offer=record.harness !== null ? offers.find((source) => source.name === 'adapter') : element === 'models' ? offers.find((source) => source.name === 'endpoints') : offers.find((source) => source.default_for.some((host) => record.upstreams.some((endpoint) => new URL(endpoint.base_url).hostname === host)));
  return {by:'default',name:offer?.name ?? null,address:null,config:{},secrets:[],descriptor:offer?.descriptor ?? null,described_at:null,last_error:null};
}

interface AgentRecord {
  id: string;
  name: string;
  harness: string;
  owner: string | null;
  home: string | null;
  machine: string | null;
  disabled: boolean;
  revision: number;
  profile: Record<string, unknown>;
  descriptor: Record<string, unknown>;
}

interface EnvironmentRecord {
  /** The verified unit generation after a retained-volume restoration. */
  generation?: number;
  id: string;
  name: string;
  owner: string;
  home: string;
  machine: string | null;
  host: string | null;
  template: string | null;
  status: Environment['status'] | 'removed';
  root: string;
  remoteWrite: boolean;
  terminal: boolean;
  revision: number;
  uses: string[];
  manages: string[];
  updates: number;
  removeAt: string | null;
  /** How the files of its workspace are known: all of them (`full`), or only those looked at (`live`, for a workspace too large to index). */
  index?: 'full' | 'live';
  /** What its own settings say, by key (`env.idle_stop_minutes`). */
  config?: Record<string, unknown>;
  temporary?: { expiresAt: string; hours: number; thread: string | null };
}

interface HostRecord {
  id: string;
  name: string;
  online?: boolean;
  offlineSince?: string | null;
  owner: string | null;
  home: string | null;
  labels: string[];
  schedulable: boolean;
  whenFull: 'warn' | 'block' | null;
  cleanupDays?: number | null;
  revision: number;
  uses: string[];
  manages: string[];
}

interface TemplateRecord {
  id: string;
  name: string;
  description: string;
  kind: 'environment' | 'provider';
  driver: string;
  owner: string | null;
  home: string | null;
  spec: Record<string, unknown>;
  enabled: boolean;
  revision: number;
  units: number;
  uses: string[];
  manages: string[];
}

interface RuntimeRecord {
  id: string;
  host: string;
  kind: 'environment' | 'provider';
  unitName: string;
  state: string;
  environment: string | null;
  provider: string | null;
  removeAt: string | null;
}

/** The schema a docker-like driver gives the spec of a template, cut down to the members the screens talk about. */
function driverSchema(title: string, extra: Record<string, unknown>): Record<string, unknown> {
  return templateSchema(title.includes('provider') ? 'provider' : 'environment', {
    $schema: 'https://json-schema.org/draft/2020-12/schema',
    title,
    type: 'object',
    additionalProperties: false,
    properties: {
      image: { type: 'string', minLength: 1, description: 'A registry reference. Exactly one of image, build and archive is given' },
      build: { type: 'object', required: ['dockerfile'], description: 'A Dockerfile, built on the host', properties: { dockerfile: { type: 'string' } } },
      archive: { type: 'object', required: ['blob'], description: 'An image archive, loaded on the host', properties: { blob: { type: 'string' } } },
      runtime: { type: 'string', description: 'A container runtime the host reports: runc, runsc, kata' },
      network: { type: 'object', properties: { mode: { enum: ['bridge', 'none'], description: 'none leaves only a link to the gateway' } } },
      security: { type: 'object', properties: { read_only_rootfs: { type: 'boolean' }, no_new_privileges: { type: 'boolean' } } },
      resources: { type: 'object', description: 'The most the unit may use', properties: { cpu_millis: { type: 'integer' }, memory_mb: { type: 'integer' }, pids: { type: 'integer' }, disk_gb: { type: 'integer' } } },
      ...extra,
    },
  });
}

/** Descriptor of a harness as its adapter declares it, cut down to what the screens read. */
function descriptor(kind: string, name: string, version: string): Record<string, unknown> {
  return {
    kind,
    harness: { name, status: 'supported', version, supported_range: '>=1.0.0, <2.0.0' },
    parameters: [
      {
        key: 'reasoning_effort',
        type: 'enum',
        apply: 'next_turn',
        group: 'model',
        title: 'Reasoning effort',
        scopes: ['org', 'project', 'topic', 'thread'],
        default: { from_model: 'default_effort' },
        options: { from_model: 'efforts' },
        description: 'How hard the model thinks before it answers.',
      },
      {
        key: 'verbosity',
        type: 'enum',
        apply: 'next_turn',
        group: 'behaviour',
        title: 'Verbosity',
        scopes: ['thread', 'topic'],
        default: 'normal',
        options: [
          { label: 'Terse', value: 'terse' },
          { label: 'Normal', value: 'normal' },
          { label: 'Chatty', value: 'chatty' },
        ],
        description: 'How much the harness says about what it does.',
      },
    ],
    credentials: { issues: [{ form: 'api_key', custody: 'shared', lifetime_secs: null }], accepts: ['api_key'] },
    arrangements: [{ type: 'separated', executor: 'orch-exec', tool_sets: ['claude-code', 'plain', 'custom'], executor_version: '*' }, { type: 'key_separated' }, { type: 'credential_shared' }],
    capabilities: { plan: true, steer: true, title: true, effort: 'turn', memory: 'directory', resume: true, approvals: ['command', 'file_change', 'permissions'], interrupt: true, subagents: true, orch_tools: 'mcp', input_images: true, usage_tokens: true, model_discovery: true, discover_sessions: true },
    sandbox_modes: ['read_only', 'workspace', 'open'],
    approval_modes: ['ask', 'on_request', 'auto'],
    adapter_version: 1,
    native_provider: {
      plans: { pro: 'Pro', free: 'Free' },
      title: `${name} subscription`,
      arrangements: ['separated', 'key_separated'],
      harness_kind: kind,
      login_methods: [
        { id: 'device_code', kind: 'interactive', title: 'Sign in with a code', fields: [], yields: 'login', summary: 'Shows an address and a code to enter there.', requires: null, recommended: true, lifetime_secs: null },
      ],
    },
  };
}

const RATE = { input_per_mtok: '2.50', cached_input_per_mtok: '0.25', output_per_mtok: '15' };
/** A rate with a multiple for the speed Fast and a step above which a prompt pays more. */
const STEPPED_RATE = { ...RATE, tiers: { fast: '2' }, above: [{ context_tokens: 272_000, input_per_mtok: '5', cached_input_per_mtok: '0.5', output_per_mtok: '22.5' }] };

/** What a thread runs on: the provider, the agent profile and the environment it is bound to, the model with its effort, and the directory of the environment it works in. */
export interface ThreadBinding {
  provider: string;
  agent: string;
  environment: string;
  model: string;
  effort: string | null;
  directory: string;
  /** The way the harness, its tools and the credential were placed when the thread was made; threads the world began with say none. */
  arrangement?: ArrangementType;
}

/**
 * What the settings of a place say about how a thread made there is bound: what to take where nothing is chosen, which arrangements to
 * put first and which are all that may be used, how a thread gets its environment, and the topic's own default.
 */
export interface BindingGround {
  node: { id: string; kind: 'topic' | 'project' };
  defaults: { provider: string | null; agent: string | null; model: string | null; effort: string | null; locked: boolean };
  prefer: ArrangementType[];
  allowed: ArrangementType[] | null;
  environmentDefault: 'shared' | 'fresh';
  topicDefault: string | null;
}

/** What a thread runs on when nobody chose otherwise: the way the team usually works, on Ada's laptop with a key. */
const USUAL_BINDING: ThreadBinding = {
  provider: RUNS_ON.openAiKey,
  agent: RUNS_ON.codex,
  environment: RUNS_ON.workspace,
  model: USUAL_MODEL.name,
  effort: USUAL_MODEL.effort,
  directory: '',
};

export class Resources {
  readonly world: World;
  machines: MachineRecord[] = [];
  providers: ProviderRecord[] = [];
  agents: AgentRecord[] = [];
  environments: EnvironmentRecord[] = [];
  hosts: HostRecord[] = [];
  templates: TemplateRecord[] = [];
  runtimes: RuntimeRecord[] = [];
  /** Environment starts and one-server restarts retain their native job receipts. */
  readonly jobs = new Map<string, Job>();
  readonly mcpServers = new Map<string, EnvironmentMcp>();
  /** Explicitly controlled update/reconnect phases, without a timer that can race a browser assertion. */
  private componentUpdates = new Map<string, ComponentUpdate>();
  private driverFacts = new Map<string, Pick<Host, 'drivers' | 'storages'> & Partial<Pick<Host, 'capacity'>>>();
  private templateDriversReported = true;
  /** The jobs that end later: an environment that is starting is running a moment after. */
  private timers = new Set<NodeJS.Timeout>();
  /** Tokens made and not yet used, so that a machine that enrolls with one is the one its dialog waits for. */
  tokens = new Set<string>();
  private enrollmentSelections = new Map<string, EnrollmentSelection>();
  private enrollmentReceipts = new Map<string, { owner: string; machine: string | null; host: string | null }>();
  private receiptByToken = new Map<string, string>();
  private lastConnectorToken: string | null = null;
  private lastHostToken: string | null = null;
  /** The tokens that enroll a machine again, by the machine each is for. */
  private againTokens = new Map<string, string>();
  /** When the world began: what is said to end "in so many days" is counted from here, so that two reads of one machine say one thing. */
  private born = Date.now();
  /** The names a discovery offers for each provider that can be asked. */
  private count = 100;
  /**
   * What each thread runs on, by thread. This is the one place that says it: the thread, the environment
   * it works in and the provider it is bound to are all read from here, so that they tell the same story.
   */
  readonly bindings = new Map<string, ThreadBinding>();
  /** The job that is under way on an environment, which the part that changes things tells; the page of the environment shows it. */
  jobRefOf: ((environmentId: string) => JobRef | null) | null = null;
  /** Whether a thread is one the caller may not see, which the part that changes things tells; the page of an environment counts such threads and does not name them. */
  unseenThread: ((threadId: string) => boolean) | null = null;
  /** People who may make no resource, whatever their role says: what the screens ask the server a person may do. */
  readonly noMaking = new Set<string>();
  /** Environments that offer no executor to a harness that runs apart from its tools. */
  readonly bare = new Set<string>();

  constructor(world: World) {
    this.world = world;
    this.reset();
  }

  reset(): void {
    for (const timer of this.timers) clearTimeout(timer);
    this.timers.clear();
    this.jobs.clear();
    this.mcpServers.clear();
    this.componentUpdates.clear();
    this.driverFacts.clear();
    this.templateDriversReported = true;
    this.tokens.clear();
    this.enrollmentReceipts.clear();
    this.receiptByToken.clear();
    this.enrollmentSelections.clear();
    this.lastConnectorToken = null;
    this.lastHostToken = null;
    this.againTokens.clear();
    this.noMaking.clear();
    this.bare.clear();
    this.born = Date.now();
    this.count = 100;
    const owner = 'ada';
    this.machines = [
      {
        id: fixtureId(KIND.machine, 1),
        name: 'ada-laptop',
        owner,
        os: 'linux',
        arch: 'x86_64',
        online: true,
        status: 'active',
        connector: '0.1.0',
        lastSeen: inMs(-2 * 60_000),
        roundTripMs: 42,
        harnesses: [
          { kind: 'codex', path: '/usr/local/bin/codex', version: '0.162.1', daemon: false },
          { kind: 'claude-code', path: '/usr/local/bin/claude', version: '2.1.296', daemon: false },
        ],
        policy: shippedPolicy(),
      },
      {
        id: fixtureId(KIND.machine, 2),
        name: 'build-box',
        owner,
        os: 'linux',
        arch: 'aarch64',
        online: false,
        status: 'active',
        connector: '0.1.0',
        lastSeen: inMs(-3 * DAY),
        roundTripMs: 310,
        harnesses: [],
        policy: shippedPolicy({ 'environment.roots': ['/srv/build'] }),
        // It has been away so long that its certificate ended before it could renew it.
        certEnds: -2 * DAY,
      },
      {
        id: fixtureId(KIND.machine, 3),
        name: 'old-mac',
        owner,
        os: 'macos',
        arch: 'aarch64',
        online: false,
        status: 'revoked',
        connector: '0.0.9',
        lastSeen: inMs(-40 * DAY),
        roundTripMs: 80,
        harnesses: [],
        policy: shippedPolicy(),
      },
      {
        id: fixtureId(KIND.machine, 4),
        name: 'lab-box',
        owner,
        os: 'windows',
        arch: 'x86_64',
        online: true,
        status: 'active',
        connector: '0.1.0',
        lastSeen: inMs(-30_000),
        roundTripMs: 120,
        harnesses: [{ kind: 'codex', path: 'C:\\tools\\codex.exe', version: '0.162.1', daemon: false }],
        policy: shippedPolicy({ 'environment.roots': ['C:\\work'], 'policy.delegated': ['environment.terminal', 'environment.roots', 'provider.upstreams'] }),
      },
    ];
    this.agents = [
      { id: fixtureId(KIND.agent, 1), name: 'Codex', harness: 'codex', owner: null, home: null, machine: null, disabled: false, revision: 1, profile: { env: {}, config: {}, harness_version: null }, descriptor: descriptor('codex', 'Codex', '0.162.1') },
      { id: fixtureId(KIND.agent, 2), name: 'Claude Code', harness: 'claude-code', owner: null, home: null, machine: null, disabled: false, revision: 1, profile: { env: {}, config: {}, harness_version: null }, descriptor: {} },
      { id: fixtureId(KIND.agent, 3), name: 'Scripted agent', harness: 'fake', owner: null, home: null, machine: null, disabled: false, revision: 1, profile: { env: {}, config: {}, harness_version: null }, descriptor: descriptor('fake', 'Scripted agent', '1.0.0') },
      { id: fixtureId(KIND.agent, 4), name: 'Codex on ada-laptop', harness: 'codex', owner: 'ada', home: ACME.org, machine: fixtureId(KIND.machine, 1), disabled: false, revision: 3, profile: { env: { RUST_LOG: 'info' }, config: { model_reasoning_effort: 'high' }, harness_version: '0.162.1' }, descriptor: descriptor('codex', 'Codex', '0.162.1') },
    ];
    this.providers = [
      {
        id: fixtureId(KIND.provider, 1),
        name: 'OpenAI key',
        kind: 'api',
        harness: null,
        owner,
        home: ACME.org,
        machine: fixtureId(KIND.machine, 1),
        keyMachine: fixtureId(KIND.machine, 1),
        keyHeldByOrch: true,
        placement: 'connected',
        status: 'ready',
        statusDetail: null,
        arrangements: ['separated', 'key_separated'],
        upstreams: [{ protocol: 'openai-responses', base_url: 'https://api.openai.com/v1', options: {} }],
        models: [
          { ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 1), model_name: 'gpt-5.4', display_name: 'GPT-5.4', enabled: true, source: 'manual', groups: ['frontier'], parameters: { efforts: ['low', 'medium', 'high'], default_effort: 'medium', context_window: 400_000, input_images: true, rate: RATE, offered: { service_tiers: [{ id: 'fast', label: 'Fast' }] } } },
          { ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 2), model_name: 'gpt-5.4-mini', display_name: '', enabled: true, source: 'suggested', groups: ['small'], parameters: { efforts: ['low', 'medium'], context_window: 200_000 } },
          { ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 6), model_name: 'gpt-5.4-chat', display_name: 'GPT-5.4 Chat', enabled: true, source: 'manual', groups: [], parameters: { context_window: 128_000 } },
          { ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 7), model_name: 'gpt-4.1', display_name: 'GPT-4.1', enabled: false, source: 'manual', groups: [], parameters: {} },
        ],
        keySet: true,
        secrets: [],
        facts: emptyProviderFacts(),
        running: 2,
        uses: ['dora'],
        manages: [],
        agent: null,
        revision: 4,
        createdAt: MADE_AT,
        heldBy: null,
      },
      {
        id: fixtureId(KIND.provider, 2),
        name: 'Codex subscription',
        kind: 'subscription',
        harness: 'codex',
        owner,
        home: ACME.org,
        machine: fixtureId(KIND.machine, 1),
        keyMachine: fixtureId(KIND.machine, 1),
        placement: 'connected',
        status: 'ready',
        statusDetail: null,
        arrangements: ['separated', 'key_separated'],
        upstreams: [],
        models: [{ ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 3), model_name: 'gpt-5.5-codex', display_name: 'GPT-5.5 Codex', enabled: true, source: 'suggested', groups: ['frontier'], parameters: { efforts: ['low', 'medium', 'high'], default_effort: 'medium', context_window: 272_000, rate: STEPPED_RATE, offered: { service_tiers: [{ id: 'fast', label: 'Fast' }], access_programs: ['Daybreak Blue', 'Daybreak Red'], context_max: 1_000_000 } } }],
        keySet: false,
        secrets: [],
        facts: {
          ...emptyProviderFacts(),
          plan: 'Pro',
          identity: 'ada@example.org',
          windows: [
            { id: '5h', label: '5-hour window', used_percent: 42, duration_minutes: 300, resets_at: inMs(2 * HOUR), model_family: null, is_active: null, reset_not_started: null, warning_percent: null, severity: null },
            { id: '7d', label: 'Weekly window', used_percent: 81, duration_minutes: 10_080, resets_at: inMs(3 * DAY), model_family: null, is_active: null, reset_not_started: null, warning_percent: null, severity: null },
          ],
          balances: [{id:'plan',label:'Plan credits',unit:'credits',remaining:'120',total:null,used:null,unlimited:false,parts:null,expires_at:null}],
          limit_reached: null,
          credential: { method: 'import', yields: 'login', issued_at: null, expires_at: null },
        },
        running: 0,
        uses: [],
        manages: [],
        agent: fixtureId(KIND.agent, 4),
        revision: 2,
        createdAt: MADE_AT,
        heldBy: null,
      },
      {
        id: fixtureId(KIND.provider, 3),
        name: 'Team Anthropic',
        kind: 'api',
        harness: null,
        owner: 'grace',
        home: ACME.org,
        machine: fixtureId(KIND.machine, 2),
        keyMachine: fixtureId(KIND.machine, 2),
        keyHeldByOrch: true,
        placement: 'connected',
        status: 'ready',
        statusDetail: null,
        arrangements: ['separated', 'key_separated'],
        upstreams: [{ protocol: 'anthropic-messages', base_url: 'https://api.anthropic.com', options: {} }],
        models: [{ ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 4), model_name: 'claude-sonnet-5', display_name: 'Claude Sonnet 5', enabled: true, source: 'manual', groups: [], parameters: {} }],
        keySet: true,
        secrets: [],
        facts: emptyProviderFacts(),
        running: 0,
        uses: ['ada', 'dora'],
        manages: [],
        agent: null,
        revision: 1,
        createdAt: MADE_AT,
        heldBy: null,
      },
      {
        id: fixtureId(KIND.provider, 4),
        name: 'Fresh key',
        kind: 'api',
        harness: null,
        owner,
        home: ACME.org,
        machine: null,
        keyMachine: null,
        placement: 'managed',
        status: 'needs_setup',
        statusDetail: null,
        arrangements: ['separated', 'key_separated'],
        upstreams: [{ protocol: 'openai-responses', base_url: 'https://api.deepseek.com', options: {} }],
        models: [],
        keySet: true,
        secrets: [],
        facts: emptyProviderFacts(),
        running: 0,
        uses: [],
        manages: [],
        agent: null,
        revision: 1,
        createdAt: MADE_AT,
        heldBy: null,
      },
      {
        id: fixtureId(KIND.provider, 5),
        name: 'Claude subscription',
        kind: 'subscription',
        harness: 'claude-code',
        owner,
        home: ACME.org,
        machine: fixtureId(KIND.machine, 4),
        keyMachine: fixtureId(KIND.machine, 4),
        placement: 'connected',
        status: 'exhausted',
        statusDetail: 'The 5-hour window is used up.',
        arrangements: ['separated', 'key_separated'],
        upstreams: [],
        models: [{ ...MODEL_ESTIMATE, id: fixtureId(KIND.model, 5), model_name: 'claude-opus-5', display_name: 'Claude Opus 5', enabled: true, source: 'manual', groups: [], parameters: {} }],
        keySet: false,
        secrets: [],
        facts: {...emptyProviderFacts(),plan:'Max',identity:'ada@example.org',windows:[{id:'5h',label:'5-hour window',used_percent:100,duration_minutes:300,resets_at:inMs(HOUR),model_family:null,is_active:null,reset_not_started:null,warning_percent:null,severity:null}],limit_reached:{type:'5-hour window',resets_at:inMs(HOUR)},credential:{method:'import',yields:'login',issued_at:null,expires_at:inMs(9 * DAY)}},
        running: 0,
        uses: [],
        manages: [],
        agent: fixtureId(KIND.agent, 2),
        revision: 1,
        createdAt: MADE_AT,
        heldBy: null,
      },
    ];
    this.environments = [
      {
        id: fixtureId(KIND.environment, 1),
        name: 'Workspace',
        owner,
        home: ACME.org,
        machine: fixtureId(KIND.machine, 1),
        host: null,
        template: null,
        status: 'running',
        root: '/home/ada/work/main',
        remoteWrite: true,
        terminal: true,
        revision: 2,
        uses: ['dora'],
        manages: [],
        updates: 0,
        removeAt: null,
      },
      {
        id: fixtureId(KIND.environment, 2),
        name: 'Docs checkout',
        owner,
        home: ACME.org,
        machine: fixtureId(KIND.machine, 2),
        host: null,
        template: null,
        status: 'offline',
        root: '/srv/build/docs',
        remoteWrite: false,
        terminal: false,
        revision: 1,
        uses: [],
        manages: [],
        updates: 0,
        removeAt: null,
      },
      {
        id: fixtureId(KIND.environment, 3),
        name: 'Sandbox',
        owner: 'grace',
        home: ACME.org,
        machine: null,
        host: fixtureId(KIND.host, 1),
        template: fixtureId(KIND.template, 1),
        status: 'stopped',
        root: '/workspace',
        remoteWrite: true,
        terminal: true,
        revision: 1,
        uses: ['ada'],
        manages: ['ada'],
        updates: 2,
        removeAt: null,
      },
    ];
    this.hosts = [
      { id: fixtureId(KIND.host, 1), name: 'host-1', owner: null, home: null, labels: ['docker', 'x86_64'], schedulable: true, whenFull: null, revision: 2, uses: ['ada', 'dora', 'grace'], manages: ['ada'] },
      { id: fixtureId(KIND.host, 2), name: 'build-host', owner, home: ACME.org, labels: ['arm64'], schedulable: false, whenFull: 'block', revision: 1, uses: [], manages: [] },
    ];
    this.runtimes = [{ id: fixtureId(KIND.runtime, 1), host: fixtureId(KIND.host, 1), kind: 'environment', unitName: 'orch-env-sandbox', state: 'stopped', environment: fixtureId(KIND.environment, 3), provider: null, removeAt: null }];
    // What the threads of the team's tree run on: most of them the usual way, one on a subscription in a stopped
    // environment of its own, and the one that is held back by a limit on the provider whose limit it is.
    this.bindings.clear();
    this.bindings.set(ACME.mainThread, { ...USUAL_BINDING, directory: 'login' });
    this.bindings.set(ACME.repro, { ...USUAL_BINDING, effort: 'medium', directory: 'repro' });
    this.bindings.set(ACME.baselineThread, { ...USUAL_BINDING, directory: 'baseline' });
    this.bindings.set(ACME.depsThread, { provider: RUNS_ON.codexSubscription, agent: RUNS_ON.codexOnLaptop, environment: RUNS_ON.sandbox, model: 'gpt-5.5-codex', effort: 'medium', directory: 'deps' });
    this.bindings.set(ACME.idleThread, { provider: RUNS_ON.claudeSubscription, agent: RUNS_ON.claudeCode, environment: RUNS_ON.workspace, model: 'claude-opus-5', effort: null, directory: 'idle' });
    this.templates = [
      {
        id: fixtureId(KIND.template, 1),
        name: 'Default',
        description: 'Where environments are made unless a place says otherwise.',
        kind: 'environment',
        driver: 'docker',
        owner: null,
        home: null,
        spec: { image: 'orch-env-base:0.1.0', user: { uid: 1000, gid: 1000, name: 'agent' }, needs: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 10 }, exec: { offers: ['codex-exec-server', 'claude-code', 'plain'], orch_tools: true } },
        enabled: true,
        revision: 1,
        units: 1,
        uses: ['ada', 'dora', 'grace', 'vera'],
        manages: [],
      },
      {
        id: fixtureId(KIND.template, 2),
        name: 'Provider runtime',
        description: "Where a managed provider's credential and harness are kept.",
        kind: 'provider',
        driver: 'docker',
        owner: null,
        home: null,
        spec: { image: 'gcr.io/distroless/static-debian12:nonroot', user: { uid: 65532, gid: 65532, name: 'nonroot' } },
        enabled: true,
        revision: 1,
        units: 0,
        uses: ['ada', 'dora', 'grace', 'vera'],
        manages: [],
      },
      {
        id: fixtureId(KIND.template, 3),
        name: 'Node 22 dev',
        description: 'Node 22 with pnpm, for the web projects.',
        kind: 'environment',
        driver: 'docker',
        owner,
        home: ACME.org,
        spec: { image: 'node:22-bookworm', user: { uid: 1000, gid: 1000, name: 'node' }, needs: { cpu_millis: 2000, memory_mb: 4096, disk_gb: 20 } },
        enabled: true,
        revision: 3,
        units: 0,
        uses: ['dora'],
        manages: [],
      },
    ];
  }

  // ---------------------------------------------------------------- what threads run on

  /** What a thread runs on: what it was bound to, or the usual when nobody bound it. */
  bindingOf(threadId: string): ThreadBinding {
    return this.bindings.get(threadId) ?? USUAL_BINDING;
  }

  /** Binds a thread to what it will run on; it is then among the threads of its environment and its provider. */
  bind(threadId: string, binding: ThreadBinding): void {
    this.bindings.set(threadId, binding);
    const environment = this.environments.find((row) => row.id === binding.environment);
    if (environment?.config?.['env.lifetime'] === 'thread' && environment.temporary?.thread === null) environment.temporary.thread = threadId;
  }

  /** The parts of a binding as the API names them; one that is no longer there is `null`. */
  refsOf(binding: ThreadBinding): { provider: ProviderRef | null; agent: AgentRef | null; environment: EnvironmentRef | null } {
    const provider = this.providers.find((record) => record.id === binding.provider);
    const agent = this.agents.find((record) => record.id === binding.agent);
    const environment = this.environments.find((record) => record.id === binding.environment);
    return {
      provider: provider === undefined ? null : this.providerRef(provider),
      agent: agent === undefined ? null : this.agentRef(agent),
      environment: environment === undefined ? null : this.environmentRef(environment),
    };
  }

  /** The models a provider lists. */
  modelsOf(providerId: string): ProviderModel[] {
    return this.providers.find((record) => record.id === providerId)?.models ?? [];
  }

  /** The parameters the agent declares, as its adapter reports them (a thread's page lists those that reach a thread). */
  declaredBy(agentId: string): readonly Record<string, unknown>[] {
    const declared = (this.agents.find((record) => record.id === agentId)?.descriptor as { parameters?: unknown } | undefined)?.parameters;
    return Array.isArray(declared) ? (declared as Record<string, unknown>[]) : [];
  }

  /** What a resource is called and who owns it, for the pages that speak of one (its permission page); `null` where there is none such. */
  describe(kind: 'provider' | 'agent' | 'environment' | 'host' | 'template', id: string): { name: string; owner: string | null } | null {
    const list: readonly { id: string; name: string; owner: string | null }[] =
      kind === 'provider' ? this.providers : kind === 'agent' ? this.agents : kind === 'environment' ? this.environments : kind === 'host' ? this.hosts : this.templates;
    const found = list.find((record) => record.id === id);
    return found === undefined ? null : { name: found.name, owner: found.owner };
  }

  /** Whether the person may run work on the provider, and it is ready for that: what a thread started on it needs. */
  readyFor(person: Person, providerId: string): boolean {
    const record = this.providers.find((candidate) => candidate.id === providerId);
    return record?.status === 'ready' && this.held(person, 'provider', record.owner, record.uses, record.manages).permissions.includes('provider.use');
  }

  /** Whether the environment is there to be worked in. */
  hasEnvironment(environmentId: string): boolean {
    return this.environments.some((record) => record.id === environmentId);
  }

  /** What a screen that looks into an environment (its files, its shells) needs of its record; `null` where there is none. */
  workspaceFacts(environmentId: string): { status: Environment['status']; remoteWrite: boolean; terminal: boolean; index: 'full' | 'live' } | null {
    const record = this.environments.find((candidate) => candidate.id === environmentId);
    return record === undefined || record.status === 'removed' ? null : { status: record.status, remoteWrite: record.remoteWrite, terminal: record.terminal, index: record.index ?? 'full' };
  }

  /** A test's steering of the part of an environment's record that a workspace shows: whether it is there, whether Orch may write into it, whether a shell may be opened, how its files are indexed. */
  steerWorkspace(environmentId: string, patch: { status?: Environment['status']; remoteWrite?: boolean; terminal?: boolean; index?: 'full' | 'live'; machineOnline?: boolean }): void {
    const record = this.environments.find((candidate) => candidate.id === environmentId);
    if (record === undefined) return;
    if (patch.status !== undefined) record.status = patch.status;
    if (patch.remoteWrite !== undefined) record.remoteWrite = patch.remoteWrite;
    if (patch.terminal !== undefined) record.terminal = patch.terminal;
    if (patch.index !== undefined) record.index = patch.index;
    if (patch.machineOnline !== undefined) {
      const machine = this.machines.find((candidate) => candidate.id === record.machine);
      if (machine !== undefined) machine.online = patch.machineOnline;
    }
    record.revision += 1;
  }

  /** Whether the profile is there, and its harness has said what it is: until then no thread can be placed on it. */
  agentReported(agentId: string): boolean {
    return Object.keys(this.agents.find((candidate) => candidate.id === agentId)?.descriptor ?? {}).length > 0;
  }

  /** The environments that the threads of a topic run in. */
  environmentsOfTopic(topicId: string): EnvironmentRef[] {
    const used = new Set([...this.bindings].filter(([thread]) => this.alive(thread)?.parent_id === topicId).map(([, binding]) => binding.environment));
    return this.environments.filter((record) => used.has(record.id)).map((record) => this.environmentRef(record));
  }

  /** The row of a thread of the tree, unless it was deleted. */
  private alive(threadId: string) {
    const row = this.world.places.rowOf(threadId);
    return row === undefined || row.deleted ? undefined : row;
  }

  /** The threads that run in an environment, with the topic of each and the directory it works in. */
  private threadsIn(environmentId: string): { thread: string; topic: string; directory: string }[] {
    return [...this.bindings]
      .filter(([thread, binding]) => binding.environment === environmentId && this.alive(thread) !== undefined)
      .map(([thread, binding]) => ({ thread, topic: this.alive(thread)?.parent_id ?? '', directory: binding.directory }));
  }

  /** How many threads work in an environment, and in how many topics. */
  reachOf(environmentId: string): { threads: number; topics: number } {
    const inside = this.threadsIn(environmentId);
    return { threads: inside.length, topics: new Set(inside.map((entry) => entry.topic)).size };
  }

  /** The threads that are bound to a provider. */
  private threadsOn(providerId: string): string[] {
    return [...this.bindings].filter(([thread, binding]) => binding.provider === providerId && this.alive(thread) !== undefined).map(([thread]) => thread);
  }

  /** A thread, a topic or an org as the API refers to it, by the name the places hold. */
  private placeRef(id: string, kind: 'thread' | 'topic' | 'org') {
    return { id, kind, name: this.world.places.rowOf(id)?.name ?? kind, redacted: false };
  }

  // ---------------------------------------------------------------- who is who

  private userOf(username: string | null): UserRef {
    const person = username === null ? undefined : this.world.people.get(username);
    if (person === undefined) return { id: fixtureId(1, 0), kind: 'human', username: username ?? 'system', display_name: username ?? 'System', avatar: { text: 'S', color: 0 } };
    return userRef(person.me);
  }

  private nodeRef(id: string | null): { id: string; kind: 'org'; name: string; redacted: boolean } {
    const name = id === ACME.org ? 'Acme' : id === ACME.research ? 'Research' : 'Acme';
    return { id: id ?? ACME.org, kind: 'org', name, redacted: false };
  }

  /** Whether the person administers the org the resources live in: the system's administrators and Acme's admin. */
  private administers(person: Person): boolean {
    return person.me.is_system_admin;
  }

  private mayMake(person: Person): boolean {
    return person.me.username !== 'vera' && !this.noMaking.has(person.me.username);
  }

  /** Refuses as the real API does when a person may not make a resource in an org: what is missing, of which org, and whom to ask. */
  private needSettings(person: Person, orgId: string): void {
    if (this.mayMake(person)) return;
    const owner = this.userOf('ada');
    throw new Refusal(403, 'authz.denied', 'settings.manage is required on this org', {
      required: 'settings.manage',
      object: { kind: 'org', id: orgId },
      requestable: true,
      override: false,
      missing: [{ object: { kind: 'org', id: orgId, name: this.nodeRef(orgId).name, owner }, permissions: ['settings.manage'], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
    });
  }

  private refuseOn(kind: 'provider' | 'environment' | 'agent' | 'template' | 'host', id: string, name: string, ownerName: string | null, permission: string): never {
    const owner = this.userOf(ownerName);
    throw new Refusal(403, 'authz.denied', `${permission} is required`, {
      required: permission,
      object: { kind, id },
      requestable: true,
      override: false,
      missing: [{ object: { kind, id, name, owner }, permissions: [permission], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
    });
  }

  // ---------------------------------------------------------------- shapes

  private machineRef(record: MachineRecord): MachineRef {
    return { id: record.id, name: record.name, online: record.online && record.status === 'active', platform_os: record.os, placement: 'connected' };
  }

  /** Safe physical publisher metadata already visible through an environment's Ports page. */
  publisherOf(person:Person,id:string):{machine:MachineRef;host:Pick<Host,'id'|'name'|'address'>|null;revision:number;policy:unknown}|null {
    const environment=this.environments.find((entry)=>entry.id===id);
    const node=this.hosts.find((entry)=>entry.id===environment?.host);
    if(node!==undefined) {
      const host=this.host(person,node);
      return {machine:host.machine,host:{id:host.id,name:host.name,address:host.address},revision:node.revision,policy:undefined};
    }
    const machine=this.machines.find((entry)=>entry.id===environment?.machine);
    return machine===undefined?null:{machine:this.machineRef(machine),host:null,revision:environment?.revision??1,policy:machine.policy.ports};
  }

  private machineFacts(record: MachineRecord): unknown {
    return {
      role_management:{supported:!record.roles?.includes('node'),delegated:this.machineKeyHanded(record,'machine.roles'),enabled:record.policy['machine.roles']===true},
      programs:this.machinePrograms(null,record),
      dedicated: record.dedicated === true,
      environment_descriptor: { name: 'directory', version: '1', title: 'Registered directories', lifecycle: 'attach', confinement: record.dedicated === true ? 'isolated' : 'none', ...(record.dedicated === true ? { confinement_by: 'machine' } : {}), mounts: 'in_place', data_storage: directoryStorage(record) },
      hostname: record.name,
      os_version: record.os === 'linux' ? 'Linux (Debian GNU/Linux 13)' : record.os === 'macos' ? 'macOS 15.1' : 'Windows 11 23H2',
      cpus: 10,
      memory_bytes: 17_179_869_184,
      storages: [{name:'default',default:true,capacity_gb:500,free_gb:320,private_views:false}],
      harnesses: record.harnesses,
      adapters: [
        { kind: 'claude-code', adapter_version: 1, builtin: true },
        { kind: 'codex', adapter_version: 1, builtin: true },
        { kind: 'fake', adapter_version: 1, builtin: true },
        ...(record.programs ?? []).filter((entry)=>entry.kind==='agent' && entry.ready).map((entry)=>({kind:entry.name,adapter_version:1,builtin:false,needs_binary:false})),
      ],
      plugins: (record.programs ?? []).map((entry)=>({kind:entry.kind==='agent'?'adapter':'provider',name:entry.name,version:entry.version ?? '',state:entry.ready?'running':'failed',descriptor:entry.kind==='provider_source'?entry.descriptor:null})),
      policy: record.policy,
      rates: { up_bps: 1_500_000, down_bps: 2_800_000, round_trip_ms: record.roundTripMs },
      clock_offset_ms: 0,
    };
  }

  private agentRef(record: AgentRecord): AgentRef {
    const machine = this.machines.find((candidate) => candidate.id === record.machine);
    return {
      id: record.id,
      name: record.name,
      address: record.home === null ? `system.${record.name}` : `acme.${record.owner ?? 'ada'}.${record.name}`,
      harness_kind: record.harness, harness: harnessName(record.harness),
      machine: machine === undefined ? null : this.machineRef(machine),
      disabled: record.disabled,
    };
  }

  private providerRef(record: ProviderRecord): ProviderRef {
    return { id: record.id, name: record.name, kind: record.kind, harness_kind: record.harness, harness: maybeHarnessName(record.harness), address: `acme.${record.owner}.${record.name}`, placement: record.machine===null?'managed':'connected', status: record.status };
  }

  /** Page, list and reference addresses share the server's short-segment rule and actual home. */
  private environmentAddress(record: EnvironmentRecord): string {
    const short = record.name.toLowerCase().replace(/[^\p{L}\p{N}_-]+/gu, '-').replace(/^-+|-+$/g, '');
    const segment = short.length === 0 ? `environment-${record.id}` : short;
    return [this.world.places.addressOf(record.home), record.owner, segment].filter((part) => part.length > 0).join('.');
  }

  /** An environment as other things name it; the adoption part names the one a session would stand in. */
  environmentRef(record: EnvironmentRecord) {
    return ({ id: record.id, name: record.name, owner: this.userOf(record.owner), address: this.environmentAddress(record), placement: record.machine === null ? ('managed' as const) : ('connected' as const), status: record.status, ports_exposed: this.world.ports.exposed(record.id) });
  }

  /** Active machines and the same certificate dates their own pages return, for Management's PKI list. */
  machineCertificates() {
    return this.machines.filter((record) => record.status === 'active').map((record) => ({
      machine: this.machineRef(record),
      not_after: this.machine(record).cert_not_after,
    }));
  }

  private installation(record:MachineRecord):MachineInstallation {
    const data=`/home/${record.owner}/.local/share/orch-connector`;
    const executable=`/home/${record.owner}/.local/bin/orch-connector`;
    const prefix=`${executable} --data-dir ${data}`;
    return {program:'orch-connector',executable,data_dir:data,config_file:`${data}/config.toml`,user:record.owner,system:false,service:'orch-connector.service',
      commands:{policy:`${prefix} policy`,roles:`${prefix} roles`,start:`${executable} service start`,stop:`${executable} service stop`,update:`${prefix} update`,uninstall:`${prefix} uninstall`}};
  }

  private machineKeyHanded(record:MachineRecord,key:string):boolean {
    const delegated=record.policy['policy.delegated'];const local=record.policy['policy.local'];
    return key!=='policy.delegated' && !(isRecord(local) && Object.hasOwn(local,key)) && Array.isArray(delegated)
      && (delegated.includes('*') || delegated.includes(key) || policySections(key).some((section)=>section!=='machine' && delegated.includes(`${section}.*`)));
  }

  private machinePrograms(person:Person|null,record:MachineRecord):MachinePrograms {
    const roles=record.roles ?? ['environment','agent','provider'];
    return {entries:record.programs ?? [],config_file:this.installation(record).config_file,can_manage:record.owner===person?.me.username,
      capabilities:(['agent','provider_source'] as const).map((kind)=>{
        const key=kind==='agent'?'agent.programs':'provider.programs';
        const supported=kind==='agent'?roles.includes('agent'):roles.includes('agent') || roles.includes('provider');
        const delegated=this.machineKeyHanded(record,key);const enabled=record.policy[key]===true;
        const remote=supported && delegated && enabled;
        return {kind,supported,remote,delegated,enabled,policy_key:key,reason:remote?null:`${key} is kept by this machine's operator.`};
      })};
  }

  private sourceOffers():ProviderSetup['sources'] {
    return [...builtinSourceOffers(),...this.machines.flatMap((machine)=>(machine.programs ?? [])
      .filter((entry)=>entry.kind==='provider_source' && entry.ready).map((entry)=>({by:'program' as const,name:entry.name,
        descriptor:sourceDescriptor(entry.name,entry.name,null,true),default_for:[],machine:this.machineRef(machine),template:null,command:entry.command})))];
  }

  private changeMachinePrograms(person:Person,record:MachineRecord,body:Record<string,unknown>):MachinePrograms {
    if(!record.online) throw new Refusal(503,'source.offline','The machine is offline.');
    const entries=record.programs ?? [];
    const kind:MachineProgramKind=body.kind==='provider_source'?'provider_source':'agent';
    const name=asText(body.name);
    const entry=entries.find((entry)=>entry.kind===kind && entry.name===name);
    if(body.action==='reload') return this.machinePrograms(person,record);
    if(body.action==='test') {
      if(entry===undefined) throw new Refusal(404,'not_found','This program is not registered.');
      entry.ready=true;
      if(kind==='provider_source') entry.models={models:[{model_name:'local-model',display_name:'Local model'}]};
      return this.machinePrograms(person,record);
    }
    const capability=this.machinePrograms(person,record).capabilities.find((entry)=>entry.kind===kind);
    if(capability?.remote!==true) throw new Refusal(409,'policy.not_delegated',capability?.reason ?? 'This machine does not offer that role.');
    if(body.action==='remove') {
      if(entry===undefined) throw new Refusal(404,'not_found','This program is not registered.');
      if(entry.origin==='local') throw new Refusal(409,'policy.not_delegated','Remove this program from the local configuration, then Reload.');
      record.programs=entries.filter((candidate)=>candidate!==entry);
    }else if(body.action==='register') {
      const command=Array.isArray(body.command)?body.command.filter((arg):arg is string=>typeof arg==='string'):[];
      if(name==='' || command.length===0) this.invalid('command','Enter a program and its arguments.');
      if(entry!==undefined) throw new Refusal(409,'conflict.exists','This program is already registered.');
      if(command[0]?.includes('missing-program')===true) this.invalid('program','The program could not start. Check its command.');
      const descriptor=kind==='provider_source'?sourceDescriptor(name,name,null,true):{kind:name,adapter_version:1,harness:{name,version:'1.0.0',supported_range:'>=1.0.0',status:'supported'},arrangements:[{type:'together'}],approval_modes:['on_request'],sandbox_modes:['open']};
      record.programs=[...entries,{kind,name,command,origin:'orch',ready:true,version:'1.0.0',descriptor,models:kind==='provider_source'?{models:[{model_name:'local-model',display_name:'Local model'}]}:null,detail:null}];
    }else this.invalid('action','Choose Register, Test, Remove or Reload.');
    return this.machinePrograms(person,record);
  }

  private machine(record: MachineRecord): Machine {
    return {
      role_management:{supported:!record.roles?.includes('node'),delegated:this.machineKeyHanded(record,'machine.roles'),enabled:record.policy['machine.roles']===true},
      access:{permissions:['machine.view','machine.use','machine.manage','access.own','access.manage'],admin_only:[],administers:false,admin_access:null,participating:true,through:[{kind:'owner'}]},
      installation:this.installation(record),
      environment_sandboxes: [],
      software_update: this.componentUpdates.get(record.id) ?? {running_version:record.connector,recommended_version:record.connector,behind:false,status:'current',detail:null,command:'orch-connector update'},
      id: record.id,
      name: record.name,
      online: record.online && record.status === 'active',
      platform_os: record.os,
      placement: 'connected',
      owner: this.userOf(record.owner),
      platform_arch: record.arch,
      connector_version: record.connector,
      protocol_version: 1,
      roles: record.roles ?? ['agent', 'environment', 'provider'],
      status: record.status,
      last_seen_at: record.lastSeen,
      last_ip: record.lastSeen === null ? null : '203.0.113.7',
      cert_not_after: new Date(this.born + (record.certEnds ?? (record.status === 'revoked' ? -DAY : 200 * DAY))).toISOString(),
      facts: this.machineFacts(record),
      providers: this.providers.filter((provider) => provider.machine === record.id).map((provider) => this.providerRef(provider)),
      agents: this.agents.filter((agent) => agent.machine === record.id).map((agent) => this.agentRef(agent)),
      environments: this.environments.filter((environment) => environment.status !== 'removed' && environment.machine === record.id).map((environment) => this.environmentRef(environment)),
    };
  }

  private arrangementConsents(allowed: readonly ArrangementType[]): ArrangementConsent[] {
    const puts: Record<string, string> = {
      separated: 'Nothing: the harness and its tools stay apart.',
      key_separated: "A token that works only at Orch's forwarder.",
      credential_shared: 'A copy of the credential.',
      handed_over: 'The credential itself, to one environment at a time.',
    };
    return (['separated', 'key_separated', 'credential_shared', 'handed_over'] as const).map((type) => ({
      type,
      allowed: allowed.includes(type),
      available: true,
      puts: puts[type] ?? '',
      form: type === 'credential_shared' || type === 'handed_over' ? { name: 'api_key', title: 'API key', lifetime_secs: null } : null,
    }));
  }

  /** What the caller holds on a resource that has an owner and two lists of who else may use it and who may manage it. */
  private held(person: Person, kind: string, owner: string | null, uses: readonly string[], manages: readonly string[]): ResourceAccess {
    const username = person.me.username;
    const owns = owner === username;
    const manage = owns || manages.includes(username) || (this.administers(person) && owner !== null);
    const use = manage || uses.includes(username);
    const permissions = [...(owns ? ['access.manage', 'access.own', 'access.external'] : []), ...(use ? [`${kind}.view`, `${kind}.use`] : [`${kind}.view`]), ...(manage ? [`${kind}.manage`] : [])];
    return {
      participating: owns || manages.includes(username) || uses.includes(username),
      admin_only: this.administers(person) && !owns && !manages.includes(username)
        ? uses.includes(username) ? [`${kind}.manage`] : [...new Set(permissions)] : [],
      permissions: [...new Set(permissions)],
      administers: this.administers(person),
      through: owns ? [{ kind: 'owner' as const }] : use ? [{ kind: 'entry' as const, holder: { kind: 'user' as const, user: this.userOf(username) }, by: null }] : [],
    };
  }

  private provider(person: Person, record: ProviderRecord,withPlan=false): Provider {
    const machine = this.machines.find((candidate) => candidate.id === record.machine);
    const keyMachine = this.machines.find((candidate) => candidate.id === record.keyMachine);
    const agent = this.agents.find((candidate) => candidate.id === record.agent);
    const attached:unknown=agent?.profile.attach;
    const profileHome:unknown=isRecord(attached)?attached.home:undefined;
    const separateKeeper=record.keyMachine!==null && record.keyMachine!==record.machine;
    const selectedHome:unknown=record.runtimeConfig?.['runtime.home'] ?? (separateKeeper?{kind:'made_by_orch'}:profileHome);
    const source=(home:unknown):EnvironmentDataSources['home']=>typeof home==='string'?{kind:'existing_directory',path:home}
      :isRecord(home) && home.kind==='existing_directory' && typeof home.path==='string'?{kind:'existing_directory',path:home.path}:{kind:'made_by_orch'};
    const homeSource=source(selectedHome);
    const homeOwned=source(separateKeeper?profileHome:selectedHome).kind==='made_by_orch';
    const keeper=keyMachine;
    const signInPolicy=typeof keeper?.policy['provider.signin']==='boolean'?keeper.policy['provider.signin']:null;
    const offers=this.sourceOffers();
    const sources=record.sources ?? {facts:defaultProviderSource(record,'facts',offers),models:defaultProviderSource(record,'models',offers),presenter:null};
    const factsState:Provider['facts_state']={identity:'unsupported',credential:'not_read_yet',credential_verified:'unsupported',plan:'unsupported',windows:'unsupported',limit_reached:'unsupported',ordinary_usage_allowed:'unsupported',balances:'unsupported',spend:'unsupported',extra_usage:'unsupported',available_resets:'unsupported',models:'unsupported'};
    const reports=sources.facts.descriptor?.facts?.reports ?? [];
    for (const member of SOURCE_MEMBERS) {
      const value=record.facts[member];
      const holds=Array.isArray(value) ? value.length > 0 : value !== null;
      const told=record.facts.field_times[member] !== undefined || Object.keys(record.facts.field_times).some((key) => key.startsWith(member + '.'));
      factsState[member]=holds ? 'told' : told ? 'empty' : member === 'credential' || reports.includes(member) ? 'not_read_yet' : 'unsupported';
    }
    const listsModels=(sources.models.descriptor?.models ?? null) !== null;
    factsState.models=record.facts.models !== null ? record.facts.models.length > 0 ? 'told' : 'empty' : listsModels && record.facts.field_times.models !== undefined ? 'empty' : listsModels ? 'not_read_yet' : 'unsupported';
    return {
      runtime_support:{resource_limits:{cpu:'reported',memory:'reported',pids:'reported',disk:'reported'},data_storage:machine===undefined?undefined:{made_by_orch:'directory',existing_directory:machine.os==='linux',reason:machine.os==='linux'?null:'This platform has not verified existing harness homes.'},home:homeSource,machine_id:machine?.id,driver:null,lifecycle:machine===undefined?'unit':'attach',commands_after_start:{supported:machine?.os!=='windows',reason:machine?.os==='windows'?'This machine cannot supervise a complete command process group.':null},
        after_up:machine===undefined?null:machine.policy['environment.after_up']===true,
        sign_in:record.kind==='subscription'?{home_owned:homeOwned,policy_allowed:typeof signInPolicy==='boolean'?signInPolicy:null,allowed:homeOwned || signInPolicy===true}:null},
      id: record.id,
      name: record.name,
      kind: record.kind,
      harness_kind: record.harness, harness: maybeHarnessName(record.harness),
      address: `acme.${record.owner}.${record.name}`,
      placement: record.machine===null?'managed':'connected',
      status: record.status,
      owner: this.userOf(record.owner),
      owners: 1,
      org_id: ACME.org,
      home: this.nodeRef(record.home),
      agent: agent === undefined ? null : this.agentRef(agent),
      arrangements: this.arrangementConsents(record.arrangements),
      held_by: null,
      runtime_config: record.runtimeConfig ?? {},
      upstreams: record.upstreams,
      secrets_set: record.secrets,
      config: {},
      subscription: record.kind === 'subscription' ? { accepted: true, shareable: true } : null,
      protocols: [...new Set(record.upstreams.map((upstream) => upstream.protocol))],
      key_set: record.keySet,
      key_machine: record.kind!=='api' || record.keyHeldByOrch===true || keyMachine === undefined ? null : this.machineRef(keyMachine),
      credential_keeper:keeper===undefined?null:this.machineRef(keeper),
      machine: machine === undefined ? null : this.machineRef(machine),
      status_detail: record.statusDetail,
      status_at: LATER_AT,
      ...(withPlan && record.planUsage!==undefined ? {plan_forecasts:record.planUsage.forecasts} : {}),
      facts: record.facts,
      facts_state: factsState,
      sources,
      facts_overridden: [],
      last_reset: null,
      facts_at: record.planUsage?.at ?? (record.facts.plan === null ? null : inMs(-5 * 60_000)),
      facts_requested_at: record.facts.plan === null ? null : inMs(-5 * 60_000),
      models: record.models,
      running_threads: record.running,
      runtime: null,
      harnesses: record.harnessReports ?? [],
      revision: record.revision,
      created_at: record.createdAt,
      updated_at: LATER_AT,
      access: this.held(person, 'provider', record.owner, record.uses, record.manages),
    };
  }

  private agent(person: Person, record: AgentRecord): Agent {
    const machine = this.machines.find((candidate) => candidate.id === record.machine);
    return {
      id: record.id,
      name: record.name,
      address: record.home === null ? `system.${record.name}` : `acme.${record.owner ?? 'ada'}.${record.name}`,
      harness_kind: record.harness,
      machine: machine === undefined ? null : this.machineRef(machine),
      disabled: record.disabled,
      owner: record.owner === null ? null : this.userOf(record.owner),
      org_id: record.home === null ? null : ACME.org,
      home: record.home === null ? null : this.nodeRef(record.home),
      profile: record.profile,
      descriptor: record.descriptor,
      descriptor_overridden: [],
      revision: record.revision,
      created_at: MADE_AT,
      updated_at: LATER_AT,
      access: record.owner === null ? { participating:false,admin_only:this.administers(person)?['agent.manage']:[], permissions: this.administers(person) ? ['agent.view', 'agent.use', 'agent.manage'] : ['agent.view', 'agent.use'], administers: this.administers(person), through: [{ kind: 'entry', holder: { kind: 'everyone' }, by: null }] } : this.held(person, 'agent', record.owner, [], []),
    } as unknown as Agent;
  }

  private environment(person: Person, record: EnvironmentRecord): Environment {
    const machine = this.machines.find((candidate) => candidate.id === record.machine);
    const host = this.hosts.find((candidate) => candidate.id === record.host);
    const template = this.templates.find((candidate) => candidate.id === record.template);
    const bound = this.threadsIn(record.id);
    const visible = bound.filter((entry) => this.unseenThread?.(entry.thread) !== true && (record.owner === person.me.username || person.me.is_system_admin || record.uses.includes(person.me.username)));
    return {
      id: record.id,
      name: record.name,
      owner: this.userOf(record.owner),
      address: this.environmentAddress(record),
      placement: record.machine === null ? 'managed' : 'connected',
      status: record.status,
      org_id: this.world.places.rootOf(record.home) ?? record.home,
      machine: machine === undefined ? null : this.machineRef(machine),
      home: this.nodeRef(record.home),
      default_for: [],
      config_trust: [],
      threads: visible.map((entry) => ({ thread: this.placeRef(entry.thread, 'thread'), topic: this.placeRef(entry.topic, 'topic'), directory: entry.directory, owner: this.userOf(record.owner) })),
      threads_hidden: bound.length - visible.length,
      reach: 1 + record.uses.length,
      network: { mode: 'open', required_mode: 'open', network_mode: record.machine === null ? 'bridge' : 'connected', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: '', always: [], always_revision: '', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
      workspace_root: record.root,
      workspace_storage: null,
      data_sources: dataSources(record.config?.['env.data_sources']),
      config: record.config ?? {},
      temporary: record.temporary === undefined ? null : { expires_at: record.temporary.expiresAt, idle_hours: record.temporary.hours, on_archive: record.config?.['env.lifetime'] === 'thread' },
      removal: null,
      host: host === undefined ? null : { id: host.id, name: host.name, address: host.home === null ? `system.${host.name}` : `acme.${host.owner ?? 'ada'}.${host.name}` },
      mounts: [],
      exec: { offers: [{ name: 'codex-exec-server', available: true, reason: null }, { name: 'plain', available: true, reason: null }], orch_tools: true, machine_offers: record.machine === null ? null : ['codex-exec-server', 'claude-code', 'plain', 'custom'] },
      mcp_found: [],
      index: record.index ?? 'full',
      index_reason: record.index === 'live' ? 'entries' : null,
      template: template === undefined ? null : { id: template.id, name: template.name, driver: template.driver },
      generation: record.generation ?? 0,
      ports_exposed: this.world.ports.exposed(record.id),
      updates: Array.from({ length: record.updates }, (_, n) => ({ name: n === 0 ? 'Image' : 'Exec node', what: n === 0 ? 'image' : 'executor', from: '0.1.0', to: '0.1.1' })),
      update_required: false,
      remote_write: record.remoteWrite,
      terminal: record.terminal,
      // Browsers and displays: the switches and what was found there are those parts' own.
      browser: this.world.browsers.switchedOn(record.id),
      vnc: this.world.displays.switchedOn(record.id),
      support: this.world.browsers.support(record.id),
      granted_capabilities: [],
      status_detail: record.status === 'offline' ? 'Its machine is offline.' : null,
      facts: { environment_descriptor: { lifecycle: record.template === null ? 'attach' : 'unit' } },
      manifest_at: null,
      last_activity_at: LATER_AT,
      idle_stop_at: null,
      unused_since: null,
      remove_at: record.removeAt,
      remove_reason: record.removeAt === null ? null : 'request',
      job: this.jobRefOf?.(record.id) ?? (record.status === 'provisioning' ? { id: fixtureId(KIND.job, 1), kind: 'environment_provision', percent: 40, phase: 'files', state: 'running' } : null),
      seq: 0,
      revision: record.revision,
      access: { ...this.held(person, 'environment', record.owner, record.uses, record.manages), permissions: this.world.access.permissionsOnEnvironment(person, record.id) },
    } as unknown as Environment;
  }

  private host(person: Person, record: HostRecord): Host {
    const stored = this.environments.filter((environment) => environment.host === record.id);
    const running = stored.filter((environment) => environment.status !== 'removed').length;
    const access = this.held(person, 'host', record.owner, record.uses, record.manages);
    const keptData = access.permissions.includes('host.manage') ? this.world.trace.hostKeptData(person, record.id) : [];
    return {
      offline_since: record.online === false ? record.offlineSince ?? null : null,
      software_update: this.componentUpdates.get(record.id) ?? {running_version:'0.1.0',recommended_version:'0.1.0',behind:false,status:'current',detail:null,command:'orch-node update'},
      id: record.id,
      name: record.name,
      address: record.home === null ? `system.${record.name}` : `acme.${record.owner ?? 'ada'}.${record.name}`,
      capacity: { cpu_millis: 16_000, memory_mb: 65_536, disk_gb: 1_000 },
      declared: { cpu_millis: 2_000 * running, memory_mb: 4_096 * running, disk_gb: 20 * stored.length },
      drivers: [{ name: 'docker', title: 'Docker', creates: ['environment', 'provider'], runtime_classes: ['runc'] }, { name: 'host', title: 'Host', creates: ['environment', 'provider'], runtime_classes: [] }],
      home: record.home === null ? null : this.nodeRef(record.home),
      labels: record.labels,
      machine: { id: fixtureId(KIND.machine, 90 + (record.id.endsWith('1') ? 1 : 2)), name: record.name, online: record.online ?? true, platform_os: 'linux', placement: 'managed' },
      mine: this.environments.filter((environment) => environment.host === record.id && environment.owner === person.me.username).length,
      org_id: record.home === null ? null : ACME.org,
      owner: record.owner === null ? null : this.userOf(record.owner),
      revision: record.revision,
      running: { environment: running, provider: 0 },
      schedulable: record.schedulable,
      storages: [
        { name: 'ssd', default: true, capacity_gb: 800, free_gb: 540, private_views: true },
        { name: 'bulk', default: false, capacity_gb: 4_000, free_gb: 3_100, private_views: false },
      ],
      when_full: { own: record.whenFull !== null, value: record.whenFull ?? 'warn' },
      access,
      ...(keptData.length === 0 ? {} : { kept_data: keptData }),
      ...this.driverFacts.get(record.id),
    } as unknown as Host;
  }

  private runtime(record: RuntimeRecord): Runtime {
    const environment = this.environments.find((candidate) => candidate.id === record.environment);
    const provider = this.providers.find((candidate) => candidate.id === record.provider);
    const template = this.templates.find((candidate) => candidate.id === (environment?.template ?? fixtureId(KIND.template, 2)));
    return {
      id: record.id,
      host_id: record.host,
      kind: record.kind,
      driver: 'docker',
      environment:
        environment === undefined
          ? null
          : this.environmentRef(environment),
      provider: provider === undefined ? null : this.providerRef(provider),
      generation: 1,
      last_active_at: record.state === 'running' ? LATER_AT : null,
      machine: null,
      network: { mode: 'open', required_mode: 'open', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: '', always: [], always_revision: '', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
      remove_at: record.removeAt,
      state: record.state,
      state_detail: record.state === 'failed' ? 'The image could not be pulled.' : null,
      successor_of: null,
      template: template === undefined ? { id: fixtureId(KIND.template, 1), name: 'Default' } : { id: template.id, name: template.name },
      unit_name: record.unitName,
      updates: 0,
    };
  }

  /** A moment from now an environment that was told to start is running, as a job that takes a while would leave it. */
  private finishLater(id: string, to: Environment['status']): void {
    const timer = setTimeout(() => {
      this.timers.delete(timer);
      const record = this.environments.find((candidate) => candidate.id === id);
      if (record?.status !== 'provisioning') return;
      record.status = to;
      record.revision += 1;
      const runtime = this.runtimes.find((candidate) => candidate.environment === id);
      if (runtime === undefined) {
        this.runtimes.push({ id: this.next(KIND.runtime), host: record.host ?? fixtureId(KIND.host, 1), kind: 'environment', unitName: `orch-env-${record.name.toLowerCase().replaceAll(/[^a-z0-9]+/g, '-')}`, state: 'running', environment: id, provider: null, removeAt: null });
      } else {
        runtime.state = 'running';
      }
      for (const job of this.jobs.values()) {
        if (job.environment_id !== id || job.kind !== 'environment_provision' || job.state !== 'running') continue;
        this.jobs.set(job.id, { ...job, state: 'succeeded', finished_at: new Date().toISOString(),
          progress: { ...job.progress, percent: 100, phase: 'done' } });
      }
      void this.world.requestFeeds.changed();
    }, FINISH_MS);
    this.timers.add(timer);
  }

  private template(person: Person, record: TemplateRecord): Template {
    return {
      id: record.id,
      name: record.name,
      description: record.description,
      kind: record.kind,
      driver: record.driver,
      address: record.home === null ? `system.${record.name}` : `acme.${record.owner ?? 'ada'}.${record.name}`,
      org_id: record.home === null ? null : ACME.org,
      owner: record.owner === null ? null : this.userOf(record.owner),
      home: record.home === null ? null : this.nodeRef(record.home),
      spec: record.spec,
      enabled: record.enabled,
      revision: record.revision,
      units: record.units,
      access: record.owner === null ? { participating:false,admin_only:this.administers(person)?['template.manage']:[], permissions: this.administers(person) ? ['template.view', 'template.use', 'template.manage'] : ['template.view', 'template.use'], administers: this.administers(person), through: [{ kind: 'entry', holder: { kind: 'everyone' }, by: null }] } : this.held(person, 'template', record.owner, record.uses, record.manages),
    } as unknown as Template;
  }

  // ---------------------------------------------------------------- finding

  private findMachine(person: Person, id: string): MachineRecord {
    const found = this.machines.find((candidate) => candidate.id === id && candidate.owner === person.me.username);
    if (found === undefined) throw new Refusal(404, 'not_found', 'there is no such machine');
    return found;
  }

  private findProvider(person: Person, id: string): ProviderRecord {
    const found = this.providers.find((candidate) => candidate.id === id);
    if (found === undefined || !this.held(person, 'provider', found.owner, found.uses, found.manages).permissions.includes('provider.view')) {
      throw new Refusal(404, 'not_found', 'there is no such provider');
    }
    return found;
  }

  /** Parameter snapshots disclose only the same provider that its native ordinary read permits. */
  providerForFacts(person: Person, id: string): Provider | undefined {
    try { return this.provider(person, this.findProvider(person, id)); }
    catch (error) { if (error instanceof Refusal && error.answer.status === 404) return undefined; throw error; }
  }

  private mustManageProvider(person: Person, record: ProviderRecord): void {
    if (!this.held(person, 'provider', record.owner, record.uses, record.manages).permissions.includes('provider.manage')) {
      this.refuseOn('provider', record.id, record.name, record.owner, 'provider.manage');
    }
  }

  private findAgent(id: string): AgentRecord {
    const found = this.agents.find((candidate) => candidate.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'there is no such agent profile');
    return found;
  }

  private findEnvironment(id: string): EnvironmentRecord {
    const found = this.environments.find((candidate) => candidate.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'there is no such environment');
    return found;
  }

  private mustManageEnvironment(person: Person, record: EnvironmentRecord): void {
    if (!this.held(person, 'environment', record.owner, record.uses, record.manages).permissions.includes('environment.manage')) {
      this.refuseOn('environment', record.id, record.name, record.owner, 'environment.manage');
    }
  }

  /** A retained resource job is read under its environment's current permission. */
  jobOf(person: Person, id: string): Job | undefined {
    const job = this.jobs.get(id);
    const environmentId = job?.environment_id;
    if (typeof environmentId === 'string') {
      const record = this.findEnvironment(environmentId);
      if (!this.held(person, 'environment', record.owner, record.uses, record.manages).permissions.includes('environment.view')) {
        this.refuseOn('environment', record.id, record.name, record.owner, 'environment.view');
      }
    }
    return job;
  }

  private mustManageHost(person: Person, record: HostRecord): void {
    if (!this.held(person, 'host', record.owner, record.uses, record.manages).permissions.includes('host.manage')) {
      this.refuseOn('host', record.id, record.name, record.owner, 'host.manage');
    }
  }

  private findHost(id: string): HostRecord {
    const found = this.hosts.find((candidate) => candidate.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'there is no such host');
    return found;
  }

  private findTemplate(id: string): TemplateRecord {
    const found = this.templates.find((candidate) => candidate.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'there is no such template');
    return found;
  }

  private next(kind: number): string {
    this.count += 1;
    return fixtureId(kind, this.count);
  }

  /** Checks a revision named by a change against the one the record holds. */
  private revisionOk(body: Record<string, unknown>, held: number): void {
    if (typeof body.expected_revision === 'number' && body.expected_revision !== held) {
      throw new Refusal(409, 'conflict.stale_revision', 'somebody changed this in the meantime');
    }
  }

  private invalid(path: string, message: string): never {
    throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
  }

  // ---------------------------------------------------------------- machines

  private enrollmentToken(person: Person, again?: MachineRecord, selection?: EnrollmentSelection): EnrollmentToken {
    const token = `oenr_${randomUUID().replaceAll('-', '')}.pin`;
    this.tokens.add(token);
    // Same wire shape as the native fingerprint, with an independent synthetic identity.
    const receipt = randomUUID().replaceAll('-', '').repeat(2);
    this.receiptByToken.set(token, receipt);
    this.enrollmentReceipts.set(receipt, { owner: person.me.username, machine: null, host: null });
    if (again !== undefined) this.againTokens.set(token, again.id);
    const result: EnrollmentToken = {
      receipt,
      token,
      expires_at: inMs(15 * 60_000),
      commands: {
        linux: `curl -fsSL http://127.0.0.1/api/v1/downloads/install.sh | sh -s -- --url http://127.0.0.1 --token '${token}'`,
        macos: `curl -fsSL http://127.0.0.1/api/v1/downloads/install.sh | sh -s -- --url http://127.0.0.1 --token '${token}'`,
        windows: `& ([scriptblock]::Create((iwr http://127.0.0.1/api/v1/downloads/install.ps1 -UseBasicParsing))) -Url http://127.0.0.1 -Token '${token}'`,
      },
    };
    if (selection !== undefined) {
      this.enrollmentSelections.set(token, selection);
      if (selection.kind === 'connector') {
        this.lastConnectorToken = token;
        const roles = selection.roles.join(',');
        result.commands.linux += ` --roles '${roles}'`;
        result.commands.macos += ` --roles '${roles}'`;
        result.commands.windows += ` -Roles '${roles}'`;
      } else {
        this.lastHostToken = token;
        result.commands.linux = `curl -fsSL http://127.0.0.1/api/v1/downloads/install-node.sh | sudo sh -s -- --from http://127.0.0.1 --url wss://gateway.test:9443 --token '${token}'`;
        result.commands.macos = result.commands.linux;
        result.commands.windows = 'Write-Output "A host runs orch-node on Linux; use its Linux command."';
      }
      if (selection.delegate.length > 0) {
        const all = selection.delegate.includes('*') || selection.delegate.includes('all');
        const sections = selection.delegate.map((section) => section.replace(/\.\*$/, '')).join(',');
        const flag = all ? ' --delegate-policy' : ` --delegate '${sections}'`;
        result.commands.linux += flag; result.commands.macos += flag;
        if (selection.kind === 'connector') result.commands.windows += all ? ' -DelegatePolicy' : ` -Delegate '${sections}'`;
      }
      if (selection.dedicated) {
        result.commands.linux += ' --dedicated'; result.commands.macos += ' --dedicated'; result.commands.windows += ' -Dedicated';
      }
    }
    return result;
  }

  private changePolicy(person: Person, record: MachineRecord, body: Record<string, unknown>): MachinePolicy {
    if (!record.online) throw new Refusal(503, 'source.offline', 'the machine is offline');
    const delegated = Array.isArray(record.policy['policy.delegated']) ? (record.policy['policy.delegated'] as string[]) : [];
    const local = record.policy['policy.local'];
    const handed = (key: string): boolean => !(typeof local === 'object' && local !== null && Object.hasOwn(local,key)) && (delegated.includes('*') || delegated.includes(key) || policySections(key).some((section) => delegated.includes(`${section}.*`)));
    const refused = Object.keys(body).filter((key) => key === 'policy.delegated' || !handed(key));
    if (refused.length > 0) throw new Refusal(409, 'policy.not_delegated', 'the machine keeps those to itself', { keys: refused });
    Object.assign(record.policy, body);
    return {...this.policy(person, record),machine_view:this.machine(record)};
  }

  private policy(person: Person, record: MachineRecord): MachinePolicy {
    const delegated = Array.isArray(record.policy['policy.delegated']) ? (record.policy['policy.delegated'] as string[]) : [];
    const local = record.policy['policy.local'];
    const kept = (key: string): boolean => typeof local === 'object' && local !== null && Object.hasOwn(local,key);
    const handed = (key: string): boolean => key !== 'policy.delegated' && !kept(key) && (delegated.includes('*') || delegated.includes(key) || policySections(key).some((section) => delegated.includes(`${section}.*`)));
    return {
      command:this.installation(record).commands.policy,
      machine: this.machineRef(record),
      reported_at: inMs(-60_000),
      keys: POLICY_KEYS.filter(([key]) => key !== 'tools.fetch' && (key==='provider.programs' && record.roles?.includes('agent')===true || policySections(key).some((section) => section === 'machine' || (record.roles ?? ['agent','environment','provider']).includes(section) || section === 'environment' && record.roles?.includes('node')))).map(([key, bounds]) => ({
        definition: policyDefinition(key),
        key,
        value: record.policy[key] ?? null,
        bounds,
        delegated: handed(key),
        writable: handed(key) && record.owner === person.me.username,
      })),
    };
  }

  /** A machine that enrolls: the control request of the tests, as the connector's own command is on a real one. */
  enroll(body: Record<string, unknown>): MachineRecord {
    const token = asText(body.token) || this.lastConnectorToken;
    const selection = token === null ? undefined : this.enrollmentSelections.get(token);
    if (token !== null && !this.tokens.has(token)) throw new Refusal(410, 'gone', 'the enrollment token is gone');
    const receipt = token === null ? undefined : this.receiptByToken.get(token);
    const progress = receipt === undefined ? undefined : this.enrollmentReceipts.get(receipt);
    const owner = progress?.owner ?? (asText(body.owner) || 'ada');
    if (progress !== undefined && asText(body.owner) !== '' && body.owner !== owner) throw new Refusal(403, 'authz.denied', 'This enrollment belongs to another person.');
    const name = asText(body.name) === '' ? 'new-machine' : asText(body.name);
    const record: MachineRecord = {
      id: this.next(KIND.machine),
      name,
      owner,
      os: body.os === 'macos' || body.os === 'windows' ? body.os : 'linux',
      arch: 'x86_64',
      online: body.online !== false,
      status: 'active',
      connector: '0.1.0',
      lastSeen: inMs(0),
      roundTripMs: 30,
      harnesses: [{ kind: 'codex', path: '/usr/local/bin/codex', version: '0.162.1', daemon: false }],
      policy: shippedPolicy(),
      ...(selection === undefined ? {} : { roles: selection.roles, dedicated: selection.dedicated }),
    };
    if (selection !== undefined) {
      const delegated=selection.delegate.map((section)=>section==='*' || section==='all'?'*':`${section.replace(/\.\*$/,'')}.*`);
      record.policy['policy.delegated']=delegated;
      for(const [key] of POLICY_KEYS)if(key!=='policy.delegated' && (delegated.includes('*') || policySections(key).some((section)=>delegated.includes(`${section}.*`)))) record.policy[key]=policyDefinition(key).managed;
    }
    if (token !== null) { this.tokens.delete(token); this.enrollmentSelections.delete(token); if (token === this.lastConnectorToken) this.lastConnectorToken = null; }
    this.machines.push(record);
    if (receipt !== undefined && progress !== undefined) {
      progress.machine = record.id;
      this.announceEnrollment(receipt);
    }
    return record;
  }

  private enrollmentProgress(person: Person, receipt: string): EnrollmentProgress {
    const saved = this.enrollmentReceipts.get(receipt);
    if (saved?.owner !== person.me.username) throw new Refusal(404, 'not_found', 'This enrollment is not available.');
    const machine = saved.machine === null ? null : this.machine(this.findMachine(person, saved.machine));
    let host: Host | null = null;
    if (saved.host !== null) {
      const row = this.findHost(saved.host);
      if (!this.held(person, 'host', row.owner, row.uses, row.manages).permissions.includes('host.view')) this.refuseOn('host', row.id, row.name, row.owner, 'host.view');
      host = this.host(person, row);
    }
    return { receipt, machine, host };
  }

  private announceEnrollment(receipt: string): void {
    const saved = this.enrollmentReceipts.get(receipt);
    const person = saved === undefined ? undefined : this.world.people.get(saved.owner);
    if (saved === undefined || person === undefined) return;
    this.world.users.tell(saved.owner, { what: 'enrollment', receipt, status: this.enrollmentProgress(person, receipt) });
  }

  // ---------------------------------------------------------------- providers

  private providerList(person: Person, query: URLSearchParams): Provider[] {
    const scope = query.get('scope') ?? 'all';
    const username = person.me.username;
    const orgId = query.get('org_id');
    return this.providers
      .filter((record) => {
        const access = this.held(person, 'provider', record.owner, record.uses, record.manages).permissions;
        const mine = record.owner === username;
        if (scope === 'mine') return mine;
        // A view nests ownership and management; actual use still checks the provider's state.
        if (scope === 'usable') return mine || access.includes('provider.manage') || access.includes('provider.use');
        if (scope === 'managed') return mine || access.includes('provider.manage');
        if (scope === 'viewable') return mine || access.includes('provider.view');
        if (orgId !== null && this.administers(person)) return record.home === orgId || (orgId === ACME.org && record.home === ACME.org);
        return mine || access.includes('provider.use') || access.includes('provider.manage');
      })
      .map((record) => this.provider(person, record));
  }

  private providerSetup(): ProviderSetup {
    const field = (name: string, label: string, type: 'text' | 'choice' | 'map' | 'secret_map', help: string | null, choices: { value: string; label: string }[] | null = null) => ({
      name,
      label,
      type,
      required: false,
      help,
      choices,
      default: null,
      max_bytes: null,
    });
    const options = [
      field('auth', 'How the key is presented', 'choice', 'How the key is sent to this endpoint.', [
        { value: 'bearer', label: 'Authorization: Bearer <key>' },
        { value: 'x-api-key', label: 'x-api-key: <key>' },
        { value: 'none', label: 'No key' },
      ]),
      field('headers', 'Headers', 'map', 'Headers sent with every request to this endpoint.'),
      field('secret_headers', 'Secret headers', 'secret_map', 'Headers whose values are secrets. They are stored like the key and never shown again.'),
      field('query', 'Query parameters', 'map', 'Parameters added to the address of every request.'),
      // An option that its descriptor gives no help for: it has no "?".
      field('organization', 'Organization', 'text', null),
    ];
    const codex = this.agentRef(this.findAgent(fixtureId(KIND.agent, 1)));
    const claude = this.agentRef(this.findAgent(fixtureId(KIND.agent, 2)));
    return {
      machines:this.machines.filter((machine)=>machine.status==='active').map((machine)=>({machine:this.machineRef(machine),directory_runtime:true,roles:['agent','provider'],harnesses:machine.harnesses.map((harness)=>harness.kind)})),
      api_arrangements: this.arrangementConsents(['separated', 'key_separated']),
      http_allowed: false,
      key_machines: this.machines.filter((machine) => machine.status === 'active' && machine.online).map((machine) => this.machineRef(machine)),
      presets: [
        {name:'Local embeddings',note:null,docs_url:null,upstreams:[{protocol:'openai-embeddings',base_url:'http://127.0.0.1:8090/v1',options:{auth:'none'}}],
          model:{model_name:'embeddinggemma-2',display_name:'EmbeddingGemma 2'},dimensions:256,
          template:{name:'Local embeddings',home_id:null,kind:'provider',driver:'docker',spec:{
            allowed_capabilities:['port_place','port_host','port_link'],image:null,image_digest:null,archive:null,
            build:{dockerfile:readFileSync(new URL('../../../images/orch-embeddings/Dockerfile',import.meta.url),'utf8'),context:null,args:{},target:null},
            runtime:'runc',user:{name:'nonroot',uid:65532,gid:65532},resources:{cpu_millis:4000,memory_mb:2048,pids:512},
            network:{mode:'bridge',egress:'open',dns:[]},security:{read_only_rootfs:true,no_new_privileges:true,cap_add:[],cap_drop:['ALL']},
            env:{LLAMA_ARG_MODEL:'/opt/models/embeddinggemma-2-Q8_0.gguf',LLAMA_ARG_ALIAS:'embeddinggemma-2',LLAMA_ARG_EMBEDDINGS:'true',LLAMA_ARG_POOLING:'mean',LLAMA_ARG_HOST:'127.0.0.1',LLAMA_ARG_PORT:'8090',LLAMA_ARG_CTX_SIZE:'2048',LLAMA_ARG_BATCH:'2048',LLAMA_ARG_UBATCH:'2048',LLAMA_ARG_THREADS:'4',EMBEDDINGS_START_SECS:'120'},
            mounts:[{type:'tmpfs',target:'/tmp'}],after_up:[{name:'embeddings',command:['/bin/bash','/opt/llama/serve'],timeout_secs:300}],plugins:[],host_labels:[]}}},
        { name: 'OpenAI', note: null, docs_url: null, upstreams: [{ protocol: 'openai-responses', base_url: 'https://api.openai.com/v1', options: {} }] },
        { name: 'Anthropic', note: null, docs_url: null, upstreams: [{ protocol: 'anthropic-messages', base_url: 'https://api.anthropic.com', options: {} }] },
        { name: 'OpenRouter', note: null, docs_url: null, upstreams: [{ protocol: 'openai-chat', base_url: 'https://openrouter.ai/api/v1', options: {} }] },
        { name: 'DeepSeek', note: 'Serves both formats on the same key.', docs_url: null, upstreams: [{ protocol: 'openai-responses', base_url: 'https://api.deepseek.com', options: {} }, { protocol: 'anthropic-messages', base_url: 'https://api.deepseek.com/anthropic', options: {} }] },
      ],
      protocols: [
        { name:'openai-embeddings',title:'OpenAI Embeddings',agents:[],endpoint_options:options},
        { name: 'openai-responses', title: 'OpenAI Responses', agents: [codex], endpoint_options: options },
        { name: 'openai-chat', title: 'OpenAI Chat Completions', agents: [], endpoint_options: options },
        { name: 'anthropic-messages', title: 'Anthropic Messages', agents: [claude], endpoint_options: options },
      ],
      runtime_templates: [],
      sources: this.sourceOffers(),
      subscriptions: [
        { agent: codex, harness: { kind: 'codex', name: 'Codex' }, arrangements: this.arrangementConsents(['separated', 'credential_shared']), methods: [], options_schema: {}, template: { id: fixtureId(KIND.template, 2), name: 'Provider runtime' } },
        { agent: claude, harness: { kind: 'claude-code', name: 'Claude Code' }, arrangements: this.arrangementConsents(['separated']), methods: [], options_schema: {}, template: { id: fixtureId(KIND.template, 2), name: 'Provider runtime' } },
      ],
      subscriptions_unshared: [],
      subscriptions_withheld: [],
    };
  }

  private makeProvider(person: Person, body: Record<string, unknown>): Provider {
    const home = asText(body.home_id);
    this.needSettings(person, home);
    const native=body.kind==='subscription';
    const agent=native?this.findAgent(asText(body.agent_id)):undefined;
    const name = asText(body.name).trim();
    if (name === '') this.invalid('name', 'give it a name');
    const upstreams = Array.isArray(body.upstreams) ? (body.upstreams as Provider['upstreams']) : [];
    if (!native && upstreams.length === 0) this.invalid('upstreams', 'at least one endpoint');
    for (const [index, upstream] of upstreams.entries()) {
      if (!upstream.base_url.startsWith('https://')) this.invalid(`upstreams[${index}].base_url`, 'the address must be https');
    }
    const runtimeMachine=asText(body.runtime_machine_id);
    const selectedKeeper=asText(body.keeper_machine_id ?? body.key_machine_id);
    if (agent?.machine!==undefined && agent.machine!==null && selectedKeeper!=='' && selectedKeeper!==agent.machine) this.invalid('keeper_machine_id','this native profile is registered on another machine');
    const keyMachine = selectedKeeper===''?agent?.machine ?? runtimeMachine:selectedKeeper;
    const secrets = isRecord(body.secrets) ? Object.keys(body.secrets) : [];
    const record: ProviderRecord = {
      id: this.next(KIND.provider),
      name,
      kind: native?'subscription':'api',
      harness: agent?.harness ?? null,
      owner: person.me.username,
      home,
      machine: runtimeMachine===''?null:runtimeMachine,
      keyMachine: keyMachine === '' ? null : keyMachine,
      keyHeldByOrch:asText(body.api_key)!=='',
      placement: runtimeMachine===''?'managed':'connected',
      status: 'needs_setup',
      statusDetail: null,
      arrangements: Array.isArray(body.arrangements) ? (body.arrangements as ArrangementType[]) : native?['separated']:['separated', 'key_separated'],
      upstreams,
      models: [],
      keySet: asText(body.api_key) !== '' || keyMachine !== '',
      secrets,
      facts: emptyProviderFacts(),
      running: 0,
      uses: body.access === 'home' ? ['dora', 'grace', 'vera'] : [],
      manages: [],
      agent: agent?.id ?? null,
      revision: 1,
      createdAt: inMs(0),
      heldBy: null,
    };
    if (isRecord(body.runtime_config)) record.runtimeConfig=body.runtime_config;
    if (isRecord(body.sources)) this.chooseSources(person,record,body.sources);
    this.providers.push(record);
    return this.provider(person, record);
  }

  /** Same source wire and revision rules, with no program/network invocation or secret-value echo in the fake world. */
  private chooseSources(person:Person,record:ProviderRecord,body:Record<string,unknown>):void {
    const sources=structuredClone(this.provider(person,record).sources);
    let set=[...record.secrets];
    const offers=this.sourceOffers();
    for (const element of ['facts','models'] as const) {
      const value=body[element];
      if (value === undefined || value === null) continue;
      if (!isRecord(value)) this.invalid(element,'choose a source');
      const by=asText(value.by);
      if (by !== 'default' && by !== 'none' && by !== 'program' && by !== 'address' && by !== 'builtin') this.invalid(element,'choose a source type');
      const before=sources[element];
      let next:SourceChoice={by,name:null,address:null,config:isRecord(value.config) ? value.config : {},secrets:[],descriptor:null,described_at:null,last_error:null};
      if (by === 'default') next=defaultProviderSource(record,element,offers);
      if (by === 'program' || by === 'builtin') {
        const name=asText(value.name);
        const template=record.runtimeConfig?.['runtime.template_id'];
        const offer=offers.find((offer) => {
          const implementation=offer.descriptor?.[element];
          if (implementation === undefined || implementation === null) return false;
          return offer.by === by && offer.name === name && (by === 'builtin' || offer.machine?.id === record.keyMachine || record.keyMachine===null && typeof template === 'string' && offer.template?.id === template);
        });
        if (offer === undefined) this.invalid(element,'that source is not offered at this keeper');
        next.name=name;next.descriptor=offer.descriptor;
      }
      if (by === 'address') {
        if (!isRecord(value.address)) this.invalid(element,'give the source address');
        const address=asText(value.address.url);
        let url:URL;
        try {url=new URL(address);} catch {this.invalid(element,'give an HTTPS address');}
        if (url.protocol !== 'https:' || url.username !== '' || url.password !== '' || url.hash !== '') this.invalid(element,'the address must be HTTPS without a username, password or fragment');
        const auth=asText(value.address.auth);
        if (auth !== 'none' && auth !== 'bearer' && auth !== 'header') this.invalid(element,'choose the address authorization');
        const send=value.address.send_credential === true;
        if (send && record.kind !== 'api') this.invalid(element,'a sign-in cannot be sent');
        next.address={url:address,auth,send_credential:send,...(auth === 'header' ? {header:asText(value.address.header)} : {})};
      }
      const same=by === before.by && next.name === before.name && next.address?.url === before.address?.url && next.address?.auth === before.address?.auth && next.address?.header === before.address?.header;
      if (same) {next.descriptor=before.descriptor;next.described_at=before.described_at;}
      const prefix='source:' + element + ':';
      if (!same || by === 'default' || by === 'none') set=set.filter((name) => !name.startsWith(prefix));
      next.secrets=same ? [...before.secrets] : [];
      for (const [name,text] of Object.entries(isRecord(value.secrets) ? value.secrets : {})) {
        if (text !== null && typeof text !== 'string') this.invalid(element,'a secret field is text or null');
        const key=prefix + 'field:' + name;
        set=set.filter((entry) => entry !== key);next.secrets=next.secrets.filter((entry) => entry !== name);
        if (typeof text === 'string' && text !== '') {set.push(key);next.secrets.push(name);}
      }
      const authorization=prefix + 'authorization';
      if (value.authorization !== undefined || next.address?.auth === 'none') {
        set=set.filter((name) => name !== authorization);
        if (typeof value.authorization === 'string' && value.authorization !== '' && next.address?.auth !== 'none') set.push(authorization);
      }
      if (next.address !== null && next.address.auth !== 'none' && !set.includes(authorization)) this.invalid(element,'give the authorization value for this address');
      if (by === 'address' || by === 'program') next.last_error={code:'not_told',message:'This fake world does not run external sources.',at:inMs(0)};
      sources[element]=next;
    }
    record.sources=sources;record.secrets=set;
  }

  private changeProvider(person: Person, record: ProviderRecord, body: Record<string, unknown>): Provider {
    this.mustManageProvider(person, record);
    this.revisionOk(body, record.revision);
    if (typeof body.name === 'string' && body.name.trim() !== '') record.name = body.name.trim();
    if (Array.isArray(body.arrangements)) {
      const chosen = body.arrangements as ArrangementType[];
      if (chosen.length === 0) this.invalid('arrangements', 'at least one type stays allowed');
      record.arrangements = chosen;
    }
    if (Array.isArray(body.upstreams)) record.upstreams = body.upstreams as Provider['upstreams'];
    if (isRecord(body.runtime_config)) record.runtimeConfig=body.runtime_config;
    record.revision += 1;
    return this.provider(person, record);
  }

  private recomputeStatus(record: ProviderRecord): void {
    if (record.status === 'disabled' || record.status === 'exhausted' || record.status === 'error') return;
    const ready = (record.keySet || record.kind === 'subscription') && record.models.some((model) => model.enabled);
    record.status = ready ? 'ready' : 'needs_setup';
  }

  /** What the source's own list names: a model that lists what it can do beyond the usual is kept as offered when it is taken. */
  private suggestions(): ModelSuggestion[] {
    return [
      { model_name: 'gpt-5.4-nano', display_name: 'GPT-5.4 Nano', listed: true, parameters: { efforts: ['low'], context_window: 128_000, rate: { input_per_mtok: '0.10', cached_input_per_mtok: '0.01', output_per_mtok: '0.40' } } },
      { model_name: 'gpt-5.4-mini', display_name: 'GPT-5.4 Mini', listed: true, parameters: { efforts: ['low', 'medium'] } },
      { model_name: 'o-research', display_name: '', listed: true, parameters: {} },
      { model_name: 'gpt-6-sol', display_name: 'GPT-6 Sol (A)', listed: true, parameters: { efforts: ['low', 'medium', 'high'], context_window: 272_000, context_max: 1_000_000, service_tiers: [{ id: 'fast', label: 'Fast' }], access_programs: ['Daybreak Blue'], rate: STEPPED_RATE } },
    ];
  }

  private models(person: Person, record: ProviderRecord, method: string, rest: string, body: Record<string, unknown>): Answer | null {
    let found: RegExpExecArray | null;
    if (method === 'GET' && rest === '') return { status: 200, body: record.models };
    if (method === 'POST' && rest === '') {
      this.mustManageProvider(person, record);
      const routing = asText(body.model_name).trim();
      if (routing === '') this.invalid('model_name', 'give the name that is sent');
      const display = asText(body.display_name).trim();
      if (record.models.some((model) => model.model_name === routing || (display !== '' && model.display_name === display))) {
        throw new Refusal(409, 'conflict.exists', 'the provider has that model already');
      }
      const parameters = isRecord(body.parameters) ? (body.parameters as ModelRecord['parameters']) : {};
      const groups = Array.isArray(body.groups) ? (body.groups as string[]) : [];
      checkEntry(parameters, groups);
      const model: ModelRecord = { ...MODEL_ESTIMATE, id: this.next(KIND.model), model_name: routing, display_name: display, enabled: true, source: 'manual', groups: [...new Set(groups)], parameters };
      record.models.push(model);
      this.recomputeStatus(record);
      return { status: 200, body: model };
    }
    if (method === 'GET' && rest === '/suggestions') {
      this.mustManageProvider(person, record);
      if (record.kind === 'api' && record.machine === null) throw new Refusal(503, 'provider.unavailable', 'the provider has no machine to ask');
      const known = new Set(record.models.map((model) => model.model_name));
      return { status: 200, body: this.suggestions().filter((suggestion) => !known.has(suggestion.model_name)) };
    }
    if (method === 'POST' && rest === '/suggestions/accept') {
      this.mustManageProvider(person, record);
      const names = Array.isArray(body.model_names) ? (body.model_names as string[]) : [];
      const listed = this.suggestions();
      const added: ModelRecord[] = names.map((name) => {
        const suggestion = listed.find((one) => one.model_name === name);
        return { ...MODEL_ESTIMATE, id: this.next(KIND.model), model_name: name, display_name: suggestion?.display_name ?? '', enabled: true, source: 'suggested', groups: [], parameters: acceptedEntry(suggestion?.parameters ?? {}) };
      });
      record.models.push(...added);
      this.recomputeStatus(record);
      return { status: 200, body: {added,provider:this.provider(person,record)} };
    }
    if ((found = /^\/([^/]+)$/.exec(rest))) {
      const model = record.models.find((candidate) => candidate.id === found?.[1]);
      if (model === undefined) throw new Refusal(404, 'not_found', 'there is no such model');
      this.mustManageProvider(person, record);
      if (method === 'DELETE') {
        record.models = record.models.filter((candidate) => candidate.id !== model.id);
        this.recomputeStatus(record);
        return { status: 204 };
      }
      if (method === 'PATCH') {
        const display = body.display_name === undefined ? undefined : asText(body.display_name).trim();
        if (display !== undefined && display !== '' && record.models.some((other) => other.id !== model.id && other.display_name === display)) {
          throw new Refusal(409, 'conflict.exists', 'another model shows under that name');
        }
        const parameters = isRecord(body.parameters) ? (body.parameters as ModelRecord['parameters']) : model.parameters;
        const groups = Array.isArray(body.groups) ? (body.groups as string[]) : model.groups;
        checkEntry(parameters, groups);
        if (display !== undefined) model.display_name = display;
        if (typeof body.enabled === 'boolean') model.enabled = body.enabled;
        // What the source offered stays with the entry even when the request leaves it out.
        model.parameters = parameters.offered === undefined && model.parameters.offered !== undefined ? { ...parameters, offered: model.parameters.offered } : parameters;
        model.groups = [...new Set(groups)];
        this.recomputeStatus(record);
        return { status: 200, body: model };
      }
    }
    return null;
  }

  private providerThreads(record: ProviderRecord): unknown {
    const items = this.threadsOn(record.id).map((thread) => {
      const row = this.world.places.rowOf(thread);
      return {
        thread: this.placeRef(thread, 'thread'),
        topic: this.placeRef(row?.parent_id ?? '', 'topic'),
        org: this.placeRef(ACME.org, 'org'),
        state: row?.thread?.state ?? 'idle',
        blocked_reason: row?.thread?.blocked_reason ?? null,
        owner: this.userOf(this.world.places.metaOf(thread).owner),
      };
    });
    return { items, hidden: 0, next_cursor: null };
  }

  private settingsOf(entries: [string, unknown][]): ResolvedSetting[] {
    return entries.map(([key, value]) => ({ key, value, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true }) as unknown as ResolvedSetting);
  }

  // ---------------------------------------------------------------- what a thread could be bound to, before it exists

  /**
   * The executors an environment offers a harness that runs apart from its tools, in the order it tries them: what its
   * template declares, what its machine's local policy lets its node offer, and none for one that was made bare.
   */
  private offersOf(record: EnvironmentRecord | null): string[] {
    if (record !== null && this.bare.has(record.id)) return [];
    if (record !== null && record.machine !== null) {
      const listed = this.machines.find((candidate) => candidate.id === record.machine)?.policy['exec.offers'];
      return Array.isArray(listed) ? (listed as string[]) : [];
    }
    const template = this.templates.find((candidate) => candidate.id === (record?.template ?? fixtureId(KIND.template, 1)));
    const exec = isRecord(template?.spec.exec) ? template.spec.exec : {};
    return Array.isArray(exec.offers) ? (exec.offers as string[]) : [];
  }

  /**
   * Every arrangement the adapter of a profile declares, with a provider and in an environment: whether it can be had, whether the
   * owner of the provider and the places above allow it, and why not; in the order a thread that names none would take them, the
   * ones that can be had first. `null` for the environment is the one that would be made from the default template.
   */
  arrangementsOf(agentId: string, providerId: string, environmentId: string | null, prefer: readonly ArrangementType[] = [], allowed: readonly ArrangementType[] | null = null): Arrangement[] {
    const agent = this.findAgent(agentId);
    const provider = this.providers.find((candidate) => candidate.id === providerId);
    if (provider === undefined) throw new Refusal(404, 'not_found', 'there is no such provider');
    const environment = environmentId === null ? null : this.findEnvironment(environmentId);
    const declared = Array.isArray(agent.descriptor.arrangements) ? (agent.descriptor.arrangements as { type?: string; executor?: string; tool_sets?: string[] }[]) : [];
    const offers = this.offersOf(environment);
    const owner = this.userOf(provider.owner).display_name;
    const rows = declared.flatMap((entry) => {
      const type = entry.type as ArrangementType | undefined;
      if (type === undefined) return [];
      let reason: string | null = null;
      let detail: string | null = null;
      let executor: string | null = null;
      let toolSet: string | null = null;
      if (type === 'separated') {
        const usable = entry.tool_sets ?? [];
        const first = offers.find((offer) => usable.includes(offer) || offer === entry.executor);
        if (provider.kind === 'subscription' && provider.harness !== agent.harness) {
          reason = 'harness_mismatch';
          detail = 'This provider is a subscription of another harness than this agent profile runs.';
        } else if (first === undefined) {
          reason = 'executor_missing';
          detail = `The environment offers ${offers.length === 0 ? 'nothing' : offers.join(', ')} and the harness can work through ${usable.join(', ')}.`;
        } else {
          executor = entry.executor ?? null;
          toolSet = usable.includes(first) ? first : (usable[0] ?? null);
        }
      } else if (type === 'key_separated' && provider.kind !== 'api') {
        reason = 'no_common_protocol';
        detail = 'The harness and the provider speak no protocol in common.';
      }
      const available = reason === null;
      const permitted = provider.arrangements.includes(type) && (allowed === null || allowed.includes(type));
      if (available && !permitted) {
        reason = 'arrangement_not_allowed';
        detail = `${owner} does not allow this provider to be used this way.`;
      }
      return [{ type, executor, toolSet, available, permitted, reason, detail }];
    });
    const order = (type: ArrangementType): number => {
      const at = prefer.indexOf(type);
      return at === -1 ? prefer.length : at;
    };
    // Stable: the types a scope prefers first, in its order, and the rest as the adapter lists them.
    const sorted = [...rows].sort((one, other) => order(one.type) - order(other.type));
    let rank = 0;
    const ranked = new Map<ArrangementType, number>();
    for (const open of [true, false]) {
      for (const row of sorted.filter((entry) => (entry.available && entry.permitted) === open)) {
        rank += 1;
        ranked.set(row.type, rank);
      }
    }
    return sorted
      .map((row): Arrangement => ({
        type: row.type,
        executor: row.executor,
        tool_set: row.toolSet,
        isolation: this.isolationFor(row.type, provider, row.executor, row.toolSet),
        rank: ranked.get(row.type) ?? 0,
        available: row.available,
        permitted: row.permitted,
        reason: row.reason,
        not_allowed_by: row.reason === 'arrangement_not_allowed' ? 'provider' : null,
        scope: null,
        detail: row.detail,
      }))
      .sort((one, other) => one.rank - other.rank);
  }

  /** What an arrangement comes to for a provider: where the harness runs, how the credential travels and whose tools the agent works with. */
  private isolationFor(type: ArrangementType, provider: ProviderRecord, executor: string | null, toolSet: string | null): Isolation {
    const apart = type === 'separated';
    const path = type === 'separated' ? (provider.kind === 'subscription' ? 'native' : 'forwarded') : type === 'key_separated' ? 'forwarded' : type === 'credential_shared' ? 'shared' : 'handover';
    return {
      type,
      harness_runs_in: apart ? 'provider' : 'environment',
      executor: apart ? executor : null,
      tools: apart ? 'executor' : 'own',
      tool_set: apart ? toolSet : null,
      orch_tools: { offered: true, reason: null },
      credential_path: path,
      held_by: type === 'handed_over' ? 'environment' : 'provider',
      form: type === 'credential_shared' || type === 'handed_over' ? 'api_key' : null,
      lifetime_secs: null,
      model_enforcement: path === 'forwarded' ? 'wire' : 'adapter',
      metering: path === 'forwarded' ? 'wire' : 'harness',
    };
  }

  /** What a thread that was made with an arrangement of its own comes to; `null` for the threads the world began with, which say what the fixtures say. */
  isolationOf(binding: ThreadBinding): Isolation | null {
    const provider = this.providers.find((candidate) => candidate.id === binding.provider);
    if (binding.arrangement === undefined || provider === undefined) return null;
    const row = this.arrangementsOf(binding.agent, binding.provider, binding.environment).find((entry) => entry.type === binding.arrangement);
    return row?.isolation ?? this.isolationFor(binding.arrangement, provider, null, null);
  }

  /**
   * The arrangement a new thread is bound with. One that is named has to be had and allowed, or the request is refused with why; none
   * named takes the first that can be had, as the server does. Nothing is ever put in place of one that was named.
   */
  settle(agentId: string, providerId: string, environmentId: string | null, requested: ArrangementType | null, prefer: readonly ArrangementType[], allowed: readonly ArrangementType[] | null): ArrangementType {
    const rows = this.arrangementsOf(agentId, providerId, environmentId, prefer, allowed);
    const named = requested === null ? undefined : rows.find((row) => row.type === requested);
    const taken = requested === null ? rows.find((row) => row.available && row.permitted) : named;
    if (taken?.available === true && taken.permitted) return taken.type;
    const why = taken ?? rows[0];
    const by = why?.not_allowed_by ?? null;
    throw new Refusal(409, 'binding.incompatible', why?.detail ?? 'no arrangement can place this agent here', { reason: why?.reason ?? 'arrangement_not_available', ...(by === null ? {} : { not_allowed_by: by }) });
  }

  /** The agent profiles a provider could run, as the server lists them for the dialog: those that can be placed with what each can be placed in, and those whose harness never reported, not offered. */
  private agentsFor(provider: ProviderRecord, environmentId: string | null, prefer: readonly ArrangementType[], allowed: readonly ArrangementType[] | null): BindingAgent[] {
    return this.agents
      .filter((agent) => (provider.kind === 'subscription' ? agent.harness === provider.harness : agent.machine === null || agent.machine === provider.machine))
      .map((agent): BindingAgent => {
        const ref = this.agentRef(agent);
        if (agent.disabled) return { agent: ref, usable: false, reason: 'disabled', arrangements: [] };
        if (Object.keys(agent.descriptor).length === 0) return { agent: ref, usable: false, reason: 'harness_unreported', arrangements: [] };
        const arrangements = this.arrangementsOf(agent.id, provider.id, environmentId, prefer, allowed);
        const usable = arrangements.some((choice) => choice.available && choice.permitted);
        return { agent: ref, usable, reason: usable ? null : arrangements[0]?.reason ?? 'binding_incompatible', arrangements };
      });
  }

  /**
   * Every provider the caller can see, with the models and the profiles it offers; the environments the caller manages, this topic's first;
   * and whether they may make one. `ground` is what the settings of the place say; `environmentId` the environment the arrangements are
   * judged against (the topic's default, or the one a new thread would make, where none is named).
   */
  bindingOptions(person: Person, ground: BindingGround, environmentId: string | null, environmentNew = false): BindingOptions {
    const record = environmentId === null ? null : this.findEnvironment(environmentId);
    if (record !== null) this.mustManageEnvironment(person, record);
    const topicDefault = ground.topicDefault === null ? null : (this.environments.find((candidate) => candidate.id === ground.topicDefault) ?? null);
    const judged = record ?? (!environmentNew && ground.environmentDefault === 'shared' && topicDefault !== null && this.managed(person, topicDefault) ? topicDefault : null);
    const providers = this.providers
      .filter((candidate) => {
        const access = this.held(person, 'provider', candidate.owner, candidate.uses, candidate.manages).permissions;
        return candidate.owner === person.me.username || access.includes('provider.use') || access.includes('provider.manage');
      })
      .map((candidate): BindingProvider => {
        const may = this.held(person, 'provider', candidate.owner, candidate.uses, candidate.manages).permissions.includes('provider.use');
        const reason = !may ? 'no_use' : candidate.status === 'disabled' ? 'disabled' : null;
        return {
          provider: this.providerRef(candidate),
          usable: reason === null,
          reason,
          models: candidate.models.map((model): ModelOption => ({ ...MODEL_ESTIMATE, allowed: model.enabled, display_name: model.display_name, model_name: model.model_name, parameters: model.parameters, reason: model.enabled ? null : 'disabled' })),
          agents: this.agentsFor(candidate, judged?.id ?? null, ground.prefer, ground.allowed),
        };
      });
    const mayMake = this.mayMake(person);
    return {
      defaults: { provider_id: ground.defaults.provider, agent_id: ground.defaults.agent, model_name: ground.defaults.model, effort: ground.defaults.effort, locked: ground.defaults.locked },
      providers,
      environments: this.environmentsFor(person, ground),
      environment_default: ground.environmentDefault,
      source: { repository_url: null, branch: null, secret_name: null, from: null },
      access: { managed: this.summary('developer'), connected: this.summary(null) },
      new_environment: this.summary('developer'),
      can_create: { allowed: mayMake, missing: mayMake ? [] : [this.missingToMake()] },
      // The fake world's hosts and machines are not offered as places of a new thread yet.
      hosts: [],
      machines: [],
    };
  }

  /** Whether the person looks after an environment. */
  private managed(person: Person, record: EnvironmentRecord): boolean {
    return this.held(person, 'environment', record.owner, record.uses, record.manages).permissions.includes('environment.manage');
  }

  /** The environments the person manages for a place to start a thread in: its topic's default first, then those its threads run in, then the rest. */
  private environmentsFor(person: Person, ground: BindingGround): BindingEnvironment[] {
    const topic = ground.node.kind === 'topic' ? ground.node.id : null;
    const project = ground.node.kind === 'project' ? ground.node.id : (this.world.places.rowOf(ground.node.id)?.parent_id ?? null);
    const listed = this.environments
      .filter((record) => record.status !== 'removed' && this.managed(person, record))
      .map((record) => {
        const inside = this.threadsIn(record.id);
        const topics = [...new Set(inside.map((entry) => entry.topic))];
        const here = topic !== null && topics.includes(topic);
        const near = topics.some((candidate) => candidate !== topic && this.world.places.rowOf(candidate)?.parent_id === project);
        const used: BindingEnvironment['used'] = here ? 'topic' : near ? 'project' : topics.length > 0 ? 'elsewhere' : 'unused';
        const machine = this.machines.find((candidate) => candidate.id === record.machine);
        const host = this.hosts.find((candidate) => candidate.id === record.host);
        const isDefault = ground.topicDefault === record.id;
        const row: BindingEnvironment = {
          environment: this.environmentRef(record),
          host: host === undefined ? null : { id: host.id, name: host.name, address: host.home === null ? `system.${host.name}` : `acme.${host.owner ?? 'ada'}.${host.name}` },
          machine: machine === undefined ? null : this.machineRef(machine),
          threads: inside.length,
          usable: record.status !== 'disabled' && record.removeAt === null,
          reason: record.status === 'disabled' ? 'disabled' : record.removeAt === null ? null : 'being_removed',
          is_default: isDefault,
          used,
          topics: topics.map((candidate) => this.placeRef(candidate, 'topic')),
          topics_hidden: 0,
          directory: here ? (inside.find((entry) => entry.topic === topic)?.directory ?? '') : '',
        };
        return { row, group: isDefault ? 0 : here ? 1 : used === 'project' ? 2 : used === 'elsewhere' ? 3 : 4 };
      });
    return listed.sort((one, other) => one.group - other.group).map((entry) => entry.row);
  }

  /** What a page says of who holds what, cut down to the roles the lines under a composer would name. */
  private summary(role: 'developer' | null): AccessSummary {
    const roles = role === null ? [] : [{ kind: 'role' as const, role, count: 3, at: null }];
    return { custom: false, from: this.nodeRef(ACME.org), interact: roles, manage: roles, view: [{ kind: 'everyone' }] };
  }

  /** What a person lacks to make an environment: managing settings in the org, and whom to ask for it. */
  private missingToMake(): MissingAccess {
    const owner = this.userOf('ada');
    return { object: { kind: 'org', id: ACME.org, name: 'Acme', owner }, permissions: ['settings.manage'], options: null, ask: [owner], ask_more: 0, override: false, closed: null };
  }

  /** The harness of a profile reports what it is, or has not: what an adapter says once it has started on a machine. */
  report(agentId: string, reported: boolean): void {
    const agent = this.findAgent(agentId);
    const kind = agent.harness;
    agent.descriptor = reported ? descriptor(kind, kind === 'claude-code' ? 'Claude Code' : agent.name, '1.0.0') : {};
    agent.revision += 1;
  }

  // ---------------------------------------------------------------- the dispatcher

  /** A row already selected by this world's existing list reader, in the inventory's tagged wire form. */
  inventoryView(person: Person, kind: string, id: string): EverythingResource | null {
    switch (kind) {
      case 'provider': { const row = this.providers.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.provider(person, row) }; }
      case 'agent': { const row = this.agents.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.agent(person, row) }; }
      case 'environment': { const row = this.environments.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.environment(person, row) }; }
      case 'host': { const row = this.hosts.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.host(person, row) }; }
      case 'machine': { const row = this.machines.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.machine(row) }; }
      case 'template': { const row = this.templates.find((row) => row.id === id); return row === undefined ? null : { kind, value: this.template(person, row) }; }
      default: return null;
    }
  }

  /** The same current binding/activity records drive the resource's operation preview. */
  private impact(person:Person,body:Record<string,unknown>):ResourceImpact {
    const object=isRecord(body.object)?body.object:{};
    const kind=asText(object.kind),id=asText(object.id);
    if(!['provider','agent','environment'].includes(kind)||id==='')this.invalid('object','name a provider, profile or environment');
    const provider=kind==='provider'?this.findProvider(person,id):null;
    const value=provider!==null?this.provider(person,provider):kind==='agent'?this.agent(person,this.findAgent(id)):this.environment(person,this.findEnvironment(id));
    if(!value.access.permissions.includes(`${kind}.view`)&&!value.access.permissions.includes(`${kind}.manage`))throw new Refusal(404,'not_found','there is no such resource');
    const modelId=body.model_id===null||body.model_id===undefined?null:asText(body.model_id);
    const model=modelId===null?null:provider?.models.find((model)=>model.id===modelId);
    if(modelId!==null&&model===undefined)throw new Refusal(404,'not_found','there is no such model');
    const ids=[...this.bindings].filter(([thread,binding])=>(kind==='provider'?binding.provider===id:kind==='agent'?binding.agent===id:binding.environment===id)&&((model===null||model===undefined)||binding.model===model.model_name)&&this.world.places.rowOf(thread)?.deleted!==true).map(([thread])=>thread);
    const visible=(threads:readonly string[])=>threads.filter((thread)=>this.world.places.access(person,thread).permissions.includes('thread.view')).map((thread)=>({object:{kind:'thread' as const,id:thread},name:this.world.places.rowOf(thread)?.name??'',relation:'binding' as const,until:null}));
    interface Work {thread:string;at:string|null}
    const running:Work[]=[],waiting:Work[]=[];
    for(const thread of ids) {
      const snapshot=this.world.threads.snapshot(thread);
      if(!isRecord(snapshot))continue;
      for(const turn of Array.isArray(snapshot.turns)?snapshot.turns:[])if(isRecord(turn)&&(turn.state==='running'||turn.state==='starting'))running.push({thread,at:typeof turn.started_at==='string'?turn.started_at:null});
      for(const input of Array.isArray(snapshot.inputs)?snapshot.inputs:[])if(isRecord(input)&&(input.state==='pending'||input.state==='delivering'))waiting.push({thread,at:typeof input.created_at==='string'?input.created_at:null});
    }
    const group=(rows:Work[]):ActivityGroup=>{
      const threads=[...new Set(rows.map((row)=>row.thread))],names=visible(threads);
      return {count:rows.length,threads:threads.length,visible:names,hidden:threads.length-names.length,since:rows.flatMap((row)=>row.at===null?[]:[row.at]).sort()[0]??null};
    };
    const names=visible(ids);
    const target={object:{kind:kind as 'provider'|'agent'|'environment',id},model_id:modelId};
    return {target,name:model===null||model===undefined?value.name:model.display_name||model.model_name,stopping:false,
      facts:{subject:{kind:'resource',object:target.object},as_of:new Date().toISOString(),dependents_complete:true,uses:null,dependents:ids.length===0?[]:[{relation:'binding',count:ids.length,visible:names,hidden:ids.length-names.length,until:null}]},
      activity:{running:group(running),waiting:group(waiting),stopped:{count:0,threads:0,visible:[],hidden:0,since:null}}};
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    if (method==='POST' && path==='/resources/library') {
      const targets=Array.isArray(body.targets)?body.targets.filter(isRecord):[];
      return {status:200,body:targets.map((target)=>{
        const object=isRecord(target.object)?target.object:{};
        const kind=String(object.kind);
        const unit=kind==='provider'||kind==='agent'||kind==='environment'?'turn':kind==='mcp_server'?'mcp_call':kind==='skill'?'skill_invocation':kind==='host'||kind==='template'?'environment_made':null;
        return {target,facts:{subject:{kind:'resource',object},as_of:new Date().toISOString(),dependents:[],dependents_complete:false,uses:unit===null?null:{kind:unit,coverage:{state:'unrecorded'},all_time:null,last_30_days:null,people:null,people_30_days:null,last_used_at:null}},threads:null};
      })};
    }
    if(method==='POST' && path==='/resources/library/people')return {status:200,body:{items:[],hidden:0,next_cursor:null}};
    const updateRoute = /^(?:\/api\/v1)?\/(machines|hosts)\/([^/]+)\/update$/.exec(path);
    if (method === 'POST' && updateRoute !== null) {
      const id = updateRoute[2] ?? '';
      const node = updateRoute[1] === 'hosts';
      const machine = node ? undefined : this.findMachine(person,id);
      if (node) this.mustManageHost(person,this.findHost(id));
      const held = this.componentUpdates.get(id);
      if (held === undefined) throw new Refusal(409,'conflict.exists','No update is available.');
      const disabled = machine?.policy['update.auto'] === false;
      const next:ComponentUpdate = {...held,status:disabled?'refused':'updating',detail:disabled?`update.auto is off on this machine. Run \`${held.command}\` on the machine to update it locally.`:null};
      this.componentUpdates.set(id,next);
      this.world.users.tellAll({what:'resources',kind:node?'host':'machine',id});
      return {status:200,body:next};
    }
    const route = `${method} ${path}`;
    if(route==='POST /resources/impact')return {status:200,body:this.impact(person,body)};
    let found: RegExpExecArray | null;

    // machines
    if(route==='GET /machines/enrollment-options') return {status:200,body:{secure:true,public_url:'https://orch.example'}};
    if ((found = /^GET \/machines\/enrollment-status\/([^/]+)$/.exec(route))) return { status: 200, body: this.enrollmentProgress(person, found[1] ?? '') };
    if (route === 'GET /machines') return { status: 200, body: this.machines.filter((record) => record.owner === person.me.username).map((record) => this.machine(record)) };
    if (route === 'POST /machines/enrollment-tokens') {
      const roles = Array.isArray(body.roles) ? body.roles.filter((role): role is string => typeof role === 'string') : ['agent', 'environment'];
      if (roles.length === 0 || roles.some((role) => !['environment','agent','provider'].includes(role))) this.invalid('roles', 'choose connector roles; Node uses orch-node');
      const delegate = Array.isArray(body.delegate) ? body.delegate.filter((section): section is string => typeof section === 'string') : [];
      return { status: 200, body: this.enrollmentToken(person, undefined, { kind: 'connector', roles, delegate, dedicated: body.dedicated === true }) };
    }
    if ((found = /^GET \/machines\/([^/]+)\/environment-data$/.exec(route))) {
      const id = found[1] ?? '';
      const host = this.hosts.map((record) => this.host(person, record)).find((host) => host.machine.id === id);
      if (host !== undefined) {
        if (!host.access.permissions.some((permission) => permission === 'host.view' || permission === 'host.use' || permission === 'host.manage')) throw new Refusal(404, 'not_found', 'the machine is not readable');
        // This fixture's declared host driver makes volumes; it seeds no allowed existing host roots.
        const page: MachineEnvironmentData = { machine_id: id, roots: [], direct: null, drivers: { docker: { made_by_orch: 'volume', existing_directory: true, reason: null } } };
        return { status: 200, body: page };
      }
      const record = this.findMachine(person, id);
      const paths = record.policy['environment.roots'];
      const page: MachineEnvironmentData = { machine_id: id, roots: (Array.isArray(paths) ? paths.filter((path): path is string => typeof path === 'string') : []).map((path) => ({ path, uses: [] })), direct: directoryStorage(record), drivers: {} };
      return { status: 200, body: page };
    }
    if ((found = /^GET \/machines\/([^/]+)\/mounts$/.exec(route))) {
      const id = found[1] ?? '';
      const host = this.hosts.map((record) => this.host(person, record)).find((host) => host.machine.id === id);
      if (host === undefined) this.findMachine(person, id);
      else if (!host.access.permissions.includes('host.view')) throw new Refusal(404, 'not_found', 'the machine is not readable');
      // This world seeds no mount locations. Full location behavior is covered by the real API tests.
      return { status: 200, body: { locations: [], available: [] } };
    }
    // A token for a machine that exists, which enrolls it again: only its owner may ask.
    if ((found = /^POST \/machines\/([^/]+)\/enrollment-tokens$/.exec(route))) return { status: 200, body: this.enrollmentToken(person, this.findMachine(person, found[1] ?? '')) };
    if ((found = /^(GET|PATCH) \/machines\/([^/]+)$/.exec(route))) {
      const record = this.findMachine(person, found[2] ?? '');
      if (found[1] === 'PATCH') {
        const name = asText(body.name).trim();
        if (name === '') this.invalid('name', 'give it a name');
        record.name = name;
      }
      return { status: 200, body: this.machine(record) };
    }
    if((found=/^PUT \/machines\/([^/]+)\/roles$/.exec(route))) {
      const record=this.findMachine(person,found[1] ?? '');
      if(!record.online)throw new Refusal(503,'source.offline','The machine is offline.');
      const roles=Array.isArray(body.roles)?body.roles.filter((role):role is string=>typeof role==='string'):[];
      if(roles.length===0 || new Set(roles).size!==roles.length || roles.some((role)=>!['environment','agent','provider'].includes(role)))this.invalid('roles','Choose Environment, Agent or Provider; Node uses orch-node.');
      if(!this.machineKeyHanded(record,'machine.roles') || record.policy['machine.roles']!==true)this.invalid('roles','machine.roles (machine section): the machine operator keeps role changes here or has switched them off.');
      for(const [role,used] of [['environment',this.environments.some((entry)=>entry.machine===record.id)],['agent',this.agents.some((entry)=>entry.machine===record.id)],['provider',this.providers.some((entry)=>entry.machine===record.id)]] as const) {
        if(used && !roles.includes(role))this.invalid('roles',`Remove its registered ${role} resources before turning off this role.`);
      }
      record.roles=roles;
      this.world.users.tellAll({what:'resources',kind:'machine',id:record.id});
      return {status:200,body:{machine_view:this.machine(record),requested_roles:roles,reconnecting:false}};
    }
    if ((found = /^POST \/machines\/([^/]+)\/(revoke|probe)$/.exec(route))) {
      const record = this.findMachine(person, found[1] ?? '');
      if (found[2] === 'revoke') {
        record.status = 'revoked';
        record.online = false;
      } else {
        if (!record.online) throw new Refusal(503, 'source.offline', 'the machine is offline');
        record.lastSeen = inMs(0);
      }
      return { status: 200, body: this.machine(record) };
    }
    if ((found = /^(GET|PATCH) \/machines\/([^/]+)\/policy$/.exec(route))) {
      const record = this.findMachine(person, found[2] ?? '');
      return { status: 200, body: found[1] === 'GET' ? this.policy(person, record) : this.changePolicy(person, record, body) };
    }
    if ((found = /^GET \/machines\/([^/]+)\/executors$/.exec(route))) {
      this.findMachine(person, found[1] ?? '');
      return { status: 200, body: MACHINE_EXECUTORS };
    }
    if((found=/^(GET|POST) \/machines\/([^/]+)\/programs$/.exec(route))) {
      const record=this.findMachine(person,found[2] ?? '');
      return {status:200,body:found[1]==='GET'?this.machinePrograms(person,record):{...this.changeMachinePrograms(person,record,body),machine_view:this.machine(record)}};
    }
    if ((found = /^POST \/machines\/([^/]+)\/agents$/.exec(route))) {
      const machine = this.findMachine(person, found[1] ?? '');
      this.needSettings(person, asText(body.home_id));
      const kind = asText(body.harness_kind);
      if (!machine.harnesses.some((harness) => harness.kind === kind) && !machine.programs?.some((entry)=>entry.kind==='agent' && entry.name===kind && entry.ready)) this.invalid('harness_kind', 'that harness is not installed on the machine');
      const record: AgentRecord = {
        id: this.next(KIND.agent),
        name: asText(body.name).trim() || kind,
        harness: kind,
        owner: person.me.username,
        home: asText(body.home_id),
        machine: machine.id,
        disabled: false,
        revision: 1,
        profile: isRecord(body.profile) ? body.profile : { env: {}, config: {}, harness_version: null },
        descriptor: descriptor(kind, kind === 'codex' ? 'Codex' : kind, machine.harnesses.find((harness) => harness.kind === kind)?.version ?? '1.0.0'),
      };
      this.agents.push(record);
      return { status: 200, body: this.agent(person, record) };
    }
    if ((found = /^POST \/machines\/([^/]+)\/providers$/.exec(route))) {
      const machine = this.findMachine(person, found[1] ?? '');
      return {status:200,body:this.makeProvider(person,{...body,runtime_machine_id:machine.id,keeper_machine_id:machine.id})};
    }
    if ((found = /^POST \/machines\/([^/]+)\/environments$/.exec(route))) {
      const machine = this.findMachine(person, found[1] ?? '');
      this.needSettings(person, asText(body.home_id));
      const name = asText(body.name).trim();
      const selected = dataSources(body.data_sources);
      const id = this.next(KIND.environment);
      const root = selected === null ? asText(body.root).trim() : selected.workspace.kind === 'existing_directory' ? selected.workspace.path : `/orch-data/${id}/workspace`;
      if (name === '') this.invalid('name', 'give it a name');
      if (root === '') this.invalid('root', 'give the directory');
      const roots = Array.isArray(machine.policy['environment.roots']) ? (machine.policy['environment.roots'] as string[]) : [];
      if ((selected === null || selected.workspace.kind === 'existing_directory') && !roots.some((allowed) => root === allowed || root.startsWith(`${allowed.replace(/[\\/]$/, '')}/`) || root.startsWith(`${allowed.replace(/[\\/]$/, '')}\\`))) {
        this.invalid('root', "the machine's local policy does not let an environment be there");
      }
      const record: EnvironmentRecord = {
        id,
        name,
        owner: person.me.username,
        home: asText(body.home_id),
        machine: machine.id,
        host: null,
        template: null,
        status: machine.online ? 'running' : 'offline',
        root,
        remoteWrite: body.remote_write === true,
        terminal: body.terminal === true,
        revision: 1,
        uses: body.access === 'only_me' ? [] : ['dora'],
        manages: [],
        updates: 0,
        removeAt: null,
        ...(selected === null ? {} : { config: { 'env.data_sources': selected } }),
      };
      this.environments.push(record);
      return { status: 200, body: this.environment(person, record) };
    }

    // providers
    if (route === 'GET /providers') {
      const rows = this.providerList(person, query);
      const after = Number(query.get('cursor') ?? '0');
      const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? '50')));
      const offset = Number.isInteger(after) && after >= 0 ? after : 0;
      const items = rows.slice(offset, offset + limit);
      return { status: 200, body: { items, next_cursor: offset + limit < rows.length ? String(offset + limit) : null } };
    }
    if (route === 'POST /providers') return { status: 200, body: this.makeProvider(person, body) };
    if (route === 'GET /provider-setup') {
      const provider=query.get('provider_id');
      if (provider === null) this.needSettings(person, query.get('org_id') ?? ACME.org);
      else this.mustManageProvider(person,this.findProvider(person,provider));
      return { status: 200, body: this.providerSetup() };
    }
    if ((found = /^PUT \/providers\/([^/]+)\/sources$/.exec(route))) {
      const record=this.findProvider(person,found[1] ?? '');
      this.mustManageProvider(person,record);this.revisionOk(body,record.revision);
      this.chooseSources(person,record,body);record.revision+=1;
      return {status:200,body:this.provider(person,record)};
    }
    if ((found = /^GET \/providers\/([^/]+)\/plan$/.exec(route))) {
      const record=this.findProvider(person,found[1] ?? '');
      const day=new Date().toISOString().slice(0,10);
      const bucket=query.get('bucket');
      const all=planReply(record.planUsage,record.facts.windows,query.get('from') ?? day,query.get('to') ?? day,bucket==='hour' || bucket==='week' || bucket==='month' ? bucket : 'day');
      const byId=new Map(all.map((row)=>[row.window_id,row]));
      const ordered=record.facts.windows.toSorted((a,b)=>Number(b.is_active===true)-Number(a.is_active===true) || (a.duration_minutes ?? Infinity)-(b.duration_minutes ?? Infinity) || a.id.localeCompare(b.id)).flatMap((window)=>{const row=byId.get(window.id);return row===undefined ? [] : [row];});
      const limit=Math.max(1,Math.min(200,Number(query.get('limit') ?? 50)));
      const cursor=query.get('cursor');const after=cursor===null ? 0 : Number(cursor.split(':')[1]);
      if(cursor!==null && (cursor.split(':')[0]!==String(record.revision) || !Number.isInteger(after)))throw new Refusal(400,'request.invalid','the plan changed; reopen its first page');
      return {status:200,body:{items:ordered.slice(after,after+limit),next_cursor:after+limit<ordered.length ? `${record.revision}:${after+limit}` : null}};
    }
    if ((found = /^(GET|PATCH|DELETE) \/providers\/([^/]+)$/.exec(route))) {
      const record = this.findProvider(person, found[2] ?? '');
      if (found[1] === 'GET') return { status: 200, body: this.provider(person, record,true) };
      if (found[1] === 'PATCH') return { status: 200, body: this.changeProvider(person, record, body) };
      this.mustManageProvider(person, record);
      if (this.threadsOn(record.id).length > 0) throw new Refusal(409, 'resource.in_use', 'threads are bound to this provider');
      this.providers = this.providers.filter((candidate) => candidate.id !== record.id);
      return { status: 204 };
    }
    if ((found = /^POST \/providers\/([^/]+)\/(disable|enable|refresh)$/.exec(route))) {
      const record = this.findProvider(person, found[1] ?? '');
      if (found[2] !== 'refresh') this.mustManageProvider(person, record);
      if (found[2] === 'disable') record.status = 'disabled';
      if (found[2] === 'enable') {
        record.status = 'needs_setup';
        this.recomputeStatus(record);
      }
      if (found[2] === 'refresh') record.facts.plan = record.facts.plan ?? null;
      record.revision += 1;
      return { status: 200, body: this.provider(person, record,true) };
    }
    if ((found = /^PUT \/providers\/([^/]+)\/key$/.exec(route))) {
      const record = this.findProvider(person, found[1] ?? '');
      this.mustManageProvider(person, record);
      if (asText(body.api_key) === '') this.invalid('api_key', 'give the key');
      record.keySet = true;
      record.keyHeldByOrch = true;
      if(record.sources?.models.by!=='none' && this.provider(person,record).facts_state.models!=='unsupported'){
        record.facts.models=this.suggestions();
        record.facts.field_times.models=new Date().toISOString();
      }
      this.recomputeStatus(record);
      return { status: 200,body:this.provider(person,record) };
    }
    if ((found = /^PUT \/providers\/([^/]+)\/secrets\/([^/]+)$/.exec(route))) {
      const record = this.findProvider(person, found[1] ?? '');
      this.mustManageProvider(person, record);
      const name = decodeURIComponent(found[2] ?? '');
      if (!record.secrets.includes(name)) record.secrets.push(name);
      return { status: 204 };
    }
    if ((found = /^(GET|POST|PATCH|DELETE) \/providers\/([^/]+)\/models(\/.*)?$/.exec(route))) {
      const record = this.findProvider(person, found[2] ?? '');
      const answer = this.models(person, record, found[1] ?? '', found[3] ?? '', body);
      if (answer !== null) return answer;
    }
    if ((found = /^GET \/providers\/([^/]+)\/threads$/.exec(route))) return { status: 200, body: this.providerThreads(this.findProvider(person, found[1] ?? '')) };
    if ((found = /^GET \/providers\/([^/]+)\/settings$/.exec(route))) {
      this.findProvider(person, found[1] ?? '');
      return {
        status: 200,
        body: this.settingsOf([
          ['runtime.idle_stop_minutes', 1440],
          ['runtime.template_id', fixtureId(KIND.template, 2)],
          ['runtime.host_id', null],
          ['runtime.update', 'manual'],
          ['runtime.renew_days', 7],
        ]),
      };
    }

    // agent profiles
    if (route === 'GET /harness-executors') return { status: 200, body: EXECUTORS };
    if (route === 'GET /agents') return { status: 200, body: this.agents.map((record) => this.agent(person, record)) };
    if (route === 'GET /harness-kinds') {
      return {
        status: 200,
        body: [
          { kind: 'claude-code', title: 'Claude Code', builds: [], descriptor: {} },
          { kind: 'codex', title: 'Codex', builds: [{ version: '0.162.1', is_default: true }, { version: '0.161.0', is_default: false }], descriptor: descriptor('codex', 'Codex', '0.162.1') },
          { kind: 'fake', title: 'Fake', builds: [{ version: '1.0.0', is_default: true }], descriptor: descriptor('fake', 'Fake', '1.0.0') },
        ],
      };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/agents$/.exec(route))) {
      this.needSettings(person, found[1] ?? '');
      const record: AgentRecord = { id: this.next(KIND.agent), name: asText(body.name).trim() || 'Agent', harness: asText(body.harness_kind), owner: person.me.username, home: found[1] ?? ACME.org, machine: null, disabled: false, revision: 1, profile: isRecord(body.profile) ? body.profile : {}, descriptor: {} };
      this.agents.push(record);
      return { status: 200, body: this.agent(person, record) };
    }
    if ((found = /^(GET|PATCH|DELETE) \/agents\/([^/]+)$/.exec(route))) {
      const record = this.findAgent(found[2] ?? '');
      if (found[1] === 'GET') return { status: 200, body: this.agent(person, record) };
      if (record.owner !== person.me.username && !(record.owner === null && this.administers(person))) this.refuseOn('agent', record.id, record.name, record.owner, 'agent.manage');
      if (found[1] === 'DELETE') {
        if ([...this.bindings.values()].some((binding) => binding.agent === record.id)) throw new Refusal(409, 'resource.in_use', 'threads are bound to this profile');
        this.agents = this.agents.filter((candidate) => candidate.id !== record.id);
        return { status: 204 };
      }
      this.revisionOk(body, record.revision);
      if (typeof body.name === 'string' && body.name.trim() !== '') record.name = body.name.trim();
      if (typeof body.disabled === 'boolean') record.disabled = body.disabled;
      if (isRecord(body.profile)) record.profile = body.profile;
      record.revision += 1;
      return { status: 200, body: this.agent(person, record) };
    }

    // environments
    if (route === 'GET /environments') {
      // `holding=manage` lists only those the caller looks after, which is what a place to start a thread in is chosen among.
      const managing = query.get('holding') === 'manage';
      return {
        status: 200,
        body: this.environments
          .filter((record) => record.status !== 'removed')
          .filter((record) => record.owner === person.me.username || record.uses.includes(person.me.username) || this.administers(person))
          .map((record) => this.environment(person, record))
          .filter((environment) => !managing || environment.access.permissions.includes('environment.manage')),
      };
    }
    if (route === 'POST /environments') {
      const home = asText(body.home_node_id);
      if (home === '') this.invalid('home_node_id', 'name the place it is made in');
      this.needSettings(person, home);
      const template = this.templates.find((candidate) => candidate.id === (asText(body.template_id) || fixtureId(KIND.template, 1)));
      if (template?.kind !== 'environment') this.invalid('template_id', 'choose a template for environments');
      if (!template.enabled) this.invalid('template_id', 'this template is switched off');
      const host = typeof body.host_id === 'string' ? this.findHost(body.host_id) : (this.hosts.find((candidate) => candidate.schedulable) ?? null);
      if (!host?.schedulable) this.invalid('host_id', 'this host takes no new environments');
      const name = asText(body.name).trim() || 'Main';
      if (this.environments.some((environment) => environment.owner === person.me.username && environment.name === name)) this.invalid('name', 'you already have an environment with this name');
      const record: EnvironmentRecord = { id: this.next(KIND.environment), name, owner: person.me.username, home, machine: null, host: host.id, template: template.id, status: 'provisioning', root: '/workspace', remoteWrite: false, terminal: false, revision: 1, uses: [], manages: [], updates: 0, removeAt: null, config: isRecord(body.config) ? body.config : {} };
      if (record.config?.['env.lifetime'] === 'temporary' || record.config?.['env.lifetime'] === 'thread') record.temporary = { expiresAt: inMs(DAY), hours: 24, thread: null };
      const selected = dataSources(body.data_sources);
      if (selected !== null) record.config = { ...record.config, 'env.data_sources': selected };
      template.units += 1;
      this.environments.push(record);
      this.finishLater(record.id, 'running');
      return { status: 200, body: this.environment(person, record) };
    }
    if ((found = /^POST \/environments\/([^/]+)\/keep$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      this.mustManageEnvironment(person, record);
      record.config = { ...record.config, 'env.lifetime': 'lasting' };
      record.temporary = undefined; record.removeAt = null; record.revision += 1;
      return { status: 200, body: this.environment(person, record) };
    }
    if ((found = /^POST \/environments\/([^/]+)\/(start|stop|keep-awake)$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      this.mustManageEnvironment(person, record);
      if (record.template === null) this.invalid('id', 'a registered directory is not started or stopped from here');
      if (found[2] === 'start') {
        if (record.status !== 'stopped' && record.status !== 'none' && record.status !== 'failed') this.invalid('id', 'it cannot be started in its current state');
        record.status = 'provisioning';
        record.revision += 1;
        this.finishLater(record.id, 'running');
        const now = new Date().toISOString();
        const job: Job = { id: randomUUID(), kind: 'environment_provision', state: 'running', environment_id: record.id,
          progress: { bytes_done: 0, bytes_total: null, files_done: 0, files_total: null, rate_bps: null, eta_seconds: null, percent: 40, phase: 'files', phases: [], message: null },
          created_at: now, created_by: userRef(person.me), started_at: now, finished_at: null, error: null, log: null, result: null, batch_id: null, node_id: null, provider_id: null };
        this.jobs.set(job.id, job);
        return { status: 202, body: job };
      }
      if (found[2] === 'stop') {
        if (record.status !== 'running') this.invalid('id', 'it is not running');
        record.status = 'stopped';
        record.revision += 1;
        const runtime = this.runtimes.find((candidate) => candidate.environment === record.id);
        if (runtime !== undefined) runtime.state = 'stopped';
      }
      return { status: 200, body: this.environment(person, record) };
    }
    if ((found = /^(GET|PATCH|DELETE) \/environments\/([^/]+)$/.exec(route))) {
      const record = this.findEnvironment(found[2] ?? '');
      if (found[1] === 'GET') return { status: 200, body: this.environment(person, record) };
      this.mustManageEnvironment(person, record);
      if (found[1] === 'DELETE') {
        if (query.get('confirm') !== record.name) this.invalid('confirm', 'name the environment to remove it');
        this.environments = this.environments.filter((candidate) => candidate.id !== record.id);
        return { status: 204 };
      }
      this.revisionOk(body, record.revision);
      if (typeof body.name === 'string' && body.name.trim() !== '') record.name = body.name.trim();
      if (typeof body.remote_write === 'boolean') record.remoteWrite = body.remote_write;
      if (typeof body.terminal === 'boolean') record.terminal = body.terminal;
      if (typeof body.config === 'object' && body.config !== null) record.config = { ...(record.config ?? {}), ...(body.config as Record<string, unknown>) };
      const selected = dataSources(body.data_sources);
      if (selected !== null) record.config = { ...record.config, 'env.data_sources': selected };
      record.revision += 1;
      return { status: 200, body: this.environment(person, record) };
    }
    if ((found = /^POST \/environments\/([^/]+)\/(disable|enable)$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      this.mustManageEnvironment(person, record);
      record.status = found[2] === 'disable' ? 'disabled' : record.machine === null ? 'stopped' : 'running';
      record.revision += 1;
      return { status: 200, body: this.environment(person, record) };
    }
    if ((found = /^GET \/environments\/([^/]+)\/settings$/.exec(route))) {
      this.findEnvironment(found[1] ?? '');
      return { status: 200, body: this.settingsOf([['env.update', 'manual'], ['env.idle_stop_minutes', 1440], ['env.template_id', fixtureId(KIND.template, 1)], ['exec.orch_tools', true], ['workspace.index', 'full']]) };
    }
    if ((found = /^GET \/environments\/([^/]+)\/mcp$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      return { status: 200, body: this.mcpServers.get(record.id) ?? { servers: [], applies: 'next_turn', found: [], import: { allowed: false, reason: 'policy' } } };
    }
    if ((found = /^POST \/environments\/([^/]+)\/mcp\/servers\/([^/]+)\/restart$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      this.mustManageEnvironment(person,record);
      const name = decodeURIComponent(found[2] ?? '');
      const server = this.mcpServers.get(record.id)?.servers.find((server) => server.server.name === name);
      if (server === undefined) throw new Refusal(404,'not_found','there is no such server');
      server.state = 'running'; server.reason = null;
      const now = new Date().toISOString();
      const job: Job = { id: randomUUID(), kind: 'mcp_restart', node_id: null, environment_id: record.id, provider_id: null, batch_id: null, state: 'succeeded', result: { name }, error: null, log: null, created_by: null, created_at: now, started_at: now, finished_at: now,
        progress: { phase: 'record', phases: [{ name: 'restart', weight: 70, state: 'done' }, { name: 'agents', weight: 20, state: 'done' }, { name: 'record', weight: 10, state: 'done' }], percent: 100, files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, rate_bps: null, eta_seconds: null, message: null } };
      this.jobs.set(job.id,job);
      return { status: 202, body: job };
    }
    if ((found = /^GET \/environments\/([^/]+)\/agent-reach$/.exec(route))) {
      const record = this.findEnvironment(found[1] ?? '');
      const environment = this.environment(person, record);
      const permissions = environment.access.permissions;
      const visible = this.threadsIn(record.id).filter((entry) => {
        const row = this.world.places.rowOf(entry.thread);
        return row !== undefined && this.world.places.visibleTo(person, row);
      });
      const whole = permissions.includes('environment.view');
      if (!whole && visible.length === 0) throw new Refusal(403, 'authz.denied', 'the environment is not readable');
      const manages = whole && permissions.includes('environment.manage');
      const reach: AgentReachOverview = {
        ...(whole ? { home_path: this.world.places.pathFor(person, environment.home.id) } : {}),
        network: whole ? environment.network : { ...environment.network, networks: [], proxy_url: null, always: [], allowed_hosts: [], sources: [], last_denied: null },
        secrets: manages ? { names: [], source: environment.home } : { names: null, source: null },
        mounts: whole && permissions.includes('files.view') ? environment.mounts.filter((mount) => mount.state === 'attached' && mount.path !== null).map((mount) => ({ mount: mount.mount, path: mount.path ?? '', mode: mount.mode })) : null,
        credentials: visible.map((entry) => ({ thread: this.placeRef(entry.thread, 'thread'), placement: this.bindings.get(entry.thread)?.arrangement ?? 'separated', expires_at: null })),
        browser_profiles: whole && permissions.includes('browser.view') ? this.world.browsers.profileNames(record.id) : null,
      };
      return { status: 200, body: reach };
    }

    // hosts
    if ((found = /^(GET|PUT|DELETE) \/hosts\/([^/]+)\/settings(?:\/([^/]+))?$/.exec(route))) {
      const record = this.findHost(found[2] ?? '');
      const access = this.held(person, 'host', record.owner, record.uses, record.manages).permissions;
      if (!access.includes('host.view')) this.refuseOn('host', record.id, record.name, record.owner, 'host.view');
      const key = 'cleanup.unused_keep_days';
      if (found[1] !== 'GET') {
        this.mustManageHost(person, record);
        if (found[3] !== key) throw new Refusal(404, 'not_found', 'the host setting does not exist');
        if (body.locked === true) this.invalid('locked', 'the host cannot lock its own value');
        const expected = found[1] === 'DELETE' ? query.get('expected_revision') : body.expected_revision;
        if (expected !== null && expected !== undefined) this.revisionOk({ expected_revision: Number(expected) }, record.cleanupDays === null || record.cleanupDays === undefined ? 0 : record.revision);
        if (found[1] === 'PUT' && (typeof body.value !== 'number' || !Number.isInteger(body.value) || body.value < 0)) this.invalid('value', 'a nonnegative whole number is required');
        const next = found[1] === 'DELETE' ? null : body.value as number;
        if ((record.cleanupDays ?? null) !== next) { record.cleanupDays = next; record.revision += 1; }
      }
      const own = record.cleanupDays ?? null;
      const setting: ResolvedSetting = { key, value: own ?? 7, own: own === null ? null : {value:own,revision:record.revision,locked:false}, source:{scope:own === null ? 'default' : 'host_own',node:null}, locked_by:null, writable:access.includes('host.manage'),parts:[] };
      return {status:200,body:found[1] === 'GET' ? [setting] : setting};
    }
    if (route === 'GET /hosts') return { status: 200, body: this.hosts.map((record) => this.host(person, record)) };
    if (route === 'POST /hosts/enrollment-tokens') {
      if (body.home_id !== null) this.needSettings(person, asText(body.home_id));
      else if (!this.administers(person)) this.refuseOn('host', fixtureId(KIND.host, 0), 'the system', 'ada', 'system.admin');
      return { status: 200, body: this.enrollmentToken(person, undefined, { kind: 'node', roles: ['node'], delegate: body.delegate_policy === true ? ['*'] : [], dedicated: false, home: asText(body.home_id) || null }) };
    }
    if ((found = /^GET \/hosts\/([^/]+)\/runtimes$/.exec(route))) {
      const record = this.findHost(found[1] ?? '');
      this.mustManageHost(person, record);
      return { status: 200, body: this.runtimes.filter((runtime) => runtime.host === record.id).map((runtime) => this.runtime(runtime)) };
    }
    if ((found = /^POST \/hosts\/([^/]+)\/runtimes\/([^/]+)\/stop$/.exec(route))) {
      const record = this.findHost(found[1] ?? '');
      this.mustManageHost(person, record);
      const runtime = this.runtimes.find((candidate) => candidate.id === found?.[2] && candidate.host === record.id);
      if (runtime === undefined) throw new Refusal(404, 'not_found', 'there is no such runtime on this host');
      runtime.state = 'stopped';
      const environment = this.environments.find((candidate) => candidate.id === runtime.environment);
      if (environment?.status === 'running') environment.status = 'stopped';
      return { status: 200, body: this.runtime(runtime) };
    }
    if ((found = /^DELETE \/hosts\/([^/]+)\/runtimes\/([^/]+)$/.exec(route))) {
      const record = this.findHost(found[1] ?? '');
      this.mustManageHost(person, record);
      const runtime = this.runtimes.find((candidate) => candidate.id === found?.[2] && candidate.host === record.id);
      if (runtime === undefined) throw new Refusal(404, 'not_found', 'there is no such runtime on this host');
      if (query.get('confirm') !== runtime.unitName) this.invalid('confirm', 'name the unit to remove it');
      const environment = this.environments.find((candidate) => candidate.id === runtime.environment);
      if (environment !== undefined) {
        if (this.threadsIn(environment.id).length > 0 && environment.status === 'running') throw new Refusal(409, 'resource.in_use', 'a thread is running in this unit');
        environment.host = null;
        environment.status = 'none';
      }
      this.runtimes = this.runtimes.filter((candidate) => candidate.id !== runtime.id);
      return { status: 204 };
    }
    if ((found = /^(GET|PATCH|DELETE) \/hosts\/([^/]+)$/.exec(route))) {
      const record = this.findHost(found[2] ?? '');
      if (found[1] === 'DELETE') {
        this.mustManageHost(person, record);
        if (query.get('confirm') !== record.name) this.invalid('confirm', 'name the host to remove it');
        if (this.environments.some((environment) => environment.host === record.id) || this.runtimes.some((runtime) => runtime.host === record.id)) {
          throw new Refusal(409, 'resource.in_use', 'something still runs on this host', { units: this.runtimes.filter((runtime) => runtime.host === record.id).length });
        }
        this.hosts = this.hosts.filter((candidate) => candidate.id !== record.id);
        return { status: 204 };
      }
      if (found[1] === 'PATCH') {
        if (!this.held(person, 'host', record.owner, record.uses, record.manages).permissions.includes('host.manage')) this.refuseOn('host', record.id, record.name, record.owner, 'host.manage');
        this.revisionOk(body, record.revision);
        if (typeof body.name === 'string' && body.name.trim() !== '') record.name = body.name.trim();
        if (Array.isArray(body.labels)) record.labels = body.labels.filter((label): label is string => typeof label === 'string');
        if (typeof body.schedulable === 'boolean') record.schedulable = body.schedulable;
        if (body.when_full === null || body.when_full === 'warn' || body.when_full === 'block') record.whenFull = body.when_full;
        record.revision += 1;
      }
      return { status: 200, body: this.host(person, record) };
    }

    // templates, drivers, capabilities
    if (route === 'POST /templates/check') {
      if (!this.templateDriversReported || body.driver !== 'docker' && body.driver !== 'host') throw new Refusal(503, 'source.offline', "this driver's schema has not been reported");
      const kind = body.kind === 'provider' ? 'provider' : 'environment';
      const errors = templateProblems(kind, body.driver === 'host' ? hostTemplateSchema(kind) : driverSchema(kind, {}), body.spec);
      return { status: 200, body: { valid: errors.length === 0, errors } };
    }
    if (route === 'GET /templates') {
      const kind = query.get('kind');
      return { status: 200, body: this.templates.filter((record) => kind === null || record.kind === kind).map((record) => this.template(person, record)) };
    }
    if (route === 'POST /templates') {
      if (body.home_id !== null) this.needSettings(person, asText(body.home_id));
      const name = asText(body.name).trim();
      if (name === '') this.invalid('name', 'give it a name');
      if (!isRecord(body.spec)) this.invalid('spec', 'the spec must be an object');
      const kind = body.kind === 'provider' ? 'provider' : 'environment';
      const problems = templateProblems(kind, body.driver === 'host' ? hostTemplateSchema(kind) : driverSchema(kind, {}), body.spec);
      if (problems.length > 0) throw new Refusal(422, 'request.invalid', 'the configuration does not meet its schema', { errors: problems.map((problem) => ({ path: `spec${problem.pointer}`, message: problem.message })) });
      const record: TemplateRecord = { id: this.next(KIND.template), name, description: asText(body.description), kind: body.kind === 'provider' ? 'provider' : 'environment', driver: asText(body.driver) || 'docker', owner: person.me.username, home: body.home_id === null ? null : asText(body.home_id), spec: body.spec, enabled: true, revision: 1, units: 0, uses: [], manages: [] };
      this.templates.push(record);
      return { status: 200, body: this.template(person, record) };
    }
    if ((found = /^(GET|PATCH|DELETE) \/templates\/([^/]+)$/.exec(route))) {
      const record = this.findTemplate(found[2] ?? '');
      if (found[1] === 'GET') return { status: 200, body: this.template(person, record) };
      const may = record.owner === person.me.username || (record.owner === null && this.administers(person));
      if (!may) this.refuseOn('template', record.id, record.name, record.owner, 'template.manage');
      if (found[1] === 'DELETE') {
        if (record.units > 0) throw new Refusal(409, 'resource.in_use', 'environments were made from this template');
        this.templates = this.templates.filter((candidate) => candidate.id !== record.id);
        return { status: 204 };
      }
      this.revisionOk(body, record.revision);
      if (typeof body.name === 'string' && body.name.trim() !== '') record.name = body.name.trim();
      if (typeof body.description === 'string') record.description = body.description;
      if (typeof body.enabled === 'boolean') record.enabled = body.enabled;
      if (body.spec !== undefined) {
        if (!isRecord(body.spec)) this.invalid('spec', 'the spec must be an object');
        const problems = templateProblems(record.kind, record.driver === 'host' ? hostTemplateSchema(record.kind) : driverSchema(record.kind, {}), body.spec);
        if (problems.length > 0) throw new Refusal(422, 'request.invalid', 'the configuration does not meet its schema', { errors: problems.map((problem) => ({ path: `spec${problem.pointer}`, message: problem.message })) });
        record.spec = body.spec;
      }
      record.revision += 1;
      return { status: 200, body: this.template(person, record) };
    }
    if (route === 'GET /drivers') {
      if (!this.templateDriversReported) return { status: 200, body: [] };
      const environmentSchema = driverSchema('A Docker container for an environment', {
        workspace_mount: { type: 'string', default: '/workspace', description: 'Where the workspace volume is mounted' },
        needs: { type: 'object', description: 'What an environment declares it needs at least', properties: { cpu_millis: { type: 'integer' }, memory_mb: { type: 'integer' }, disk_gb: { type: 'integer' } } },
        exec: { type: 'object' },
      });
      const providerSchema = driverSchema("A Docker container for a provider's runtime", {});
      return {
        status: 200,
        body: [
          { name: 'docker', title: 'Docker', hosts: this.hosts.length, descriptor: { imports: [{ format: 'image', title: 'From an image' }], spec_schemas: { environment: environmentSchema, provider: providerSchema } } },
          { name: 'host', title: 'Host', hosts: this.hosts.length, descriptor: { spec_schemas: { environment: hostTemplateSchema('environment'), provider: hostTemplateSchema('provider') } } },
        ],
      };
    }
    if (route === 'GET /capabilities') {
      return {
        status: 200,
        body: [
          { key: 'ptrace', label: 'Debugger access', description: 'Lets processes in the environment inspect and trace each other, as a debugger does.', risk: 'medium', driver_patch: {}, default_decision: 'allow', default_approver_scope: 'host', builtin: true, revision: 1 },
          { key: 'privileged', label: 'Privileged container', description: 'Runs the container with every privilege of the host and almost no isolation.', risk: 'critical', driver_patch: {}, default_decision: 'deny', default_approver_scope: 'system', builtin: true, revision: 1 },
        ],
      };
    }
    return null;
  }

  /** The requests only the double has for resources. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/plan-usage': {
        const record=this.providers.find((record)=>record.id===body.id);
        if(record===undefined) return {status:404};
        record.planUsage=seedPlan(record.facts,Date.now());
        if(body.state==='stale') {
          const old=new Date(Date.now()-3*HOUR).toISOString();
          record.planUsage.at=old;record.planUsage.forecasts={};
          record.planUsage.facts.windows=record.planUsage.facts.windows.filter((window)=>window.id==='stale').map((window)=>({...window,label:'5-hour window',is_active:true}));
          record.planUsage.facts.field_times.windows=old;
          record.planUsage.rows=record.planUsage.rows.filter((row)=>row.window_id==='stale');
        }
        record.facts=record.planUsage.facts;
        this.world.trace.seedPlanResets(record,record.planUsage);
        record.revision+=1;
        this.world.users.tellAll({what:'resources',kind:'provider',id:record.id});
        return {status:204};
      }
      case '/host-driver-facts': {
        const host = this.findHost(asText(body.id));
        this.driverFacts.set(host.id, { drivers: body.drivers as Host['drivers'], storages: body.storages as Host['storages'],
          ...(body.capacity === undefined ? {} : { capacity: body.capacity as Host['capacity'] }) });
        return { status: 200, body: { id: host.id } };
      }
      case '/provider-program-report': {
        const record = this.providers.find((candidate) => candidate.id === asText(body.id));
        if (record === undefined) return { status: 404, body: { detail: 'no such provider observation' } };
        const agent = this.agents.find((candidate) => candidate.id === record.agent);
        const machine = this.machines.find((candidate) => candidate.id === (record.machine ?? agent?.machine ?? record.keyMachine));
        if (agent === undefined || machine === undefined) return { status: 409, body: { detail: 'this fixture needs a native profile and machine' } };
        const previous = this.machines.find((candidate) => candidate.owner === record.owner && candidate.id !== machine.id && candidate.status === 'active');
        if (previous === undefined) return { status: 409, body: { detail: 'this fixture needs a previous machine' } };
        const state = asText(body.state);
        if (!['none', 'error', 'online', 'unknown'].includes(state)) return { status: 422, body: { detail: 'unknown program observation' } };
        const detail = state === 'unknown' ? null : asText(body.reason);
        record.harnessReports = [{ agent: this.agentRef(agent), machine: this.machineRef(machine), status: state === 'error' ? 'error' : 'online',
          status_detail: state === 'error' ? detail : null, harness_version: '0.162.1', sessions_open: 0, last_active_at: '2026-10-27T12:00:00Z',
          ...(state === 'none' ? {} : { last_error: { at: '2026-10-26T12:00:00Z', detail: asText(body.reason) } }),
        }, { agent: this.agentRef(agent), machine: this.machineRef(previous), status: 'offline', status_detail: null, harness_version: '0.162.1', sessions_open: 0, last_active_at: '2026-10-25T12:00:00Z',
          ...(state === 'none' ? {} : { last_error: { at: '2026-10-27T12:00:00Z', detail } }),
        }];
        record.revision += 1;
        this.world.users.tellAll({ what: 'resources', kind: 'provider', id: record.id });
        return { status: 200, body: { revision: record.revision, source: this.machineRef(previous) } };
      }
      case '/temporary-warning': {
        const record = this.findEnvironment(asText(body.id));
        if (record.temporary === undefined) return { status: 409, body: { detail: 'this environment is lasting' } };
        record.temporary.expiresAt = inMs(HOUR); record.revision += 1;
        const data = { environment_id: record.id, subject: record.name, reason: 'temporary', expires_at: record.temporary.expiresAt };
        this.world.notifications.add(record.owner, { kind: 'environment.attention', title: `${record.name} will be removed`, url: `/resources/environments/${record.id}`, data });
        this.world.users.tellAll({ what: 'resources', kind: 'environment', id: record.id });
        return { status: 200, body: data };
      }
      case '/template-hosts': {
        if (typeof body.reported === 'boolean') this.templateDriversReported = body.reported;
        if (body.state === 'none') this.hosts = [];
        else if (body.state === 'offline' || body.state === 'online') {
          for (const host of this.hosts) {
            host.online = body.state === 'online';
            host.offlineSince = host.online || typeof body.since !== 'string' ? null : body.since;
          }
        }
        return { status: 200, body: { hosts: this.hosts.length } };
      }
      case '/component-update': {
        const machine = this.machines.find((record)=>record.id===asText(body.machine) || record.name===asText(body.machine));
        const host = this.hosts.find((record)=>record.id===asText(body.host) || record.name===asText(body.host));
        const id = machine?.id ?? host?.id;
        if (id === undefined) return {status:404,body:{detail:'no such physical program'}};
        const previous = this.componentUpdates.get(id);
        const recommended = [asText(body.recommended), previous?.recommended_version].find((version) => typeof version === 'string' && version !== '') ?? '0.2.0';
        const reconnect = body.reconnect === true;
        const running = reconnect ? recommended : [asText(body.running), machine?.connector, previous?.running_version].find((version) => typeof version === 'string' && version !== '') ?? '0.1.0';
        if (machine !== undefined) {machine.connector=running;machine.online=true;}
        const next:ComponentUpdate = {running_version:running,recommended_version:recommended,behind:running!==recommended,status:'current',detail:null,command:host===undefined?'orch-connector update':'orch-node update'};
        this.componentUpdates.set(id,next);
        this.world.users.tellAll({what:'resources',kind:host===undefined?'machine':'host',id});
        return {status:200,body:{id,...next}};
      }
      case '/enroll': {
        const record = this.enroll(body);
        return { status: 200, body: { id: record.id, name: record.name } };
      }
      case '/enroll-again': {
        // The connector of a machine runs the command of a token made for it: the machine gets a new certificate, and is the same machine.
        const token = asText(body.token);
        const record = this.machines.find((candidate) => candidate.id === (this.againTokens.get(token) ?? '') || candidate.name === asText(body.name));
        if (record === undefined) return { status: 404, body: { detail: 'no such machine' } };
        record.status = 'active';
        record.online = body.online !== false;
        record.lastSeen = inMs(0);
        // The new certificate is good for as long as a first one is, counted from now.
        record.certEnds = Date.now() - this.born + 200 * DAY;
        this.againTokens.delete(token);
        return { status: 200, body: { id: record.id, name: record.name } };
      }
      case '/enroll-host': {
        const pending = asText(body.token) || this.lastHostToken;
        if (pending !== null && !this.tokens.has(pending)) throw new Refusal(410, 'gone', 'the enrollment token is gone');
        const receipt = pending === null ? undefined : this.receiptByToken.get(pending);
        const progress = receipt === undefined ? undefined : this.enrollmentReceipts.get(receipt);
        const selection = pending === null ? undefined : this.enrollmentSelections.get(pending);
        const owner = progress?.owner ?? (asText(body.owner) || 'ada');
        if (progress !== undefined && asText(body.owner) !== '' && body.owner !== owner) throw new Refusal(403, 'authz.denied', 'This enrollment belongs to another person.');
        const record: HostRecord = { id: this.next(KIND.host), name: asText(body.name) === '' ? 'new-host' : asText(body.name), owner, home: selection?.home === undefined ? ACME.org : selection.home, online: body.online !== false, labels: [], schedulable: true, whenFull: null, revision: 1, uses: [], manages: [] };
        this.hosts.push(record);
        if (pending !== null) { this.tokens.delete(pending); this.enrollmentSelections.delete(pending); this.lastHostToken = null; }
        if (receipt !== undefined && progress !== undefined) { progress.host = record.id; this.announceEnrollment(receipt); }
        return { status: 200, body: { id: record.id, name: record.name } };
      }
      case '/report': {
        // The harness of a profile has said what it is (or is made to have said nothing): what an adapter does once it has started somewhere.
        const agent = this.agents.find((candidate) => candidate.id === asText(body.agent) || candidate.name === asText(body.agent));
        if (agent === undefined) return { status: 404, body: { detail: 'no such agent profile' } };
        this.report(agent.id, body.reported !== false);
        return { status: 204 };
      }
      case '/cannot-make': {
        // A person who holds no right to make an environment, though the page may believe otherwise.
        const name = asText(body.username);
        if (body.cannot === false) this.noMaking.delete(name);
        else this.noMaking.add(name);
        return { status: 204 };
      }
      case '/bare': {
        // An environment that offers no executor: a harness that runs apart from its tools cannot work through it.
        const environment = this.environments.find((candidate) => candidate.id === asText(body.environment) || candidate.name === asText(body.environment));
        if (environment === undefined) return { status: 404, body: { detail: 'no such environment' } };
        if (body.bare === false) this.bare.delete(environment.id);
        else this.bare.add(environment.id);
        return { status: 204 };
      }
      case '/online': {
        const record = this.machines.find((candidate) => candidate.name === asText(body.name));
        if (record === undefined) return { status: 404, body: { detail: 'no such machine' } };
        record.online = body.online !== false;
        return { status: 204 };
      }
      default:
        return null;
    }
  }
}
