/** API client for The usage ledger at three depths as figures, series and records, rates and the catalogue of estimates, and caps on tokens, credits and turns at once.
 *
 * What the usage screens ask of the API: the figures of a span for one's own use, a place or a provider
 * (the totals, the series and a breakdown for each dimension in one answer), the records behind them a page
 * at a time, the rates a model would be counted at, the shipped catalogue, and the caps that bound work:
 * what stands at a place, what a person is covered by, and what waits for room in one. A file of the series
 * or of the records is asked for as CSV and handed to the person by the screen.
 *
 * A cap that is made carries the key of the act, so that a second attempt after a lost answer makes nothing
 * twice; a change names the revision the person saw.
 */
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { infiniteQueryOptions, type QueryClient, queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import type {
  AdminUsageCatalogueSetBody,
  CapInput,
  CapStatus,
  Group,
  Limits,
  RateCatalogue,
  RateTable,
  ResolvedSetting,
  UsageAnalytics,
  UsageBucket,
  UsageCap,
  UsageCapUpdateBody,
  UsageCapWaitingListReply,
  UsageDimension,
  UsageEventPage,
  UsageEnvironmentKind,
  UsageOutcomeFilter,
  UsageDegradation,
} from '@/generated/api-types';

// ---------------------------------------------------------------- what is asked about

/** What the figures and the records are limited to, beyond the scope and the span: the ids and names the API filters by. */
export interface UsageFilters {
  effort?: string;
  tier?: string;
  program?: string;
  outcome?: UsageOutcomeFilter;
  org_id?: string;
  turn_id?: string;
  user_id?: string;
  model?: string;
  provider_id?: string;
  harness_kind?: string;
  environment_kind?: UsageEnvironmentKind;
  project_id?: string;
  topic_id?: string;
  thread_id?: string;
}

/** One request for figures. */
export interface UsageParams {
  health?: boolean;
  measure?: 'credits' | 'tokens' | 'turns' | 'requests' | 'speed' | 'first_token' | 'errors' | undefined;
  /** `me`, `node:` and an identifier, or `provider:` and an identifier. */
  scope: string;
  /** The first and the last day, UTC, as `YYYY-MM-DD`. */
  from: string;
  to: string;
  /** Left out, the server cuts the span by its length. */
  bucket?: UsageBucket | undefined;
  seriesBy?: UsageDimension | undefined;
  /** The dimensions to break the totals down by; an empty list asks for none. */
  breakdowns: readonly UsageDimension[];
  /** Rows of each breakdown. */
  limit?: number | undefined;
  filters: UsageFilters;
}

/** One request for records. */
export interface EventParams {
  includeRunning?: boolean;
  requestsOnly?: boolean;
  scope: string;
  from: string;
  to: string;
  filters: UsageFilters;
  counted?: boolean | undefined;
  order?: 'asc' | 'desc' | undefined;
}

/** How many records a page holds: a screenful and a little more, so that the first look is not a stretch of scrolling. */
export const EVENTS_PAGE = 25;

/** How many records one file of them holds at most; the file says so when it is cut. */
export const EXPORT_RECORDS_MOST = 10_000;

/** How many records a request for a file asks for: the most the server gives at once, so that a file takes the fewest requests. */
const RECORDS_PER_REQUEST = 200;

export const usageKeys = {
  analytics: (params: UsageParams) => ['usage', 'figures', params] as const,
  events: (params: EventParams) => ['usage', 'records', params] as const,
  rates: (nodeId: string | null) => ['usage', 'rates', nodeId] as const,
  settings: (nodeId: string | null) => nodeId === null ? ['admin', 'settings'] as const : ['settings', nodeId] as const,
  catalogue: (match: string | null) => ['usage', 'catalogue', match] as const,
  limits: (nodeId: string, userId: string | null) => ['usage', 'limits', nodeId, userId] as const,
  caps: (where: CapPlace) => ['usage', 'caps', capPlaceKey(where)] as const,
  mine: ['usage', 'mine'] as const,
  waiting: (capId: string) => ['usage', 'waiting', capId] as const,
};

/** Where the caps that are listed are defined: on a node, on a provider, or the whole system. */
export type CapPlace = { kind: 'node'; id: string; covering: boolean } | { kind: 'provider'; id: string } | { kind: 'system' };

function capPlaceKey(where: CapPlace): string {
  return where.kind === 'system' ? 'system' : where.kind === 'node' ? `node:${where.id}:${where.covering}` : `provider:${where.id}`;
}

function filterQuery(filters: UsageFilters): UsageFilters {
  // Only what is set goes into the address of the request.
  return Object.fromEntries(Object.entries(filters).filter(([, value]) => value !== undefined && value !== ''));
}

// ---------------------------------------------------------------- figures and records

/** Current provider health without chart, comparison or previous-span reads. */
export const healthQuery = (providerId: string) => queryOptions({
  queryKey: ['usage', 'health', providerId] as const,
  queryFn: ({ signal }): Promise<UsageDegradation> => ok(api.GET('/usage/health', { params: { query: { provider_id: providerId } }, signal })),
  staleTime: 10_000,
});

/** The figures of a span: the totals, the series and a breakdown for each dimension asked for. */
export const usageQuery = (params: UsageParams) =>
  queryOptions({
    queryKey: usageKeys.analytics(params),
    queryFn: ({ signal }): Promise<UsageAnalytics> =>
      ok(
        api.GET('/usage', {
          params: {
            query: {
              scope: params.scope,
              from: params.from,
              to: params.to,
              breakdowns: params.breakdowns.join(','),
              ...(params.measure === undefined ? {} : { measure: params.measure }),
              ...(params.health ? { health: true } : {}),
              ...(params.bucket === undefined ? {} : { bucket: params.bucket }),
              ...(params.seriesBy === undefined ? {} : { series_by: params.seriesBy }),
              ...(params.limit === undefined ? {} : { limit: params.limit }),
              ...filterQuery(params.filters),
            },
          },
          signal,
        }),
      ),
    // A span that ended before today does not change, and today's changes with every turn: a minute is a fair wait.
    staleTime: 60_000,
    // The screen keeps what it showed while the next span is on its way, dimmed.
    placeholderData: (before) => before,
  });

/** The ledger rows behind the figures, newest first, a page at a time. */
export function fetchEventsPage(params: EventParams, cursor: string | null, limit = EVENTS_PAGE, signal?: AbortSignal): Promise<UsageEventPage> {
  return ok(api.GET('/usage/events', { params: { query: { scope: params.scope, from: params.from, to: params.to, limit,
    ...(params.includeRunning ? { include_running: true } : {}), ...(params.requestsOnly ? { requests_only: true } : {}),
    ...(params.counted === undefined ? {} : { counted: params.counted }), ...(params.order === undefined ? {} : { order: params.order }),
    ...(cursor === null ? {} : { cursor }), ...filterQuery(params.filters),
  } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Cursor repair uses this same route when a live snapshot holds more than one page. */
export const eventsQuery = (params: EventParams, pageSize: number = EVENTS_PAGE) =>
  infiniteQueryOptions({
    queryKey: [...usageKeys.events(params), pageSize] as const,
    queryFn: ({ pageParam, signal }): Promise<UsageEventPage> =>
      ok(
        api.GET('/usage/events', {
          params: {
            query: {
              scope: params.scope,
              from: params.from,
              to: params.to,
              limit: pageSize,
              ...(params.includeRunning ? { include_running: true } : {}),
              ...(params.requestsOnly ? { requests_only: true } : {}),
              ...(params.counted === undefined ? {} : { counted: params.counted }),
              ...(params.order === undefined ? {} : { order: params.order }),
              ...(pageParam === null ? {} : { cursor: pageParam }),
              ...filterQuery(params.filters),
            },
          },
          signal,
        }),
      ),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.next_cursor,
    staleTime: 60_000,
  });

/** The series of a span as a file of CSV, one line for each point. */
export async function fetchSeriesCsv(params: UsageParams): Promise<string> {
  const answer = await api.GET('/usage', {
    params: {
      query: {
        scope: params.scope,
        from: params.from,
        to: params.to,
        breakdowns: '',
        ...(params.bucket === undefined ? {} : { bucket: params.bucket }),
        ...filterQuery(params.filters),
      },
    },
    headers: { Accept: 'text/csv' },
    parseAs: 'text',
  });
  return ok(Promise.resolve(answer));
}

/** What a file of records holds: its text, and whether it stops short of the first record the span has. */
export interface RecordsFile {
  text: string;
  rows: number;
  /** True when more records lie beyond the file, which is cut at [`EXPORT_RECORDS_MOST`]. */
  cut: boolean;
}

/** One page of records as CSV, with the cursor of the page after it. */
async function fetchRecordsPage(params: EventParams, cursor: string | null, signal?: AbortSignal): Promise<{ text: string; next: string | null }> {
  const answer = await api.GET('/usage/events', {
    params: {
      query: {
        scope: params.scope,
        from: params.from,
        to: params.to,
        limit: RECORDS_PER_REQUEST,
        ...(params.requestsOnly ? { requests_only: true } : {}),
        ...(params.includeRunning ? { include_running: true } : {}),
        ...(params.counted === undefined ? {} : { counted: params.counted }),
        ...(params.order === undefined ? {} : { order: params.order }),
        ...(cursor === null ? {} : { cursor }),
        ...filterQuery(params.filters),
      },
    },
    headers: { Accept: 'text/csv' },
    parseAs: 'text',
    ...(signal === undefined ? {} : { signal }),
  });
  const text = await ok(Promise.resolve(answer));
  return { text, next: answer.response.headers.get('x-next-cursor') };
}

/**
 * The records of a span as a file of CSV: page after page of the newest first, each page's header line dropped
 * but the first, until the span is exhausted or the most a file holds is reached.
 */
export async function fetchRecordsCsv(params: EventParams, onProgress: (rows: number) => void, signal?: AbortSignal): Promise<RecordsFile> {
  const lines: string[] = [];
  let cursor: string | null = null;
  let header = '';
  for (;;) {
    const page: { text: string; next: string | null } = await fetchRecordsPage(params, cursor, signal);
    const rows = page.text.split('\n').filter((line) => line !== '');
    const first = rows.shift();
    if (header === '') header = first ?? '';
    lines.push(...rows);
    onProgress(lines.length);
    cursor = page.next;
    if (cursor === null) return { text: [header, ...lines].join('\n') + '\n', rows: lines.length, cut: false };
    if (lines.length >= EXPORT_RECORDS_MOST) return { text: [header, ...lines.slice(0, EXPORT_RECORDS_MOST)].join('\n') + '\n', rows: EXPORT_RECORDS_MOST, cut: true };
  }
}

// ---------------------------------------------------------------- rates and the catalogue

/** What each model usable at a place would be counted at now, and where that rate comes from. */
export const ratesQuery = (nodeId: string | null) =>
  queryOptions({
    queryKey: usageKeys.rates(nodeId),
    queryFn: ({ signal }): Promise<RateTable> => ok(api.GET('/usage/rates', { params: { query: nodeId === null ? { system:true } : { node_id: nodeId } }, signal })),
  });

/** The estimates as loaded, or null when none is loaded; with a model's name, its matching family. */
export const catalogueQuery = (match: string | null = null) =>
  queryOptions({
    queryKey: usageKeys.catalogue(match),
    queryFn: ({ signal }): Promise<RateCatalogue | null> =>
      ok(api.GET('/usage/catalogue', { params: { query: match === null || match === '' ? {} : { q: match } }, signal })),
    staleTime: 5 * 60_000,
  });

/** Replaces the catalogue in force: a file, whatever the address of the setting gives, or the one the release came with. */
export function setCatalogue(body: AdminUsageCatalogueSetBody): Promise<RateCatalogue> {
  return ok(api.POST('/admin/usage/catalogue', { body }));
}

/** The three settings that decide how credits are counted at a place: the rate card, whether estimates are made, and what a unit of the catalogue is worth. */
export const RATE_SETTINGS = { card: 'usage.rates', estimates: 'usage.catalogue', scale: 'usage.catalogue_scale' } as const;

/** The settings of a place as they resolve there; the rate card is read from them to be changed. */
export const nodeSettingsQuery = (nodeId: string | null) =>
  queryOptions({
    ...(nodeId === null ? ADMIN_QUERY : {}),
    queryKey: usageKeys.settings(nodeId),
    queryFn: (): Promise<ResolvedSetting[]> => nodeId === null ? ok(api.GET('/admin/settings')) : ok(api.GET('/nodes/{id}/settings', { params: { path: { id: nodeId } } })),
  });

/** Writes one of the rate settings at a place, naming the revision of the row that was seen when there was one. */
export function setRateSetting(nodeId: string | null, key: (typeof RATE_SETTINGS)[keyof typeof RATE_SETTINGS], value: unknown, expectedRevision: number | null): Promise<ResolvedSetting> {
  if (nodeId === null) return ok(api.PUT('/admin/settings/{key}', { params:{ path:{ key } },body:{ value,...(expectedRevision === null ? {} : { expected_revision:expectedRevision }) } }));
  return ok(api.PUT('/nodes/{id}/settings/{key}', { params: { path: { id: nodeId, key } }, body: { value, ...(expectedRevision === null ? {} : { expected_revision: expectedRevision }) } }));
}

/** Reads the rates and their revisions after a refused write; successful writes are refreshed by the registry of acts. */
export function refreshRates(queryClient: QueryClient): Promise<void> {
  return Promise.all([queryClient.invalidateQueries({ queryKey: ['usage', 'rates'] }), queryClient.invalidateQueries({ queryKey: ['usage', 'settings'] })]).then(() => undefined);
}

// ---------------------------------------------------------------- caps

/** The groups of an org, which a cap for a group names. */
export const orgGroupsQuery = (orgId: string) =>
  queryOptions({
    queryKey: ['org-groups', orgId] as const,
    queryFn: ({ signal }): Promise<Group[]> => ok(api.GET('/orgs/{id}/groups', { params: { path: { id: orgId } }, signal })),
    staleTime: 60_000,
  });

/** Everything that bounds work at a place, with the caller's share of what is counted per person. */
export const placeLimitsQuery = (nodeId: string, userId: string | null = null) =>
  queryOptions({
    queryKey: usageKeys.limits(nodeId, userId),
    queryFn: ({ signal }): Promise<Limits> =>
      ok(api.GET('/usage/limits', { params: { query: { node_id: nodeId, ...(userId === null ? {} : { user_id: userId }) } }, signal })),
    staleTime: 10_000,
  });

/** The caps defined on a place, a provider or the system, with how each stands. */
export const capsQuery = (where: CapPlace) =>
  queryOptions({
    queryKey: usageKeys.caps(where),
    queryFn: ({ signal }): Promise<CapStatus[]> =>
      ok(
        api.GET('/usage/caps', {
          params: {
            query:
              where.kind === 'system'
                ? { system: true }
                : where.kind === 'provider'
                  ? { provider_id: where.id }
                  : { node_id: where.id, ...(where.covering ? { covering: true } : {}) },
          },
          signal,
        }),
      ),
    staleTime: 10_000,
  });

/** The caller's own share of every cap counted per person that covers them, wherever it is defined. */
export const myLimitsQuery = queryOptions({
  queryKey: usageKeys.mine,
  queryFn: ({ signal }): Promise<CapStatus[]> => ok(api.GET('/me/limits', { signal })),
  staleTime: 10_000,
});

/** What waits for room in a cap on turns running at once, in the order it will run. */
export const waitingQuery = (capId: string) =>
  queryOptions({
    queryKey: usageKeys.waiting(capId),
    queryFn: ({ signal }): Promise<UsageCapWaitingListReply> => ok(api.GET('/usage/caps/{id}/waiting', { params: { path: { id: capId } }, signal })),
    staleTime: 5_000,
  });

/** Makes a cap. `key` is made once, when the person acts. */
export function createCap(input: CapInput, key: string): Promise<UsageCap> {
  return ok(api.POST('/usage/caps', { body: input, ...repeatable(key) }));
}

/** Changes a cap against the revision the caller saw. */
export function updateCap(id: string, body: UsageCapUpdateBody): Promise<UsageCap> {
  return ok(api.PATCH('/usage/caps/{id}', { params: { path: { id } }, body }));
}

/** Removes a cap. */
export function deleteCap(id: string): Promise<void> {
  return done(api.DELETE('/usage/caps/{id}', { params: { path: { id } } }));
}

