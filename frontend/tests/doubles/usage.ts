/** What the fake API says of usage: a ledger of what the team's threads used, the figures, series and records read from it as deep as the caller may see, caps with how each stands, and the rates and the catalogue that credits are counted by.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same shapes,
 * the same depths (a person's own use; the summary of a place; reports), the same refusals naming what is
 * missing, the same buckets for the same spans, credits that are exact sums of the rows that had a rate,
 * and caps whose use is counted from the same ledger, so that a cap made on the screen is reached by what
 * the ledger holds. The ledger is the same every run: six uses a day for the last days, one of each of six
 * kinds, so that a test can count what a span comes to.
 */
import type {
  CapInput,
  CapStatus,
  NodeRef,
  Rate,
  RateCatalogue,
  RateFamily,
  RateSource,
  ResolvedSetting,
  RequestTiming,
  UsageComparison,
  ThreadLimits,
  UsageAnalytics,
  UsageBucket,
  UsageCap,
  UsageDimension,
  UsageEvent,
  UsageEventPage,
  Provider,
  UsageFigures,
  UsageKey,
  UserRef,
} from '../../src/generated/api-types.ts';
import type { IncomingMessage } from 'node:http';
import { ACME, id as fixtureId, userRef } from './fixtures.ts';
import { maybeHarnessName } from './harness-names.ts';
import { type Answer, type Person, Refusal, type Subscriber, type World } from './world.ts';
import { performanceOf, timingFixture } from './performance.ts';

// ---------------------------------------------------------------- the ledger

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

/** How many days back the ledger reaches unless a test asks for more. */
const HISTORY_DAYS = 120;

/** What one use of the ledger is: the same fields a row of the real ledger has, in the shapes the fake keeps them. */
interface Row {
  timing?: RequestTiming;
  running?: boolean;
  program: string | null;
  id: string;
  at: string;
  day: string;
  user: string | null;
  thread: string | null;
  topic: string;
  project: string;
  turn: string | null;
  provider: string;
  agent: string | null;
  harness: string | null;
  environment: string | null;
  envKind: 'managed' | 'connected' | null;
  model: string;
  effort: string | null;
  tier: string | null;
  tokens: { input: number; cached: number; output: number; reasoning: number };
  context: number | null;
  origin: 'harness' | 'forwarder' | 'orch';
  counted: boolean;
  rate: Rate | null;
  source: RateSource | null;
  credits: number | null;
}

/** The six kinds of use the ledger holds one of for each day, and what each came to. */
interface Pattern {
  program?: string | null;
  name: string;
  user: string;
  thread: string;
  topic: string;
  project: string;
  model: string;
  effort: string | null;
  tier: string | null;
  tokens: Row['tokens'];
  context: number | null;
  /** The time of day, in UTC. */
  hour: number;
  minute: number;
}

/** Tokens, in millions, as the whole numbers a row holds. */
const m = (millions: number): number => Math.round(millions * 1_000_000);

export const PATTERNS: readonly Pattern[] = [
  { name: 'A', user: 'ada', thread: ACME.mainThread, topic: ACME.fixLogin, project: ACME.web, model: 'gpt-5.4', effort: 'high', tier: null, tokens: { input: m(4), cached: m(1), output: m(0.8), reasoning: m(0.2) }, context: null, hour: 8, minute: 10 },
  { name: 'B', user: 'grace', thread: ACME.repro, topic: ACME.fixLogin, project: ACME.web, model: 'gpt-5.4', effort: 'medium', tier: null, tokens: { input: m(2), cached: m(0.5), output: m(0.4), reasoning: m(0.1) }, context: null, hour: 9, minute: 20 },
  { name: 'C', user: 'dora', thread: ACME.depsThread, topic: ACME.upgradeDeps, project: ACME.web, model: 'gpt-5.5-codex', effort: 'medium', tier: 'fast', tokens: { input: m(3), cached: m(2), output: m(0.6), reasoning: m(0.15) }, context: 300_000, hour: 10, minute: 30 },
  { name: 'D', user: 'dora', thread: ACME.baselineThread, topic: ACME.baseline, project: ACME.evals, model: 'gpt-5.4-mini', effort: 'low', tier: null, tokens: { input: m(1), cached: m(0.2), output: m(0.2), reasoning: 0 }, context: null, hour: 13, minute: 40 },
  { name: 'E', user: 'ada', thread: ACME.idleThread, topic: ACME.idleStop, project: ACME.infra, model: 'claude-opus-5', effort: null, tier: null, tokens: { input: m(1.5), cached: m(0.3), output: m(0.3), reasoning: 0 }, context: null, hour: 15, minute: 50 },
  { name: 'F', user: 'grace', thread: ACME.baselineThread, topic: ACME.baseline, project: ACME.evals, model: 'our-local-model', effort: null, tier: null, tokens: { input: m(0.5), cached: 0, output: m(0.1), reasoning: 0 }, context: null, hour: 16, minute: 55 },
];

/** What a day of the ledger comes to, by the kinds above, for the tests that count: credits and the part of them that is an estimate. */
export const PER_DAY = { turns: PATTERNS.length, tokens: 14_400_000, credits: 62.42, estimated: 32.795, unrated: 600_000 } as const;

/** The estimate for each family of model that the release ships, as a catalogue holds it. */
const SHIPPED_FAMILIES: RateFamily[] = [
  { match: 'gpt-5.4-mini*', vendor: 'OpenAI', source: 'https://example.invalid/prices/openai', as_of: '2026-10-01', rate: { input_per_mtok: '0.25', cached_input_per_mtok: '0.025', output_per_mtok: '2' } },
  {
    match: 'gpt-5*',
    vendor: 'OpenAI',
    source: 'https://example.invalid/prices/openai',
    as_of: '2026-10-01',
    rate: {
      input_per_mtok: '1.25',
      cached_input_per_mtok: '0.125',
      output_per_mtok: '10',
      tiers: { fast: '2' },
      above: [{ context_tokens: 272_000, input_per_mtok: '2.5', cached_input_per_mtok: '0.25', output_per_mtok: '15' }],
    },
    note: 'Prompts over 272k tokens are priced higher.',
  },
  { match: 'claude-*', vendor: 'Anthropic', source: 'https://example.invalid/prices/anthropic', as_of: '2026-10-01', rate: { input_per_mtok: '3', cached_input_per_mtok: '0.3', output_per_mtok: '15', cache_write_per_mtok: '3.75' } },
  { match: 'gemini-*', vendor: 'Google', source: 'https://example.invalid/prices/google', as_of: '2026-10-01', rate: { input_per_mtok: '1.5', cached_input_per_mtok: '0.15', output_per_mtok: '9' } },
];

const SHIPPED_AS_OF = '2026-10-01';

const SHIPPED: RateCatalogue = {
  format: 1,
  as_of: SHIPPED_AS_OF,
  unit: 'US dollars at the vendors\' list prices',
  origin: 'shipped',
  source: null,
  loaded_at: '2026-10-01T09:00:00.000Z',
  loaded_by: null,
  shipped_as_of: SHIPPED_AS_OF,
  families: SHIPPED_FAMILIES,
};

/** Credits are exact to a millionth; a sum is held to it so that floating point leaves no trace. */
const micro = (value: number): number => Math.round(value * 1e6) / 1e6;

/** A decimal as the API writes it: the shortest form. */
const decimalText = (value: number): string => String(micro(value));

const totalOf = (row: Row): number => row.tokens.input + row.tokens.output;

// ---------------------------------------------------------------- caps

interface CapRecord extends UsageCap {
  /** The identifier of a person or group the subject names, kept to read the rows by. */
  subjectId: string | null;
}

/** What only a test can say of a cap: who has let a turn past it, and what waits for room in it. */
interface CapExtras {
  bypasses: { count: number; last: { by: UserRef; at: string } | null };
  waiting: { thread: string | null; author: string | null; since: string; mine: boolean }[];
}

const invalid = (path: string, message: string): never => {
  throw new Refusal(400, 'request.invalid', message, { errors: [{ path, message }] });
};

/** Whether a model name is covered by a pattern of a cap: a name, a family by its beginning, or a group of its provider. */
function modelMatches(pattern: string, model: string, groups: readonly string[]): boolean {
  if (pattern.startsWith('@')) return groups.includes(pattern.slice(1));
  const name = model.toLowerCase().split('/').at(-1) ?? '';
  if (!pattern.includes('*')) return name === (pattern.includes('/') ? model.toLowerCase() : pattern);
  const expression = new RegExp(`^${pattern.split('*').map((part) => part.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*')}$`);
  return expression.test(name);
}

/** The start of the day, week (Monday) or month a moment is in, in UTC. */
function windowStart(period: 'day' | 'week' | 'month' | 'total', now: number, countedFrom: string | null): number {
  const date = new Date(now);
  const midnight = Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
  if (period === 'day') return midnight;
  if (period === 'week') return midnight - ((date.getUTCDay() + 6) % 7) * DAY_MS;
  if (period === 'month') return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1);
  return countedFrom === null ? 0 : Date.parse(`${countedFrom}T00:00:00Z`);
}

function windowEnd(period: 'day' | 'week' | 'month' | 'total', now: number): number | null {
  const start = windowStart(period, now, null);
  if (period === 'day') return start + DAY_MS;
  if (period === 'week') return start + 7 * DAY_MS;
  if (period === 'month') {
    const date = new Date(start);
    return Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 1);
  }
  return null;
}

// ---------------------------------------------------------------- the double

/** A cap as the API says it: without what only the fake keeps to read the rows by. */
function publicCap(cap: CapRecord): UsageCap {
  const { subjectId: _subject, ...rest } = cap;
  return rest;
}

