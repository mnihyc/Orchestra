/** What the address of a usage screen says and what it comes to: the scope asked about, the span, what the chart shows and how it is split, which table is open and what the whole screen is filtered by.
 *
 * Pure functions, so that a link reproduces the screen and the screen can be tested without drawing it.
 * Days are written as the API writes them, `2026-10-01`, and counted in UTC, which is the clock the
 * ledger is kept by. Explicit address choices take precedence over the device's remembered view,
 * including a choice that matches the initial default.
 */
import type { EventParams, UsageFilters, UsageParams } from '@/api/usage';
import type { UsageBucket, UsageDepth, UsageDimension, UsageEnvironmentKind, UsageOutcomeFilter } from '@/generated/api-types';
import { readOutcome } from '@/features/usage/outcomes';
import { DAY_MS } from '@/lib/time';

// ---------------------------------------------------------------- the scope

/** What the figures are about: one's own use, a place and everything below it, or what ran on a provider. */
export type UsageScope = { kind: 'me' } | { kind: 'node'; id: string } | { kind: 'provider'; id: string };

/** An identifier: the shape every node, provider and person has. */
const IDENTIFIER = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The scope as the API names it: `me`, `node:<id>` or `provider:<id>`. */
export function scopeText(scope: UsageScope): string {
  return scope.kind === 'me' ? 'me' : `${scope.kind}:${scope.id}`;
}

/** The scope a text names; one's own use for a text that names none. */
export function parseScope(text: unknown): UsageScope {
  if (typeof text !== 'string') return { kind: 'me' };
  const at = text.indexOf(':');
  if (at < 0) return { kind: 'me' };
  const kind = text.slice(0, at);
  const id = text.slice(at + 1);
  if ((kind === 'node' || kind === 'provider') && IDENTIFIER.test(id)) return { kind, id };
  return { kind: 'me' };
}

/** Whether two scopes are the same. */
export function sameScope(one: UsageScope, other: UsageScope): boolean {
  return scopeText(one) === scopeText(other);
}

// ---------------------------------------------------------------- what the address holds

/** The spans a person picks from, besides a span of their own. */
export const RANGES = ['7d', 'last_week', '30d', '90d', 'year', 'all'] as const;
export type Range = (typeof RANGES)[number];

/** What the chart measures. */
export const MEASURES = ['turns', 'requests', 'tokens', 'credits', 'speed', 'first_token', 'errors'] as const;
export type Measure = (typeof MEASURES)[number];

/** What the chart can be split by, besides nothing. */
export type Split = UsageDimension | 'token_kind' | 'none';
export type ChartBucket = UsageBucket | 'auto';

/** The thirty days the screen opens on, in credits, split by model. */
export const DEFAULT_RANGE: Range = '30d';
export const DEFAULT_MEASURE: Measure = 'credits';
export const DEFAULT_SPLIT: Split = 'model';

/**
 * The earliest day a span can begin on: the ledger is kept for good and reaches back to the first turn, which
 * cannot be before this system was written. "All time" begins here, and the chart leaves out the quiet stretch
 * before the first use.
 */
export const ALL_TIME_FROM = '2020-01-01';

/** The filters an address can carry, by the word it writes them with, and the member of the request each becomes. */
export const FILTERS = {
  effort: 'effort',
  tier: 'tier',
  program: 'program',
  outcome: 'outcome',
  turn: 'turn_id',
  org: 'org_id',
  topic: 'topic_id',
  person: 'user_id',
  model: 'model',
  provider: 'provider_id',
  agent: 'harness_kind',
  environment: 'environment_kind',
  project: 'project_id',
  thread: 'thread_id',
} as const satisfies Record<string, keyof UsageFilters>;

export type FilterName = keyof typeof FILTERS;

/** The dimension each filter narrows to one member of, which is the table whose row sets it. */
export const FILTER_DIMENSION: Record<FilterName, UsageDimension | undefined> = {
  effort: 'effort', tier: 'tier', program: 'program', outcome: undefined, turn: undefined, org: 'org', topic: 'topic',
  person: 'person',
  model: 'model',
  provider: 'provider',
  agent: 'agent',
  environment: 'environment',
  project: 'project',
  thread: 'thread',
};