/** The three settings that decide how credits are counted. */
type SettingKey = 'usage.rates' | 'usage.catalogue' | 'usage.catalogue_scale';
const SETTING_KEYS: readonly SettingKey[] = ['usage.rates', 'usage.catalogue', 'usage.catalogue_scale'];

const asText = (value: unknown): string => (typeof value === 'string' ? value : '');
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const stringsOf = (value: unknown): string[] => (Array.isArray(value) ? value.filter((one): one is string => typeof one === 'string') : []);

export class Usage {
  private requestListeners: { subscriber: Subscriber; person: Person; id: number; scope: string; seen: Map<string,string> }[] = [];
  readonly world: World;
  rows: Row[] = [];
  caps: CapRecord[] = [];
  extras = new Map<string, CapExtras>();
  /** Turns under way, as a test says them, which a cap on turns at once counts. */
  running: { thread: string | null; user: string; provider: string | null }[] = [];
  catalogue: RateCatalogue = structuredClone(SHIPPED);
  /** The rate card (`usage.rates`) as the team wrote it: a pattern that names models, and the rate it says. */
  card: Record<string, Rate> = {};
  /** The setting of a place that switches estimates off, and what one unit of the catalogue is worth there. */
  estimates = true;
  scale = '1';
  /** No loaded catalogue is a successful null read, as in a development installation. */
  noCatalogue = false;
  /** How many times each of the three settings was written, which a change must have seen. */
  revisions: Record<SettingKey, number> = { 'usage.rates': 0, 'usage.catalogue': 0, 'usage.catalogue_scale': 0 };
  /** The address the catalogue is fetched from, if one was set. */
  fetchUrl: string | null = null;
  private sequence = 0;
  private capSequence = 0;

  constructor(world: World) {
    this.world = world;
  }

  /** Puts the ledger and everything else back to how a team starts: no cap, the shipped catalogue, a history of the last days. */
  reset(): void {
    this.requestListeners = [];
    this.caps = [];
    this.extras.clear();
    this.running = [];
    this.catalogue = structuredClone(SHIPPED);
    this.card = {};
    this.estimates = true;
    this.scale = '1';
    this.noCatalogue = false;
    this.revisions = { 'usage.rates': 0, 'usage.catalogue': 0, 'usage.catalogue_scale': 0 };
    this.fetchUrl = null;
    this.sequence = 0;
    this.capSequence = 0;
    this.history(HISTORY_DAYS, Date.now());
  }

  // ---------------------------------------------------------------- making the ledger

  /** Writes the ledger anew: the six kinds of use for each of the last `days` days, counted at the rates in force now. */
  history(days: number, now: number): void {
    this.rows = [];
    const today = Date.UTC(new Date(now).getUTCFullYear(), new Date(now).getUTCMonth(), new Date(now).getUTCDate());
    for (let back = days - 1; back >= 0; back -= 1) {
      const midnight = today - back * DAY_MS;
      // What was used today is in the past however early in the day this is read: a second apart from midnight, in order.
      PATTERNS.forEach((pattern, order) => this.write(pattern, back === 0 ? midnight + (order + 1) * 1_000 : midnight + pattern.hour * HOUR_MS + pattern.minute * 60_000));
    }
  }

  /** The rate a use of a model on a provider is counted at now, where it comes from, and the pattern that matched. */
  rateFor(model: string, providerId: string): { rate: Rate | null; source: RateSource | null; match: string | null } {
    const own = this.world.resources.modelsOf(providerId).find((candidate) => candidate.model_name === model)?.parameters.rate;
    if (own !== undefined && own !== null) return { rate: own, source: 'provider', match: null };
    const pattern = this.cardMatch(model);
    if (pattern !== null) return { rate: this.card[pattern]!, source: 'rate_card', match: pattern };
    if (!this.estimates) return { rate: null, source: null, match: null };
    const family = this.familyOf(model);
    if (family === null) return { rate: null, source: null, match: null };
    return { rate: this.scaled(family.rate), source: 'catalogue', match: family.match };
  }

  /** The pattern of the rate card that names a model with the most characters that are not `*`. */
  private cardMatch(model: string): string | null {
    const name = model.toLowerCase().split('/').at(-1) ?? '';
    const found = Object.keys(this.card).filter((pattern) => modelMatches(pattern, name, []));
    found.sort((one, other) => other.replaceAll('*', '').length - one.replaceAll('*', '').length);
    return found[0] ?? null;
  }

  /** The family of the catalogue in force that a model name matches, the most particular first. */
  familyOf(model: string): RateFamily | null {
    if (this.noCatalogue) return null;
    const name = model.toLowerCase().split('/').at(-1) ?? '';
    const found = this.catalogue.families.filter((family) => modelMatches(family.match, name, []));
    found.sort((one, other) => other.match.length - one.match.length);
    return found[0] ?? null;
  }

  /** A rate multiplied by what one unit of the catalogue is worth here. */
  private scaled(rate: Rate): Rate {
    const factor = Number(this.scale);
    if (factor === 1) return rate;
    const times = (value: string): string => decimalText(Number(value) * factor);
    return {
      ...rate,
      input_per_mtok: times(rate.input_per_mtok),
      cached_input_per_mtok: times(rate.cached_input_per_mtok),
      output_per_mtok: times(rate.output_per_mtok),
      ...(rate.above === undefined || rate.above === null
        ? {}
        : { above: rate.above.map((step) => ({ ...step, input_per_mtok: times(step.input_per_mtok), output_per_mtok: times(step.output_per_mtok), ...(step.cached_input_per_mtok === undefined || step.cached_input_per_mtok === null ? {} : { cached_input_per_mtok: times(step.cached_input_per_mtok) }) })) }),
    };
  }

  /** Credits for tokens at a rate: the three prices, the step a large prompt crossed, and the multiple of the speed. */
  creditsOf(tokens: Row['tokens'], rate: Rate, tier: string | null, context: number | null): number {
    const step = (rate.above ?? []).filter((one) => context !== null && one.context_tokens < context).sort((a, b) => b.context_tokens - a.context_tokens)[0];
    const input = Number(step?.input_per_mtok ?? rate.input_per_mtok);
    const cached = Number(step?.cached_input_per_mtok ?? rate.cached_input_per_mtok);
    const output = Number(step?.output_per_mtok ?? rate.output_per_mtok);
    const multiple = tier === null ? 1 : Number(rate.tiers?.[tier] ?? 1);
    return micro(((tokens.input - tokens.cached) * input + tokens.cached * cached + tokens.output * output) / 1e6 * multiple);
  }

  /** Writes one use into the ledger, counted at the rate in force now. */
  write(pattern: Pattern, at: number, user: string | null = pattern.user): Row {
    const binding = this.world.resources.bindingOf(pattern.thread);
    const refs = this.world.resources.refsOf(binding);
    const found = this.rateFor(pattern.model, binding.provider);
    const stamp = new Date(at).toISOString();
    this.sequence += 1;
    const row: Row = {
      program: pattern.program ?? null,
      id: fixtureId(0x31, this.sequence),
      at: stamp,
      day: stamp.slice(0, 10),
      user,
      thread: pattern.thread,
      topic: pattern.topic,
      project: pattern.project,
      turn: fixtureId(0x32, this.sequence),
      provider: binding.provider,
      agent: binding.agent,
      harness: refs.agent?.harness_kind ?? null,
      environment: binding.environment,
      envKind: refs.environment?.placement ?? null,
      model: pattern.model,
      effort: pattern.effort,
      tier: pattern.tier,
      tokens: { ...pattern.tokens },
      context: pattern.context,
      origin: 'harness',
      counted: true,
      rate: found.rate,
      source: found.source,
      credits: found.rate === null ? null : this.creditsOf(pattern.tokens, found.rate, pattern.tier, pattern.context),
    };
    this.rows.push(row);
    return row;
  }

  // ---------------------------------------------------------------- shapes

  private person(username: string | null): UserRef | null {
    if (username === null) return null;
    const found = this.world.people.get(username);
    return found === undefined ? null : userRef(found.me);
  }

  /** A place as the caller may know it: with its name where they may see it, and without where they may not. */
  private place(caller: Person, nodeId: string): NodeRef {
    const row = this.world.places.rowOf(nodeId);
    const kind = row?.kind ?? 'org';
    const seen = this.world.places.access(caller, nodeId).permissions.includes(`${kind}.view`);
    return { id: nodeId, kind, name: seen ? (row?.name ?? null) : null, redacted: !seen };
  }

  private refsOfRow(row: Row) {
    return this.world.resources.refsOf({ provider: row.provider, agent: row.agent ?? '', environment: row.environment ?? '', model: row.model, effort: row.effort, directory: '' });
  }

  private modelDisplay(row: Row): string {
    return this.world.resources.modelsOf(row.provider).find((candidate) => candidate.model_name === row.model)?.display_name ?? row.model;
  }

  /** The member of a dimension that a row belongs to, named for the caller. */
  private keyOf(dimension: UsageDimension, row: Row, caller: Person): UsageKey {
    switch (dimension) {
      case 'person':
        return { person: this.person(row.user) };
      case 'model':
        return { model: row.model, model_display: this.modelDisplay(row) };
      case 'provider': {
        const provider = this.refsOfRow(row).provider;
        return provider === null ? {} : { provider };
      }
      case 'agent':
        return { agent: row.harness === null ? null : { kind: row.harness, name: this.refsOfRow(row).agent?.name ?? row.harness } };
      case 'environment':
        return { environment: row.envKind };
      case 'effort':
        return { effort: row.effort };
      case 'tier':
        return { tier: row.tier };
      case 'program':
        return { program: row.program };
      case 'org':
        return { org: null };
      case 'project':
        return { project: this.place(caller, row.project) };
      case 'topic':
        return { topic: this.place(caller, row.topic) };
      case 'thread':
        return { thread: row.thread === null ? null : this.place(caller, row.thread) };
    }
  }

  private keyText(dimension: UsageDimension, row: Row): string {
    switch (dimension) {
      case 'person':
        return row.user ?? '';
      case 'model':
        return row.model;
      case 'provider':
        return row.provider;
      case 'agent':
        return row.harness ?? '';
      case 'environment':
        return row.envKind ?? '';
      case 'effort':
        return row.effort ?? '';
      case 'tier':
        return row.tier ?? '';
      case 'program':
        return row.program ?? '';
      case 'org':
        return '';
      case 'project':
        return row.project;
      case 'topic':
        return row.topic;
      case 'thread':
        return row.thread ?? '';
    }
  }

  private figures(rows: readonly Row[]): UsageFigures {
    const sum = (pick: (row: Row) => number) => rows.reduce((total, row) => total + (row.counted ? pick(row) : 0), 0);
    return {
      turns: new Set(rows.flatMap((row)=>row.turn===null ? [] : [row.turn])).size,
      tokens: {
        input_tokens: sum((row) => row.tokens.input),
        cached_input_tokens: sum((row) => row.tokens.cached),
        output_tokens: sum((row) => row.tokens.output),
        reasoning_tokens: sum((row) => row.tokens.reasoning),
        total_tokens: sum(totalOf),
      },
      ...(rows.some((row) => row.timing !== undefined) ? { performance: performanceOf(rows.flatMap((row) => row.timing === undefined ? [] : [row.timing])) } : {}),
      credits: decimalText(sum((row) => row.credits ?? 0)),
      estimated_credits: decimalText(sum((row) => (row.source === 'catalogue' ? (row.credits ?? 0) : 0))),
      unrated_tokens: sum((row) => (row.credits === null ? totalOf(row) : 0)),
    };
  }

  // ---------------------------------------------------------------- who sees what

  /** How deep the caller is shown the use of a place. */
  private depthAt(caller: Person, nodeId: string): 'own' | 'summary' | 'reports' {
    const held = this.world.places.access(caller, nodeId).permissions;
    return held.includes('usage.view') ? 'reports' : held.includes('usage.summary') ? 'summary' : 'own';
  }

  private under(row: Row, nodeId: string): boolean {
    const chain = [row.thread, row.topic, row.project].filter((one): one is string => one !== null);
    return chain.some((one) => this.world.places.pathTo(one).some((entry) => entry.id === nodeId));
  }

  /** The rows a scope shows the caller, with the depth they are shown at. */
  private scoped(caller: Person, scope: string): { rows: Row[]; depth: 'own' | 'summary' | 'reports'; nodeId: string | null; own: boolean } {
    if (scope === 'me') return { rows: this.rows.filter((row) => row.user === caller.me.username), depth: 'own', nodeId: null, own: false };
    const node = /^node:([0-9a-f-]{36})$/.exec(scope);
    if (node?.[1] !== undefined) {
      const nodeId = node[1];
      if (this.world.places.rowOf(nodeId) === undefined) throw new Refusal(404, 'not_found', 'there is no such place');
      this.world.places.need(caller, nodeId, `${this.world.places.rowOf(nodeId)?.kind ?? 'org'}.view`);
      const depth = this.depthAt(caller, nodeId);
      const inside = this.rows.filter((row) => this.under(row, nodeId));
      return { rows: depth === 'own' ? inside.filter((row) => row.user === caller.me.username) : inside, depth, nodeId, own: true };
    }
    const provider = /^provider:([0-9a-f-]{36})$/.exec(scope);
    if (provider?.[1] !== undefined) {
      // Whoever may see the provider reads what ran on it; the provider's own request says whether they may.
      this.world.resources.handle(caller, 'GET', `/providers/${provider[1]}`, new URLSearchParams(), {});
      return { rows: this.rows.filter((row) => row.provider === provider[1]), depth: 'reports', nodeId: null, own: true };
    }
    return invalid('scope', 'is `me`, `node:` and an identifier, or `provider:` and an identifier');
  }

  private denied(nodeId: string | null, caller: Person, what: string): never {
    if (nodeId !== null) this.world.places.need(caller, nodeId, 'usage.view');
    throw new Refusal(403, 'authz.denied', `${what} needs usage.view here`, { required: 'usage.view', object: { kind: 'user', id: caller.me.id }, requestable: false, override: false, missing: [] });
  }

  // ---------------------------------------------------------------- figures