export interface UsageSearch {
  view?: 'usage' | 'performance' | 'requests';
  plan?: 'current' | 'history';
  plan_bucket?: ChartBucket;
  effort?: string;
  tier?: string;
  program?: string;
  outcome?: UsageOutcomeFilter;
  turn?: string;
  org?: string;
  topic?: string;
  /** `me` (left out), `node:<id>` or `provider:<id>`. */
  scope?: string;
  /** One of the spans; the thirty days when left out and no day is named. */
  range?: Range;
  /** A span of one's own: the first and the last day. */
  from?: string;
  to?: string;
  /** How the span is cut; the server decides by its length when left out. */
  bucket?: ChartBucket;
  /** What the chart measures; credits when left out. */
  show?: Measure;
  /** What the chart is split by; the model when left out. */
  series?: Split;
  /** Which table is open; the first when left out. */
  by?: UsageDimension;
  person?: string;
  model?: string;
  provider?: string;
  agent?: string;
  environment?: UsageEnvironmentKind;
  project?: string;
  thread?: string;
  /** The tables list up to a hundred rows instead of ten. */
  more?: true;
}

const BUCKETS: readonly UsageBucket[] = ['hour', 'day', 'week', 'month'];
const DIMENSIONS: readonly UsageDimension[] = ['person', 'model', 'provider', 'agent', 'environment', 'effort', 'tier', 'program', 'org', 'project', 'topic', 'thread'];
const SPLITS: readonly Split[] = [...DIMENSIONS, 'token_kind', 'none'];

function oneOf<Value extends string>(allowed: readonly Value[], value: unknown): Value | undefined {
  return allowed.find((candidate) => candidate === value);
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

/** A day written as `YYYY-MM-DD` that is a day; nothing for any other text. */
export function readDay(value: unknown): string | undefined {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return undefined;
  const at = Date.parse(`${value}T00:00:00Z`);
  return Number.isNaN(at) || new Date(at).toISOString().slice(0, 10) !== value ? undefined : value;
}

/** What an address says, keeping explicit defaults and leaving unsupported values out. */
export function readSearch(raw: Record<string, unknown>): UsageSearch {
  const scope = scopeText(parseScope(raw.scope));
  const from = readDay(raw.from);
  const to = readDay(raw.to);
  const range = oneOf(RANGES, raw.range);
  const bucket = oneOf<ChartBucket>([...BUCKETS,'auto'], raw.bucket);
  const planBucket=oneOf<ChartBucket>([...BUCKETS,'auto'],raw.plan_bucket);
  const show = oneOf(MEASURES, raw.show);
  const series = oneOf(SPLITS, raw.series);
  const by = oneOf(DIMENSIONS, raw.by);
  const environment = oneOf<UsageEnvironmentKind>(['managed', 'connected'], raw.environment);
  const person = IDENTIFIER.test(text(raw.person) ?? '') ? text(raw.person) : undefined;
  const provider = IDENTIFIER.test(text(raw.provider) ?? '') ? text(raw.provider) : undefined;
  const project = IDENTIFIER.test(text(raw.project) ?? '') ? text(raw.project) : undefined;
  const thread = IDENTIFIER.test(text(raw.thread) ?? '') ? text(raw.thread) : undefined;
  // A span of one's own wins over a named one; a half of one is completed by the other end of the default.
  const custom = from !== undefined || to !== undefined;
  return {
    ...(raw.view === 'usage' || raw.view === 'performance' || raw.view === 'requests' ? { view: raw.view } : {}),
    ...(raw.plan === 'current' || raw.plan === 'history' ? { plan: raw.plan } : {}),
    ...(planBucket===undefined ? {} : {plan_bucket:planBucket}),
    ...Object.fromEntries(['effort', 'tier', 'program', 'org', 'topic', 'turn'].flatMap((key) => text(raw[key]) === undefined ? [] : [[key, text(raw[key])]])),
    ...(readOutcome(raw.outcome) === undefined ? {} : { outcome: readOutcome(raw.outcome) }),
    ...(scope === 'me' ? {} : { scope }),
    ...(custom ? { ...(from === undefined ? {} : { from }), ...(to === undefined ? {} : { to }) } : range === undefined ? {} : { range }),
    ...(bucket === undefined ? {} : { bucket }),
    ...(show === undefined ? {} : { show }),
    ...(series === undefined ? {} : { series }),
    ...(by === undefined ? {} : { by }),
    ...(person === undefined ? {} : { person }),
    ...(text(raw.model) === undefined ? {} : { model: text(raw.model) }),
    ...(provider === undefined ? {} : { provider }),
    ...(text(raw.agent) === undefined ? {} : { agent: text(raw.agent) }),
    ...(environment === undefined ? {} : { environment }),
    ...(project === undefined ? {} : { project }),
    ...(thread === undefined ? {} : { thread }),
    ...(raw.more === true || raw.more === 'true' ? { more: true as const } : {}),
  };
}

/** The search with a change laid over it, keeping explicit defaults and validating every value. */
export function patchSearch(search: UsageSearch, patch: { [Member in keyof UsageSearch]?: UsageSearch[Member] | undefined }): UsageSearch {
  // An undefined member removes that choice; the remaining explicit choices retain precedence on a refresh.
  const merged: Record<string, unknown> = { ...search, ...patch };
  if(patch.view!==undefined && patch.view!==search.view) {
    // Each chart resumes its own device choice when the view changes.
    if(patch.series===undefined) merged.series=undefined;
    if(patch.bucket===undefined) merged.bucket=undefined;
    if(patch.show===undefined) merged.show=undefined;
  }
  // Choosing one of the spans lets go of a span of one's own, and the other way about.
  if (patch.range !== undefined) {
    merged.from = undefined;
    merged.to = undefined;
  }
  if (patch.from !== undefined || patch.to !== undefined) merged.range = undefined;
  return readSearch(merged);
}

// ---------------------------------------------------------------- the span

export interface Span {
  /** The first and the last day, UTC. */
  from: string;
  to: string;
  /** How many days that is. */
  days: number;
  /** Which span it is, or that it is one of the person's own. */
  range: Range | 'custom';
}

const dayOf = (ms: number): string => new Date(ms).toISOString().slice(0, 10);
const msOf = (day: string): number => Date.parse(`${day}T00:00:00Z`);
const countDays = (from: string, to: string): number => Math.round((msOf(to) - msOf(from)) / DAY_MS) + 1;

/** Today in UTC, as a day. */
export function today(now: number): string {
  return dayOf(now);
}

/** The days a search names, as of `now`. */
export function spanOf(search: UsageSearch, now: number): Span {
  const last = today(now);
  if (search.from !== undefined || search.to !== undefined) {
    const to = search.to ?? last;
    const from = search.from ?? (search.to === undefined ? last : dayOf(msOf(to) - 29 * DAY_MS));
    // Two ends the wrong way round are the same span read the right way.
    const [first, second] = from <= to ? [from, to] : [to, from];
    return { from: first, to: second, days: countDays(first, second), range: 'custom' };
  }
  const range = search.range ?? DEFAULT_RANGE;
  if (range === 'last_week') {
    const monday = msOf(last) - ((new Date(msOf(last)).getUTCDay() + 6) % 7) * DAY_MS;
    return { from: dayOf(monday - 7 * DAY_MS), to: dayOf(monday - DAY_MS), days: 7, range };
  }
  const begin = (daysBack: number): string => dayOf(msOf(last) - daysBack * DAY_MS);
  const from =
    range === '7d' ? begin(6) : range === '30d' ? begin(29) : range === '90d' ? begin(89) : range === 'year' ? `${last.slice(0, 4)}-01-01` : ALL_TIME_FROM;
  return { from, to: last, days: countDays(from, last), range };
}

/** The span of the same length that ends the day before this one begins; none for all time, which has nothing before it. */
export function spanBefore(span: Span): Span | null {
  if (span.range === 'all') return null;
  const to = dayOf(msOf(span.from) - DAY_MS);
  const from = dayOf(msOf(to) - (span.days - 1) * DAY_MS);
  return { from, to, days: span.days, range: 'custom' };
}

/** The days from a moment in milliseconds to another, as a span of one's own: what dragging across the chart sets. */
export function spanBetween(fromMs: number, toMs: number): { from: string; to: string } {
  const [first, second] = fromMs <= toMs ? [fromMs, toMs] : [toMs, fromMs];
  return { from: dayOf(first), to: dayOf(second) };
}

// ---------------------------------------------------------------- how a span is cut

/** The limits the API holds a bucket to, and the lengths it chooses by: the contract's own. */
export const BUCKET_LIMITS = { hourMostDays: 31, dayMostDays: 800, hourUpToDays: 2, dayUpToDays: 92, weekUpToDays: 730 } as const;

/** The bucket the server chooses for a span of this many days when none is asked for. */
export function defaultBucket(days: number): UsageBucket {
  if (days <= BUCKET_LIMITS.hourUpToDays) return 'hour';
  if (days <= BUCKET_LIMITS.dayUpToDays) return 'day';
  if (days <= BUCKET_LIMITS.weekUpToDays) return 'week';
  return 'month';
}

/** The buckets a span of this many days can be cut into. */
export function bucketsFor(days: number): UsageBucket[] {
  return BUCKETS.filter((bucket) => (bucket === 'hour' ? days <= BUCKET_LIMITS.hourMostDays : bucket === 'day' ? days <= BUCKET_LIMITS.dayMostDays : true));
}

/** The bucket that is asked for: the person's, when the span allows it; otherwise none, and the server chooses. */
export function bucketAsked(search: UsageSearch, days: number): UsageBucket | undefined {
  return search.bucket !== undefined && search.bucket !== 'auto' && bucketsFor(days).includes(search.bucket) ? search.bucket : undefined;
}

// ---------------------------------------------------------------- the filters

/** What the search narrows the whole screen to, in the members of a request. */
export function filtersOf(search: UsageSearch): UsageFilters {
  return {
    ...(search.effort === undefined ? {} : { effort: search.effort }),
    ...(search.tier === undefined ? {} : { tier: search.tier }),
    ...(search.program === undefined ? {} : { program: search.program }),
    ...(search.outcome === undefined ? {} : { outcome: search.outcome }),
    ...(search.turn === undefined ? {} : { turn_id: search.turn }),
    ...(search.org === undefined ? {} : { org_id: search.org }),
    ...(search.topic === undefined ? {} : { topic_id: search.topic }),
    ...(search.person === undefined ? {} : { user_id: search.person }),
    ...(search.model === undefined ? {} : { model: search.model }),
    ...(search.provider === undefined ? {} : { provider_id: search.provider }),
    ...(search.agent === undefined ? {} : { harness_kind: search.agent }),
    ...(search.environment === undefined ? {} : { environment_kind: search.environment }),
    ...(search.project === undefined ? {} : { project_id: search.project }),
    ...(search.thread === undefined ? {} : { thread_id: search.thread }),
  };
}

/** The filters that are set, in the order the tables are listed. */
export function activeFilters(search: UsageSearch): { name: FilterName; value: string }[] {
  const names: FilterName[] = ['person', 'model', 'provider', 'agent', 'environment', 'effort', 'tier', 'program', 'outcome', 'org', 'project', 'topic', 'thread', 'turn'];
  return names.flatMap((name) => {
    const value = search[name];
    return value === undefined ? [] : [{ name, value }];
  });
}

// ---------------------------------------------------------------- the tables

/**
 * The tables a reader is offered, in order, for what is asked about: the people appear only where there is
 * more than oneself, a place's tables name the places inside it, and a provider's the orgs that used it. A
 * table by thread is the reader's own use, or reports, and is asked for when it is opened.
 */
export function tablesFor(scope: UsageScope, nodeKind: string | null): UsageDimension[] {
  if (scope.kind === 'me') return ['project', 'model', 'provider', 'agent', 'environment', 'tier', 'effort', 'program'];
  if (scope.kind === 'provider') return ['person', 'model', 'org', 'agent', 'environment', 'tier', 'effort', 'program'];
  return ['person', 'model', 'provider', 'agent', 'environment', ...(nodeKind === null || nodeKind === 'org' ? (['org', 'project'] as UsageDimension[]) : nodeKind === 'project' ? (['project'] as UsageDimension[]) : []), 'tier', 'effort', 'program'];
}

/** Whether the tables by thread are offered at this depth: one's own use, a provider's and a place's reports have them, a place's summary has not. */
export function offersThreads(scope: UsageScope, depth: UsageDepth | null): boolean {
  return scope.kind !== 'node' || depth === 'reports' || depth === 'own';
}

/** The dimension a filter can be set from, and the member of the key that carries its value; a dimension without a filter is not a row to click. */
export function filterOf(dimension: UsageDimension): FilterName | null {
  const found = (Object.entries(FILTER_DIMENSION) as [FilterName, UsageDimension][]).find(([, candidate]) => candidate === dimension);
  return found === undefined ? null : found[0];
}

// ---------------------------------------------------------------- the requests

/** The request for the figures of a search: the span, the filters, the chart's split and the tables. */
export function figuresParams(scope: UsageScope, search: UsageSearch, span: Span, tables: readonly UsageDimension[], rows: number): UsageParams {
  const split = search.series ?? DEFAULT_SPLIT;
  return {
    scope: scopeText(scope),
    measure: search.view === 'performance' ? search.show === 'first_token' || search.show === 'errors' ? search.show : 'speed' : search.show === 'tokens' || search.show === 'turns' || search.show === 'requests' ? search.show : undefined,
    from: span.from,
    to: span.to,
    bucket: bucketAsked(search, span.days),
    seriesBy: split === 'none' || split === 'token_kind' ? undefined : split,
    breakdowns: tables,
    limit: rows,
    filters: filtersOf(search),
  };
}

/** The request for the records of a search. */
export function recordsParams(scope: UsageScope, search: UsageSearch, span: Span): EventParams {
  return { scope: scopeText(scope), from: span.from, to: span.to, filters: filtersOf(search), ...(search.view === 'requests' ? { requestsOnly: true, includeRunning: true } : {}) };
}

/** How many rows a table lists: ten, or a hundred once the reader asks for more. */
export const ROWS = { few: 10, many: 100 } as const;