  private bucketStart(day: string, at: string, bucket: UsageBucket): number {
    const midnight = Date.parse(`${day}T00:00:00Z`);
    if (bucket === 'hour') return Math.floor(Date.parse(at) / HOUR_MS) * HOUR_MS;
    if (bucket === 'day') return midnight;
    if (bucket === 'week') return midnight - ((new Date(midnight).getUTCDay() + 6) % 7) * DAY_MS;
    const date = new Date(midnight);
    return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1);
  }

  private bucketsOf(from: string, to: string, bucket: UsageBucket): number[] {
    const starts: number[] = [];
    const first = Date.parse(`${from}T00:00:00Z`);
    const last = Date.parse(`${to}T00:00:00Z`);
    if (bucket === 'hour') {
      for (let at = first; at < last + DAY_MS; at += HOUR_MS) starts.push(at);
      return starts;
    }
    let at = this.bucketStart(from, `${from}T00:00:00Z`, bucket);
    while (at <= last) {
      starts.push(at);
      if (bucket === 'month') {
        const date = new Date(at);
        at = Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 1);
      } else at += (bucket === 'day' ? 1 : 7) * DAY_MS;
    }
    return starts;
  }

  private filtered(rows: readonly Row[], query: URLSearchParams): Row[] {
    const program = query.get('program');
    const effort = query.get('effort');
    const tier = query.get('tier');
    const user = query.get('user_id');
    const model = query.get('model');
    const provider = query.get('provider_id');
    const harness = query.get('harness_kind');
    const kind = query.get('environment_kind');
    const project = query.get('project_id');
    const topic = query.get('topic_id');
    const thread = query.get('thread_id');
    return rows.filter((row) => {
      const outcome = row.running ? 'running' : row.timing?.outcome.state === 'told' ? row.timing.outcome.value : 'not_measured';
      const askedOutcome = query.get('outcome');
      if (askedOutcome === 'errors' ? ['done', 'forced', 'running', 'not_measured'].includes(outcome) : askedOutcome !== null && askedOutcome !== outcome) return false;
      if (query.get('turn_id') !== null && row.turn !== query.get('turn_id')) return false;
      const org = query.get('org_id');
      if (org !== null && !this.under(row, org)) return false;
      if (program !== null && (program === 'none' ? row.program !== null : row.program !== program)) return false;
      if (effort !== null && (effort === 'none' ? row.effort !== null : row.effort !== effort)) return false;
      if (tier !== null && (tier === 'none' ? row.tier !== null : row.tier !== tier)) return false;
      if (user !== null && this.world.people.get(row.user ?? '')?.me.id !== user) return false;
      if (model !== null && row.model !== model) return false;
      if (provider !== null && row.provider !== provider) return false;
      if (harness !== null && row.harness !== harness) return false;
      if (kind !== null && row.envKind !== kind) return false;
      if (project !== null && row.project !== project) return false;
      if (topic !== null && row.topic !== topic) return false;
      if (thread !== null && row.thread !== thread) return false;
      return true;
    });
  }

  private health(caller: Person, providerId: string): Answer {
    const scope = this.scoped(caller, `provider:${providerId}`);
    const since = new Date(Date.now() - 15 * 60_000).toISOString();
    const measured = (rows: readonly Row[]) => performanceOf(rows.flatMap((row) => row.timing === undefined ? [] : [row.timing]));
    const recent = measured(scope.rows.filter((row) => row.at >= since));
    const baseline = measured(scope.rows.filter((row) => Date.parse(row.at) >= Date.now() - 7 * DAY_MS));
    return { status: 200, body: { since, recent, baseline, errors: recent.requests >= 10 && Object.values(recent.errors).reduce((sum,n) => sum+n,0) / recent.requests >= 0.2, first_output: recent.first_token_samples >= 10 && baseline.first_token_samples >= 10 && recent.first_token_ms !== null && baseline.first_token_ms !== null && recent.first_token_ms > baseline.first_token_ms * 3 } };
  }

  private analytics(caller: Person, query: URLSearchParams, accept: string): Answer {
    const scopeText = query.get('scope') ?? '';
    const scope = this.scoped(caller, scopeText);
    const from = query.get('from') ?? '';
    const to = query.get('to') ?? '';
    for (const [path, value] of [['from', from], ['to', to]] as const) if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || Number.isNaN(Date.parse(`${value}T00:00:00Z`))) invalid(path, 'is a day, written YYYY-MM-DD');
    if (to < from) invalid('to', 'is not before from');
    const days = Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS) + 1;
    const asked = query.get('bucket') as UsageBucket | null;
    const bucket: UsageBucket = asked ?? (days <= 2 ? 'hour' : days <= 92 ? 'day' : days <= 730 ? 'week' : 'month');
    if (bucket === 'hour' && days > 31) invalid('bucket', 'by the hour is for a span of up to 31 days');
    if (bucket === 'day' && days > 800) invalid('bucket', 'by the day is for a span of up to 800 days');
    const seriesBy = query.get('series_by') as UsageDimension | null;
    const defaults: UsageDimension[] = scopeText === 'me' ? ['project', 'model', 'provider', 'agent', 'environment'] : scopeText.startsWith('provider:') ? ['person', 'model', 'org', 'environment'] : ['person', 'model', 'provider', 'agent', 'environment'];
    const breakdownText = query.get('breakdowns');
    const breakdowns: UsageDimension[] = breakdownText === null ? defaults : breakdownText.split(',').map((word) => word.trim()).filter((word) => word !== '').map((word) => word as UsageDimension);
    for (const dimension of [...(seriesBy === null ? [] : [seriesBy]), ...breakdowns]) if (scope.depth === 'summary' && dimension === 'thread') this.denied(scope.nodeId, caller, 'this dimension');
    const wantedUser = query.get('user_id');
    if (wantedUser !== null && wantedUser !== caller.me.id && scope.depth === 'own') this.denied(scope.nodeId, caller, 'somebody else\'s use');

    const inSpan = this.filtered(scope.rows, query).filter((row) => row.day >= from && row.day <= to && !row.running);
    const totals = this.figures(inSpan);
    const starts = this.bucketsOf(from, to, bucket);
    const byStart = new Map<number, Row[]>();
    for (const row of inSpan) {
      const key = this.bucketStart(row.day, row.at, bucket);
      byStart.set(key, [...(byStart.get(key) ?? []), row]);
    }

    // The dimension the series is split by: its eight largest members over the span, and the rest as one.
    const ranked = (dimension: UsageDimension): { text: string; rows: Row[] }[] => {
      const groups = new Map<string, Row[]>();
      for (const row of inSpan) groups.set(this.keyText(dimension, row), [...(groups.get(this.keyText(dimension, row)) ?? []), row]);
      return [...groups].map(([text, rows]) => ({ text, rows })).sort((one, other) => Number(this.figures(other.rows).credits) - Number(this.figures(one.rows).credits) || this.figures(other.rows).tokens.total_tokens - this.figures(one.rows).tokens.total_tokens || one.text.localeCompare(other.text));
    };
    const kept = seriesBy === null ? [] : ranked(seriesBy).slice(0, 8);
    const hasRest = seriesBy !== null && ranked(seriesBy).length > 8;
    const series = starts.map((start) => {
      const rows = byStart.get(start) ?? [];
      return {
        at: new Date(start).toISOString(),
        figures: this.figures(rows),
        by:
          seriesBy === null
            ? []
            : [
                ...kept.map((member) => ({ key: this.keyOf(seriesBy, member.rows[0]!, caller), figures: this.figures(rows.filter((row) => this.keyText(seriesBy, row) === member.text)) })),
                ...(hasRest ? [{ key: {} as UsageKey, figures: this.figures(rows.filter((row) => !kept.some((member) => member.text === this.keyText(seriesBy, row)))) }] : []),
              ],
      };
    });

    const limit = Math.min(100, Math.max(1, Number(query.get('limit') ?? 10)));
    const totalCredits = Number(totals.credits);
    const tables = breakdowns.map((dimension) => {
      const members = ranked(dimension);
      return {
        by: dimension,
        more: Math.max(0, members.length - limit),
        rows: members.slice(0, limit).map((member) => {
          const figures = this.figures(member.rows);
          return {
            key: this.keyOf(dimension, member.rows[0]!, caller),
            figures,
            share: totalCredits > 0 ? Number(figures.credits) / totalCredits : totals.tokens.total_tokens > 0 ? figures.tokens.total_tokens / totals.tokens.total_tokens : 0,
          };
        }),
      };
    });

    const mine = scope.own ? this.figures(inSpan.filter((row) => row.user === caller.me.username)) : null;
    const groups = new Map<string, Row[]>();
    for (const row of inSpan.filter((row) => row.timing !== undefined)) { const key = JSON.stringify([row.model, row.tier, row.effort, row.program]); groups.set(key, [...(groups.get(key) ?? []), row]); }
    const comparison: UsageComparison[] = [...groups.values()].flatMap((rows) => { const row = rows[0]; return row === undefined ? [] : [{ model: row.model, tier: row.tier, effort: row.effort, program: row.program, performance: performanceOf(rows.flatMap((row) => row.timing === undefined ? [] : [row.timing])) }]; });
    const beforeTo = new Date(Date.parse(`${from}T00:00:00Z`) - DAY_MS).toISOString().slice(0, 10);
    const beforeFrom = new Date(Date.parse(`${from}T00:00:00Z`) - days * DAY_MS).toISOString().slice(0, 10);
    const beforeRows = this.filtered(scope.rows, query).filter((row) => row.day >= beforeFrom && row.day <= beforeTo && !row.running);
    const performanceBefore = performanceOf(beforeRows.flatMap((row) => row.timing === undefined ? [] : [row.timing]));
    const answer: UsageAnalytics = { scope: scopeText, from, to, bucket, depth: scope.depth, totals, series, breakdowns: tables, me: mine, comparison, performance_before: performanceBefore, figures_before: this.figures(beforeRows) };
    if (query.get('health') === 'true' && scopeText.startsWith('provider:')) {
      const since = new Date(Date.now() - 15 * 60_000).toISOString();
      const measured = (rows: readonly Row[]) => performanceOf(rows.flatMap((row) => row.timing === undefined ? [] : [row.timing]));
      const recent = measured(scope.rows.filter((row) => row.at >= since));
      const baseline = measured(scope.rows.filter((row) => Date.parse(row.at) >= Date.now() - 7 * DAY_MS));
      answer.degradation = { since, recent, baseline, errors: recent.requests >= 10 && Object.values(recent.errors).reduce((a, n) => a + n, 0) / recent.requests >= 0.2, first_output: recent.first_token_samples >= 10 && baseline.first_token_samples >= 10 && recent.first_token_ms !== null && baseline.first_token_ms !== null && recent.first_token_ms > baseline.first_token_ms * 3 };
    }
    if (accept.includes('text/csv')) {
      const lines = ['at,turns,input_tokens,cached_input_tokens,output_tokens,reasoning_tokens,total_tokens,credits,estimated_credits,unrated_tokens'];
      for (const point of series) {
        const f = point.figures;
        lines.push([point.at.slice(0, 19) + 'Z', f.turns, f.tokens.input_tokens, f.tokens.cached_input_tokens, f.tokens.output_tokens, f.tokens.reasoning_tokens, f.tokens.total_tokens, f.credits, f.estimated_credits, f.unrated_tokens].join(','));
      }
      return { status: 200, text: `${lines.join('\n')}\n` };
    }
    return { status: 200, body: answer };
  }

  // ---------------------------------------------------------------- records

  private event(caller: Person, row: Row): UsageEvent {
    const refs = this.refsOfRow(row);
    const thread = row.thread === null ? null : this.place(caller, row.thread);
    return {
      id: row.id,
      access_program: row.program,
      ...(row.timing === undefined ? {} : { timing: row.timing }),
      ...(row.running ? { running: true } : {}),
      at: row.at,
      origin: row.origin,
      purpose: 'turn',
      scope: row.timing === undefined ? 'turn' : 'request',
      counted: row.counted,
      project: this.place(caller, row.project),
      topic: this.place(caller, row.topic),
      thread,
      turn_id: thread !== null && !thread.redacted ? row.turn : null,
      turn_ordinal: thread !== null && !thread.redacted && row.turn !== null ? 1 : null,
      user: this.person(row.user),
      provider: refs.provider ?? { id: row.provider, name: '', kind: 'api', harness_kind: null, harness: null, address: '', placement: 'managed', status: 'ready' },
      agent: refs.agent,
      harness_kind: row.harness,
      harness: maybeHarnessName(row.harness),
      environment: refs.environment,
      environment_kind: row.envKind,
      arrangement: null,
      model: row.model,
      model_display: this.modelDisplay(row),
      effort: row.effort,
      tier: row.tier,
      tokens: { input_tokens: row.tokens.input, cached_input_tokens: row.tokens.cached, output_tokens: row.tokens.output, reasoning_tokens: row.tokens.reasoning, total_tokens: totalOf(row) },
      request_ref: null,
      context_tokens: row.context,
      rate: row.rate,
      rate_source: row.source,
      credits: row.credits === null ? null : decimalText(row.credits),
    };
  }

  private events(caller: Person, query: URLSearchParams, accept: string): Answer {
    const scopeText = query.get('scope') ?? '';
    const scope = this.scoped(caller, scopeText);
    const from = query.get('from') ?? '';
    const to = query.get('to') ?? '';
    if (to < from) invalid('to', 'is not before from');
    if (query.get('thread_id') !== null && scope.depth === 'summary') this.denied(scope.nodeId, caller, 'the use of one thread');
    const wantedUser = query.get('user_id');
    if (wantedUser !== null && wantedUser !== caller.me.id && scope.depth !== 'reports' && scope.nodeId !== null) this.denied(scope.nodeId, caller, 'somebody else\'s records');
    // A summary gives no record but the caller's own.
    let readable = scope.depth === 'summary' ? scope.rows.filter((row) => row.user === caller.me.username) : scope.rows;
    if (scopeText.startsWith('provider:')) {
      const provider = this.world.resources.handle(caller, 'GET', `/providers/${scopeText.slice('provider:'.length)}`, new URLSearchParams(), {})?.body as Provider | undefined;
      if (provider?.access.permissions.includes('provider.manage') !== true) readable = readable.filter((row) => {
        const place = row.thread ?? row.topic;
        try { this.world.places.need(caller, place, `${this.world.places.rowOf(place)?.kind ?? 'topic'}.view`); return true; }
        catch (error) { if (error instanceof Refusal) return false; throw error; }
      });
    }
    const counted = query.get('counted');
    let rows = this.filtered(readable, query).filter((row) => row.day >= from && row.day <= to && (!row.running || query.get('include_running') === 'true') && (counted === null || String(row.counted) === counted));
    const newestFirst = query.get('order') !== 'asc';
    rows = rows.sort((one, other) => (newestFirst ? -1 : 1) * (Date.parse(one.at) - Date.parse(other.at) || one.id.localeCompare(other.id)));
    const cursor = /^(\d+)\.(.+)$/.exec(query.get('cursor') ?? '');
    if (cursor?.[1] !== undefined && cursor[2] !== undefined) {
      const at = Number(cursor[1]), id = cursor[2];
      rows = rows.filter((row) => (newestFirst ? -1 : 1) * (Date.parse(row.at) - at || row.id.localeCompare(id)) > 0);
    }
    const start = 0;
    const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? 50)));
    const page = rows.slice(start, start + limit);
    const next = start + limit < rows.length && page.length > 0 ? `${Date.parse(page.at(-1)!.at)}.${page.at(-1)!.id}` : null;
    if (accept.includes('text/csv')) {
      const lines = ['id,at,user,thread,model,provider,tokens,credits,input_tokens,output_tokens,first_output_ms,speed_tokens_per_second,request_ms,outcome,outcome_message,vendor_words,running,access_program'];
      const cell = (value: string | number) => /[",\n]/.test(String(value)) ? `"${String(value).replaceAll('"','""')}"` : String(value);
      for (const row of page) {
        const timing=row.timing;
        const first=timing?.first_token_ms.state === 'told' ? timing.first_token_ms.value : '';
        const speed=timing?.speed_milli.state === 'told' ? timing.speed_milli.value / 1000 : '';
        const whole=timing?.request_ms.state === 'told' ? timing.request_ms.value : '';
        lines.push([row.id,row.at,row.user ?? '',row.thread ?? '',row.model,row.provider,totalOf(row),row.credits === null ? '' : decimalText(row.credits),timing === undefined ? row.tokens.input : timing.reported_tokens?.input_tokens ?? '',timing === undefined ? row.tokens.output : timing.reported_tokens?.output_tokens ?? '',first,speed,whole,timing?.outcome.state === 'told' ? timing.outcome.value : '',timing?.error_message ?? '',timing?.vendor_words ?? '',row.running ? 'true' : 'false',row.program ?? ''].map(cell).join(','));
      }
      return { status: 200, text: `${lines.join('\n')}\n`, headers: next === null ? {} : { 'x-next-cursor': next } };
    }
    return { status: 200, body: { items: page.map((row) => this.event(caller, row)), next_cursor: next } };
  }

  // ---------------------------------------------------------------- caps

  /** Requests share the exact record projection and current scope checks of the HTTP route. */
  private requestPage(person: Person, scope: string): Answer {
    const day = new Date().toISOString().slice(0, 10);
    const from = new Date(Date.now() - DAY_MS).toISOString().slice(0, 10);
    const [base,org] = scope.split(':org:');
    return this.events(person, new URLSearchParams({ scope: base ?? 'me', from, to: day, limit: '200', include_running: 'true', requests_only: 'true', ...(org === undefined ? {} : { org_id: org }) }), '');
  }

  private tellRequests(): void {
    this.requestListeners = this.requestListeners.filter((listener) => {
      try {
        const answer = this.requestPage(listener.person, listener.scope);
        const page = answer.body as UsageEventPage;
        const items = page.items.filter((row) => listener.seen.get(row.id) !== JSON.stringify(row));
        for (const row of items) listener.seen.set(row.id, JSON.stringify(row));
        if (items.length > 0) listener.subscriber.send({ op: 'event', id: listener.id, feed: `usage:${listener.scope}`, data: { items, next_cursor: null } });
        return true;
      } catch {
        listener.subscriber.send({ op: 'closed', id: listener.id, feed: `usage:${listener.scope}`, reason: 'revoked' });
        return false;
      }
    });
  }

  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const id = Number(frame.id);
    if (frame.op === 'sub' && typeof frame.feed === 'string' && frame.feed.startsWith('usage:')) {
      const scope = frame.feed.slice('usage:'.length);
      try {
        if (person === undefined) throw new Error('sign in');
        const answer = this.requestPage(person, scope);
        const page = answer.body as UsageEventPage;
        this.requestListeners.push({ subscriber, person, id, scope, seen: new Map(page.items.map((row) => [row.id, JSON.stringify(row)])) });
        subscriber.send({ op: 'subscribed', id, feed: frame.feed, seq: null, mode: 'ephemeral' });
        subscriber.send({ op: 'event', id, feed: frame.feed, data: answer.body });
      } catch {
        subscriber.send({ op: 'error', id, code: 'forbidden', detail: 'This usage is not visible.' });
      }
      return true;
    }
    if (frame.op === 'unsub') {
      const before = this.requestListeners.length;
      this.requestListeners = this.requestListeners.filter((listener) => listener.subscriber !== subscriber || listener.id !== id);
      if (before === this.requestListeners.length) return false;
      subscriber.send({ op: 'closed', id, feed: typeof frame.feed === 'string' ? frame.feed : '', reason: 'unsubscribed' });
      return true;
    }
    return false;
  }

  gone(subscriber: Subscriber): void {
    this.requestListeners = this.requestListeners.filter((listener) => listener.subscriber !== subscriber);
  }

  /** Whether a row is use that a cap covers. */
  private covered(cap: CapRecord, row: Row, person: string | null): boolean {
    if (cap.node !== null && !this.under(row, cap.node.id)) return false;
    if (cap.provider !== null && row.provider !== cap.provider.id) return false;
    if (cap.models.length > 0) {
      const groups = this.world.resources.modelsOf(row.provider).find((candidate) => candidate.model_name === row.model)?.groups ?? [];
      if (!cap.models.some((pattern) => modelMatches(pattern, row.model, groups))) return false;
    }
    if (cap.tiers.length > 0 && (row.tier === null || !cap.tiers.includes(row.tier))) return false;
    if (cap.harness_kinds.length > 0 && (row.harness === null || !cap.harness_kinds.includes(row.harness))) return false;
    return this.covers(cap, row.user) && (person === null || row.user === person);
  }

  /** Whether a cap covers a person. */
  private covers(cap: CapRecord, username: string | null): boolean {
    const subject = cap.subject;
    if (subject.kind === 'everyone') return true;
    if (username === null) return false;
    if (subject.kind === 'user') return subject.user.username === username;
    const person = this.world.people.get(username);
    if (person === undefined) return false;
    if (subject.kind === 'group') return this.world.access.isHolder(person, { kind: 'group', group: subject.group }, cap.node?.id ?? subject.group.org_id);
    if (subject.kind === 'members') return this.world.places.isMember(person, subject.node.id);
    const role = this.world.places.roleAt(person, subject.at?.id ?? cap.node?.id ?? ACME.org);
    return role === subject.role;
  }

  private statusOf(cap: CapRecord, person: string | null, now: number): CapStatus {
    const extra = this.extras.get(cap.id);
    const base = { cap: publicCap(cap), person: this.person(person), in_progress: '0', left_out: { unrated_tokens: 0, unmetered_threads: 0 }, replaced_by: null, can_bypass: false, bypasses: extra?.bypasses ?? { count: 0, last: null } };
    if (cap.meter === 'running') {
      const running = this.running.filter((turn) => (person === null || turn.user === person) && this.covers(cap, turn.user) && (cap.provider === null || turn.provider === cap.provider.id));
      const waiting = extra?.waiting.length ?? 0;
      const amount = cap.amount === null ? null : Number(cap.amount);
      return { ...base, used: String(running.length), waiting, state: amount !== null && running.length >= amount ? 'reached' : 'ok', period_ends_at: null };
    }
    const period = cap.period ?? 'total';
    const start = windowStart(period, now, cap.counted_from);
    const covered = this.rows.filter((row) => row.counted && Date.parse(row.at) >= start && Date.parse(row.at) <= now + DAY_MS && this.covered(cap, row, person));
    const sum = cap.meter === 'tokens' ? covered.reduce((total, row) => total + totalOf(row), 0) : micro(covered.reduce((total, row) => total + (row.credits ?? 0), 0));
    const unrated = cap.meter === 'credits' ? covered.reduce((total, row) => total + (row.credits === null ? totalOf(row) : 0), 0) : 0;
    const amount = cap.amount === null ? null : Number(cap.amount);
    const warn = cap.warn_percent === null || amount === null ? null : (amount * cap.warn_percent) / 100;
    const end = windowEnd(period, now);
    return {
      ...base,
      left_out: { unrated_tokens: unrated, unmetered_threads: 0 },
      ...(end!==null && amount!==null && amount>0 && now-start>=6*HOUR_MS ? {forecast:{basis:'period' as const,percent_at_end:sum/amount*100*(end-start)/(now-start),runs_out_at:sum>0 && sum/amount*(end-start)/(now-start)>1 ? new Date(Math.max(now,start+(now-start)/(sum/amount))).toISOString() : null}} : {}),
      used: decimalText(sum),
      waiting: 0,
      state: amount !== null && sum >= amount ? 'reached' : warn !== null && sum >= warn ? 'warning' : 'ok',
      period_ends_at: end === null ? null : new Date(end).toISOString(),
    };
  }

  /** The rows that show a cap: one for a cap that is shared; for one counted per person, one for each who used any of it, or one for nobody. */
  private rowsOf(cap: CapRecord, now: number): CapStatus[] {
    if (cap.split === 'shared' || cap.meter === 'running') return [this.statusOf(cap, null, now)];
    const start = windowStart(cap.period ?? 'total', now, cap.counted_from);
    const users = [...new Set(this.rows.filter((row) => row.counted && Date.parse(row.at) >= start && this.covered(cap, row, null)).map((row) => row.user))].filter((one): one is string => one !== null);
    return users.length === 0 ? [this.statusOf(cap, null, now)] : users.map((user) => this.statusOf(cap, user, now));
  }

  private canEdit(caller: Person, cap: Pick<UsageCap, 'node' | 'provider'>): boolean {
    if (cap.provider !== null) {
      const answer = this.world.resources.handle(caller, 'GET', `/providers/${cap.provider.id}`, new URLSearchParams(), {});
      const access = isRecord(answer?.body) && isRecord(answer.body.access) ? stringsOf(answer.body.access.permissions) : [];
      return access.includes('provider.manage');
    }
    if (cap.node !== null) return this.world.places.access(caller, cap.node.id).permissions.includes('usage.manage');
    return caller.me.is_system_admin;
  }

  /** The caller's view of a status: whether they may change the cap or let a turn past it. */
  private shown(caller: Person, status: CapStatus): CapStatus {
    const can = this.canEdit(caller, status.cap);
    return { ...status, cap: { ...status.cap, can_edit: can }, can_bypass: can && status.cap.node !== null && this.world.places.access(caller, status.cap.node.id).permissions.includes('usage.bypass') };
  }

  private limitsAt(caller: Person, nodeId: string, userId: string | null): CapStatus[] {
    this.world.places.need(caller, nodeId, `${this.world.places.rowOf(nodeId)?.kind ?? 'org'}.view`);
    const now = Date.now();
    const chain = new Set(this.world.places.pathTo(nodeId).map((entry) => entry.id));
    const who = userId === null ? caller.me.username : ([...this.world.people.values()].find((one) => one.me.id === userId)?.me.username ?? caller.me.username);
    // A provider's cap is in reach where the provider has been used.
    const used = new Set(this.rows.filter((row) => this.under(row, nodeId)).map((row) => row.provider));
    return this.caps
      .filter((cap) => cap.enabled && (cap.node === null || chain.has(cap.node.id)) && (cap.provider === null || used.has(cap.provider.id)))
      .filter((cap) => cap.subject.kind === 'everyone' || this.covers(cap, who))
      .map((cap) => this.shown(caller, this.statusOf(cap, cap.split === 'each' && cap.meter !== 'running' ? who : null, now)));
  }

  // ---------------------------------------------------------------- what stands in the way of a turn

  /**
   * The limits that would stand in the way of a turn the caller starts in a thread: the caps that cover it and have
   * no room, each with whom to ask to let it past. None where nothing stands in the way, so that the thread keeps
   * the limits it was given (a provider's own limit, for one).
   */
  threadLimits(caller: Person, threadId: string): ThreadLimits | null {
    const reached = this.limitsAt(caller, threadId, null).filter((status) => status.state === 'reached');
    if (reached.length === 0) return null;
    const limits = reached.map((status) => {
      const holders = [...this.world.people.values()].filter((one) => status.cap.node !== null && this.world.places.access(one, status.cap.node.id).permissions.includes('usage.bypass'));
      return { cap: status, can_bypass: status.can_bypass, ask: holders.slice(0, 5).map((one) => userRef(one.me)), ask_more: Math.max(0, holders.length - 5), passed_by: null, provider_limit: null };
    });
    return { bypass_all: limits.every((one) => one.can_bypass), reached: limits };
  }

  /** Counts a turn that was let past caps, as the caller sent it: each cap named has one more, and the last to do so is the caller. */
  lettingPast(caller: Person, bypass: unknown): void {
    if (!isRecord(bypass)) return;
    for (const id of stringsOf(bypass.caps)) {
      const extra = this.extras.get(id) ?? { bypasses: { count: 0, last: null }, waiting: [] };
      extra.bypasses = { count: extra.bypasses.count + 1, last: { by: userRef(caller.me), at: new Date().toISOString() } };
      this.extras.set(id, extra);
    }
  }

  private capRecord(caller: Person, input: CapInput): CapRecord {
    const node = input.node_id === undefined || input.node_id === null ? null : this.world.places.rowOf(input.node_id);
    if (input.node_id !== undefined && input.node_id !== null && node === undefined) throw new Refusal(404, 'not_found', 'there is no such place');
    const provider = input.provider_id === undefined || input.provider_id === null ? null : this.world.resources.refsOf({ provider: input.provider_id, agent: '', environment: '', model: '', effort: null, directory: '' }).provider;
    if (input.provider_id !== undefined && input.provider_id !== null && provider === null) throw new Refusal(404, 'not_found', 'there is no such provider');
    const subject = input.subject;
    const rolePlace = subject.kind === 'role' && subject.at !== undefined && subject.at !== null ? this.world.places.rowOf(subject.at) : null;
    if (subject.kind === 'role' && subject.at !== undefined && subject.at !== null && rolePlace === undefined) throw new Refusal(404, 'not_found', 'there is no such role place');
    if (subject.kind === 'user' && input.split !== 'shared') invalid('split', 'is `shared` for a cap that names one person: the amount is theirs');
    if (subject.kind === 'role' && (input.split !== 'each' || provider !== null)) invalid('subject', 'a cap for a role is counted for each person, and is not defined on a provider');
    const running = input.meter === 'running';
    if (running) {
      for (const [path, given] of [['period', input.period !== undefined && input.period !== null], ['warn_percent', input.warn_percent !== undefined && input.warn_percent !== null], ['at_limit', input.at_limit !== undefined && input.at_limit !== null], ['uncounted', input.uncounted !== undefined && input.uncounted !== null], ['counted_from', input.counted_from !== undefined && input.counted_from !== null]] as const) {
        if (given) invalid(path, 'a cap on turns running at once has no period, warning, at_limit or uncounted');
      }
    } else if (input.period === undefined || input.period === null) invalid('period', 'is required unless meter is `running`');
    if (input.counted_from !== undefined && input.counted_from !== null && input.period !== 'total') invalid('counted_from', 'is for the period `total`');
    if (input.amount !== null && !/^\d+(\.\d{1,6})?$/.test(input.amount)) invalid('amount', 'is a decimal number of at most 6 places, as text');
    if (input.amount !== null && input.meter !== 'credits' && !/^\d+$/.test(input.amount)) invalid('amount', `is a whole number of ${input.meter === 'tokens' ? 'tokens' : 'turns'}`);
    const warn = input.warn_percent ?? null;
    if (warn !== null && (warn < 1 || warn > 99)) invalid('warn_percent', 'is from 1 to 99');
    const models = [...new Set((input.models ?? []).map((one) => one.trim().toLowerCase()))].sort();
    models.forEach((one, index) => {
      if (one === '' || /\s/.test(one)) invalid(`models[${index}]`, 'holds a name that is empty or has a space in it');
      if (one.startsWith('@') && !/^@[a-z0-9][a-z0-9_-]{0,39}$/.test(one)) invalid(`models[${index}]`, 'a group is written @ and its name: 1 to 40 of a-z, 0-9, - and _, beginning with a letter or a digit');
    });
    const group = subject.kind === 'group' ? this.world.access.groupFor(caller, subject.id) : undefined;
    if (subject.kind === 'group' && group === undefined) invalid('subject.id', 'is not a group of this tree');
    const person = subject.kind === 'user' ? [...this.world.people.values()].find((one) => one.me.id === subject.id) : undefined;
    if (subject.kind === 'user' && person === undefined) invalid('subject.id', 'is not a person');
    this.capSequence += 1;
    const stamp = new Date().toISOString();
    return {
      id: fixtureId(0x33, this.capSequence),
      node: node === undefined || node === null ? null : { id: node.id, kind: node.kind, name: node.name, redacted: false },
      provider,
      subject:
        subject.kind === 'everyone'
          ? { kind: 'everyone' }
          : subject.kind === 'role'
            ? { kind: 'role', role: subject.role, ...(rolePlace === null || rolePlace === undefined ? {} : { at: this.place(caller, rolePlace.id) }) }
            : subject.kind === 'user'
              ? { kind: 'user', user: userRef(person!.me) }
              : { kind: 'group', group: group! },
      subjectId: subject.kind === 'user' || subject.kind === 'group' ? subject.id : null,
      split: input.split,
      models,
      tiers: [...new Set(input.tiers ?? [])].sort(),
      harness_kinds: [...new Set(input.harness_kinds ?? [])].sort(),
      meter: input.meter,
      period: running ? null : (input.period ?? null),
      counted_from: input.counted_from ?? null,
      amount: input.amount === null ? null : decimalText(Number(input.amount)),
      at_limit: input.at_limit ?? 'stop',
      warn_percent: warn,
      uncounted: input.uncounted ?? 'allow',
      note: input.note ?? '',
      enabled: true,
      revision: 1,
      created_by: userRef(caller.me),
      created_at: stamp,
      updated_at: stamp,
      can_edit: true,
    };
  }

  private sameCap(one: CapRecord, other: CapRecord): boolean {
    return (
      one.node?.id === other.node?.id &&
      one.provider?.id === other.provider?.id &&
      one.subject.kind === other.subject.kind &&
      one.subjectId === other.subjectId &&
      (one.subject.kind !== 'role' || (other.subject.kind === 'role' && one.subject.role === other.subject.role)) &&
      one.split === other.split &&
      one.models.join() === other.models.join() &&
      one.tiers.join() === other.tiers.join() &&
      one.harness_kinds.join() === other.harness_kinds.join() &&
      one.meter === other.meter &&
      one.period === other.period
    );
  }

  private mayChange(caller: Person, input: Pick<UsageCap, 'node' | 'provider'>): void {
    if (this.canEdit(caller, input)) return;
    const need = input.provider !== null ? 'provider.manage' : input.node !== null ? 'usage.manage' : 'system administrator';
    if (input.node !== null && input.provider === null) this.world.places.need(caller, input.node.id, 'usage.manage');
    throw new Refusal(403, 'authz.denied', `${need} is required`, { required: need, object: { kind: input.provider !== null ? 'provider' : 'org', id: input.provider?.id ?? input.node?.id ?? caller.me.id }, requestable: false, override: false, missing: [] });
  }

  // ---------------------------------------------------------------- rates and the catalogue

  private rateTable(caller: Person, nodeId: string): Answer {
    this.world.places.need(caller, nodeId, 'usage.view');
    const since = Date.now() - 30 * DAY_MS;
    const models = this.world.resources.providers.flatMap((provider) =>
      provider.models.map((model) => {
        const found = this.rateFor(model.model_name, provider.id);
        const unrated = found.rate === null ? this.rows.filter((row) => row.provider === provider.id && row.model === model.model_name && Date.parse(row.at) >= since).reduce((total, row) => total + totalOf(row), 0) : 0;
        return { provider: this.world.resources.refsOf({ provider: provider.id, agent: '', environment: '', model: '', effort: null, directory: '' }).provider!, model_name: model.model_name, display_name: model.display_name === '' ? model.model_name : model.display_name, rate: found.rate, source: found.source, match: found.match, unrated_tokens: unrated };
      }),
    );
    // A model that was used and that no provider lists is a row of its own: the one the ledger holds without a rate.
    const listed = new Set(models.map((entry) => `${entry.provider.id}/${entry.model_name}`));
    for (const row of this.rows) {
      const key = `${row.provider}/${row.model}`;
      if (listed.has(key) || row.credits !== null) continue;
      listed.add(key);
      const provider = this.world.resources.refsOf({ provider: row.provider, agent: '', environment: '', model: '', effort: null, directory: '' }).provider;
      if (provider === null) continue;
      const found = this.rateFor(row.model, row.provider);
      models.push({ provider, model_name: row.model, display_name: row.model, rate: found.rate, source: found.source, match: found.match, unrated_tokens: this.rows.filter((one) => one.provider === row.provider && one.model === row.model && Date.parse(one.at) >= since).reduce((total, one) => total + totalOf(one), 0) });
    }
    models.sort((one, other) => other.unrated_tokens - one.unrated_tokens);
    return { status: 200, body: { models, catalogue: { on: this.estimates, scale: this.scale, as_of: this.noCatalogue ? '' : this.catalogue.as_of } } };
  }

  private uploaded(caller: Person, body: Record<string, unknown>): Answer {
    if (!caller.me.is_system_admin) throw new Refusal(403, 'authz.denied', 'only a system admin does this', { required: 'system administrator', object: { kind: 'user', id: caller.me.id }, requestable: false, override: false, missing: [] });
    if (body.shipped === true) {
      this.catalogue = structuredClone(SHIPPED);
      return { status: 200, body: this.catalogue };
    }
    if (body.fetch === true) {
      if (this.fetchUrl === null) invalid('fetch', 'no address is set to take the catalogue from');
      return { status: 200, body: this.catalogue };
    }
    if (body.format !== 1) invalid('format', 'is not a format this release reads');
    const families = Array.isArray(body.families) ? body.families : invalid('families', 'is a list of families');
    const kept: RateFamily[] = [];
    (families as unknown[]).forEach((entry, index) => {
      const at = `families[${index}]`;
      if (!isRecord(entry) || typeof entry.match !== 'string' || entry.match === '') invalid(`${at}.match`, `Entry ${index + 1}: match is a pattern`);
      const rate = (entry as Record<string, unknown>).rate;
      for (const member of ['input_per_mtok', 'cached_input_per_mtok', 'output_per_mtok']) {
        if (!isRecord(rate) || typeof rate[member] !== 'string' || Number.isNaN(Number(rate[member]))) invalid(`${at}.rate.${member}`, `Entry ${index + 1}: ${member} is not a number. Nothing was changed.`);
      }
      kept.push(entry as RateFamily);
    });
    this.catalogue = {
      format: 1,
      as_of: asText(body.as_of),
      unit: asText(body.unit) === '' ? SHIPPED.unit : asText(body.unit),
      origin: 'uploaded',
      source: null,
      loaded_at: new Date().toISOString(),
      loaded_by: userRef(caller.me),
      shipped_as_of: SHIPPED_AS_OF,
      families: kept,
    };
    return { status: 200, body: this.catalogue };
  }

  // ---------------------------------------------------------------- the settings that decide how credits are counted

  /** The usage-specific values composed into the complete node-settings answer. */
  private settingValue(key: SettingKey): unknown {
    return key === 'usage.rates' ? this.card : key === 'usage.catalogue' ? this.estimates : this.scale;
  }

  /** The three settings as they resolve at a node: an org holds the rows, everything inside it follows. */
  private settingsAt(caller: Person, nodeId: string): ResolvedSetting[] {
    // GET is admitted by the complete settings handler's node-view check;
    // writeSetting separately keeps settings.manage and revision guards.
    const org = this.world.places.rowOf(nodeId)?.kind === 'org';
    const writable = org && this.world.places.access(caller, nodeId).permissions.includes('settings.manage');
    return SETTING_KEYS.map((key) => {
      const written = this.revisions[key] > 0;
      return {
        key,
        value: this.settingValue(key),
        own: org && written ? { value: this.settingValue(key), revision: this.revisions[key], locked: false } : null,
        source: { scope: written ? 'org' : 'default', node: null },
        parts: [],
        locked_by: null,
        writable,
      };
    });
  }

  private writeSetting(caller: Person, nodeId: string, key: SettingKey, body: Record<string, unknown>): Answer {
    this.world.places.need(caller, nodeId, 'settings.manage');
    if (this.world.places.rowOf(nodeId)?.kind !== 'org') invalid('id', 'this setting is written for an org as a whole');
    if (typeof body.expected_revision === 'number' && body.expected_revision !== this.revisions[key]) throw new Refusal(409, 'conflict.stale_revision', 'the setting changed since it was read', {});
    const value = body.value;
    if (key === 'usage.rates') {
      const given = isRecord(value) ? value : invalid('value', 'is a map of patterns to rates');
      const card: Record<string, Rate> = {};
      for (const [pattern, rate] of Object.entries(given)) {
        if (pattern === '' || /\s|\//.test(pattern) || pattern !== pattern.toLowerCase()) invalid(`value.${pattern}`, `${pattern}: a pattern is a model name in lower case`);
        for (const member of ['input_per_mtok', 'cached_input_per_mtok', 'output_per_mtok']) {
          if (!isRecord(rate) || typeof rate[member] !== 'string' || !/^\d+(\.\d{1,6})?$/.test(rate[member])) invalid(`value.${pattern}.${member}`, `${pattern}: ${member} is not a number`);
        }
        card[pattern] = rate as Rate;
      }
      this.card = card;
    } else if (key === 'usage.catalogue') {
      if (typeof value !== 'boolean') invalid('value', 'is true or false');
      this.estimates = value as boolean;
    } else {
      if (typeof value !== 'string' || !/^\d+(\.\d{1,6})?$/.test(value) || Number(value) <= 0) invalid('value', 'is a number above 0');
      this.scale = value as string;
    }
    this.revisions[key] += 1;
    return { status: 200, body: this.settingsAt(caller, nodeId).find((one) => one.key === key) };
  }

  // ---------------------------------------------------------------- the requests

  /** CSV keeps the request's Accept header instead of the generic part handler's idempotency key. */
  raw(person: Person, request: IncomingMessage, method: string, path: string, query: URLSearchParams): Promise<Answer | null> {
    const accept = request.headers.accept ?? '';
    return Promise.resolve(method === 'GET' && (path === '/usage' || path === '/usage/events') && accept.includes('text/csv')
      ? this.handle(person,method,path,query,{},accept) : null);
  }

  /** Answers a request about usage, or `null` when it is not one. */
  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>, accept = ''): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    const now = Date.now();

    if (route === 'GET /usage/health') return this.health(person, query.get('provider_id') ?? '');
    if (route === 'GET /usage') return this.analytics(person, query, accept);
    if (route === 'GET /usage/events') return this.events(person, query, accept);
    if (route === 'GET /usage/rates' && query.get('system') === 'true') {
      if (!person.me.is_system_admin) throw new Refusal(403,'authz.denied','a system administrator is required');
      return { status:200,body:{ models:[],catalogue:{ on:this.estimates,scale:this.scale,as_of:this.catalogue.as_of } } };
    }
    if (route === 'GET /usage/rates') return this.rateTable(person, query.get('node_id') ?? '');
    if (route === 'GET /usage/catalogue') {
      if (this.noCatalogue) return { status: 200, body: null };
      const q = query.get('q');
      if (q === null || q === '') return { status: 200, body: this.catalogue };
      const family = this.familyOf(q);
      return { status: 200, body: { ...this.catalogue, families: family === null ? [] : [family] } };
    }
    if (route === 'POST /admin/usage/catalogue') return this.uploaded(person, body);
    if ((found = /^\/nodes\/([^/]+)\/settings$/.exec(path)) !== null && method === 'GET') {
      const complete = this.world.peopleWork.handle(person, method, path, query, body);
      if (complete === null || !Array.isArray(complete.body)) throw new Error('The complete node-settings fixture did not answer its route');
      const rows = complete.body as ResolvedSetting[];
      return { ...complete, body: [...rows.filter((row) => !SETTING_KEYS.some((key) => row.key === key)), ...this.settingsAt(person, found[1]!)] };
    }
    if ((found = /^\/nodes\/([^/]+)\/settings\/(usage\.[a-z_]+)$/.exec(path)) !== null && method === 'PUT' && SETTING_KEYS.includes(found[2] as SettingKey)) return this.writeSetting(person, found[1]!, found[2] as SettingKey, body);
    if (route === 'GET /usage/limits') return { status: 200, body: { caps: this.limitsAt(person, query.get('node_id') ?? '', query.get('user_id')) } };
    if (route === 'GET /me/limits') {
      const mine = this.caps.filter((cap) => cap.enabled && cap.split === 'each' && cap.meter !== 'running' && this.covers(cap, person.me.username));
      return { status: 200, body: mine.map((cap) => this.shown(person, this.statusOf(cap, person.me.username, now))) };
    }
    if (route === 'GET /usage/caps') {
      const nodeId = query.get('node_id');
      const providerId = query.get('provider_id');
      if (query.get('system') === 'true') {
        if (!person.me.is_system_admin) this.mayChange(person, { node: null, provider: null });
        return { status: 200, body: this.caps.filter((cap) => cap.node === null && cap.provider === null).flatMap((cap) => this.rowsOf(cap, now)).map((status) => this.shown(person, status)) };
      }
      if (providerId !== null) {
        this.world.resources.handle(person, 'GET', `/providers/${providerId}`, new URLSearchParams(), {});
        return { status: 200, body: this.caps.filter((cap) => cap.provider?.id === providerId).flatMap((cap) => this.rowsOf(cap, now)).map((status) => this.shown(person, status)) };
      }
      if (nodeId !== null) {
        this.world.places.need(person, nodeId, 'usage.view');
        const chain = new Set(this.world.places.pathTo(nodeId).map((entry) => entry.id));
        const wanted = this.caps.filter((cap) => (cap.node?.id === nodeId && cap.provider === null) || (query.get('covering') === 'true' && cap.enabled && (cap.node === null || chain.has(cap.node.id)) && !(cap.node?.id === nodeId && cap.provider === null)));
        return { status: 200, body: wanted.flatMap((cap) => this.rowsOf(cap, now)).map((status) => this.shown(person, status)) };
      }
      return invalid('node_id', 'exactly one of node_id, provider_id and system is wanted');
    }
    if (route === 'POST /usage/caps') {
      const input = body as unknown as CapInput;
      const made = this.capRecord(person, input);
      this.mayChange(person, made);
      if (this.caps.some((cap) => this.sameCap(cap, made))) throw new Refusal(409, 'conflict.exists', 'a cap of this kind exists already');
      this.caps.push(made);
      return { status: 201, body: { ...publicCap(made), can_edit: true } };
    }
    if ((found = /^(PATCH|DELETE) \/usage\/caps\/([^/]+)$/.exec(route))) {
      const cap = this.caps.find((candidate) => candidate.id === found?.[2]);
      if (cap === undefined) throw new Refusal(404, 'not_found', 'there is no such cap');
      this.mayChange(person, cap);
      if (found[1] === 'DELETE') {
        this.caps = this.caps.filter((candidate) => candidate !== cap);
        this.extras.delete(cap.id);
        return { status: 204 };
      }
      if (body.expected_revision !== cap.revision) throw new Refusal(409, 'conflict.stale_revision', 'the cap changed since it was read', { current: cap });
      if ('amount' in body) {
        const amount = body.amount === null ? null : asText(body.amount);
        if (amount !== null && !/^\d+(\.\d{1,6})?$/.test(amount)) invalid('amount', 'is a decimal number of at most 6 places, as text');
        if (amount !== null && cap.meter !== 'credits' && !/^\d+$/.test(amount)) invalid('amount', `is a whole number of ${cap.meter === 'tokens' ? 'tokens' : 'turns'}`);
        cap.amount = amount === null ? null : decimalText(Number(amount));
      }
      if (Array.isArray(body.models)) cap.models = [...new Set(stringsOf(body.models).map((one) => one.trim().toLowerCase()))].sort();
      if (Array.isArray(body.tiers)) cap.tiers = [...new Set(stringsOf(body.tiers))].sort();
      if (Array.isArray(body.harness_kinds)) cap.harness_kinds = [...new Set(stringsOf(body.harness_kinds))].sort();
      if (body.at_limit === 'stop' || body.at_limit === 'finish_turn') cap.at_limit = body.at_limit;
      if (body.uncounted === 'allow' || body.uncounted === 'block') cap.uncounted = body.uncounted;
      if ('warn_percent' in body) {
        const warn = body.warn_percent === null ? null : Number(body.warn_percent);
        if (warn !== null && (warn < 1 || warn > 99)) invalid('warn_percent', 'is from 1 to 99');
        cap.warn_percent = warn;
      }
      if ('counted_from' in body) cap.counted_from = body.counted_from === null ? null : asText(body.counted_from);
      if (typeof body.note === 'string') cap.note = body.note;
      if (typeof body.enabled === 'boolean') cap.enabled = body.enabled;
      cap.revision += 1;
      cap.updated_at = new Date().toISOString();
      return { status: 200, body: { ...publicCap(cap), can_edit: true } };
    }
    if ((found = /^GET \/usage\/caps\/([^/]+)\/waiting$/.exec(route))) {
      const cap = this.caps.find((candidate) => candidate.id === found?.[1]);
      if (cap === undefined) throw new Refusal(404, 'not_found', 'there is no such cap');
      const waiting = this.extras.get(cap.id)?.waiting ?? [];
      return {
        status: 200,
        body: { items: waiting.map((entry) => ({ thread: entry.thread === null ? null : this.place(person, entry.thread), author: this.person(entry.author), since: entry.since, mine: entry.author === person.me.username })) },
      };
    }
    return null;
  }

  /** What only a test can do: write more history, add a use, say what runs and what waits, and say that a turn was let past a cap. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/history':
        this.history(typeof body.days === 'number' ? body.days : HISTORY_DAYS, Date.now());
        return { status: 204 };
      case '/use': {
        // One more use, now: it is counted by the caps at once.
        const pattern = PATTERNS.find((candidate) => candidate.name === asText(body.pattern)) ?? PATTERNS[0];
        if (pattern === undefined) return { status: 400, body: { detail: 'no pattern' } };
        const scale = typeof body.scale === 'number' ? body.scale : 1;
        const row = this.write({ ...pattern, tokens: { input: Math.round(pattern.tokens.input * scale), cached: Math.round(pattern.tokens.cached * scale), output: Math.round(pattern.tokens.output * scale), reasoning: Math.round(pattern.tokens.reasoning * scale) } }, Date.now(), body.user === null ? null : typeof body.user === 'string' ? body.user : pattern.user);
        row.program = typeof body.program === 'string' ? body.program : row.program;
        if (body.performance === true) { row.timing = timingFixture(); row.tokens = {input:100,cached:20,output:64,reasoning:0}; row.credits = row.rate === null ? null : this.creditsOf(row.tokens,row.rate,row.tier,row.context); }
        if (body.running === true) { row.running = true; row.tokens = {input:0,cached:0,output:0,reasoning:0}; row.credits = null; row.timing = timingFixture({ outcome: { state: 'unread' }, speed_milli: { state: 'unread' }, streamed_tokens: {state:'unread'}, first_token_ms: { state: 'unread' }, request_ms: { state: 'unread' }, generation_ms: { state: 'unread' }, reported_tokens: null }); }
        this.tellRequests();
        return { status: 200, body: { id: row.id } };
      }
      case '/performance': {
        const pattern = PATTERNS[0];
        if (pattern === undefined) return { status: 400 };
        const now = Date.now();
        // Known values make the real screen's medians, sorting, missing state and error count reviewable.
        for (const [model, speed, first] of [['fast-model', 64_000, 512], ['slow-model', 32_000, 1024]] as const) {
          for (let index = 0; index < 22; index += 1) {
            const row = this.write({ ...pattern, model, tokens: { input: 100, cached: 20, output: 64, reasoning: 0 } }, now - index * 10, pattern.user);
            row.timing = timingFixture({ speed_milli: { state: 'told', value: speed }, first_token_ms: { state: 'told', value: first } });
          }
        }
        const hidden = this.write({ ...pattern, model: 'hidden-model', tokens:{input:100,cached:20,output:64,reasoning:0} }, now, pattern.user);
        hidden.timing = timingFixture({ reported_tokens:{input_tokens:100,cached_input_tokens:20,cache_write_tokens:null,output_tokens:64,reasoning_tokens:null,total_tokens:164},reasoning_hidden: true, speed_milli: { state: 'unsupported' }, streamed_tokens: { state: 'unsupported' } });
        for (const code of ['provider_busy', 'forced']) {
          const row = this.write({ ...pattern, model: 'fast-model',tokens:{input:100,cached:20,output:10,reasoning:0} }, now, pattern.user);
          row.timing = timingFixture({ reported_tokens:{input_tokens:100,cached_input_tokens:20,cache_write_tokens:null,output_tokens:10,reasoning_tokens:0,total_tokens:110},streamed_tokens:{state:'told',value:10},generation_ms:{state:'told',value:100},request_ms:{state:'told',value:612},outcome: { state: 'told', value: code }, error_message: code === 'forced' ? 'Cancelled by a person' : 'Try again later', speed_milli: { state: 'empty' } });
        }
        this.tellRequests();
        return { status: 204 };
      }
      case '/running':
        this.running = Array.isArray(body.turns) ? (body.turns as typeof this.running) : [];
        return { status: 204 };
      case '/waiting': {
        const extra = this.extras.get(asText(body.cap)) ?? { bypasses: { count: 0, last: null }, waiting: [] };
        extra.waiting = Array.isArray(body.items) ? (body.items as CapExtras['waiting']) : [];
        this.extras.set(asText(body.cap), extra);
        return { status: 204 };
      }
      case '/bypass': {
        const extra = this.extras.get(asText(body.cap)) ?? { bypasses: { count: 0, last: null }, waiting: [] };
        const by = this.person(asText(body.by));
        extra.bypasses = { count: typeof body.count === 'number' ? body.count : extra.bypasses.count + 1, last: by === null ? null : { by, at: new Date().toISOString() } };
        this.extras.set(asText(body.cap), extra);
        return { status: 204 };
      }
      case '/catalogue':
        if (typeof body.estimates === 'boolean') this.estimates = body.estimates;
        if (typeof body.scale === 'string') this.scale = body.scale;
        if (typeof body.none === 'boolean') this.noCatalogue = body.none;
        if ('fetch_url' in body) this.fetchUrl = typeof body.fetch_url === 'string' ? body.fetch_url : null;
        return { status: 204 };
      case '/rows':
        return { status: 200, body: this.rows };
      default:
        return null;
    }
  }

}
