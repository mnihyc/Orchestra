/** API client for System management, root orgs and their two limits, and stored metrics.
 *
 * Everything here is for a system admin in a browser session: the people of the system and the
 * orgs at its root, the settings of the system itself, what it stores about itself, its hosts and
 * runtimes, its capabilities, harness builds and agent profiles, its storage and authority, its
 * audit trail, its plugins and its identity providers. A list that grows is read a page at a time.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { resourceRows } from '@/api/listed';
import type {
  AdminPlugin,
  AdminUser,
  AdminUserCreateBody,
  AdminUserCreateReply,
  AdminUserPage,
  AdminUserResetLinkCreateBody,
  AdminUserResetLinkCreateReply,
  AdminUserStatus,
  AdminUserUpdateBody,
  Agent,
  AuditEntryPage,
  AuditExportFormat,
  BackupRestoreList,
  CapabilityDef,
  HarnessBuild,
  HarnessChannelsStatus,
  HarnessKindInfo,
  Host,
  IdentityProvider,
  IdentityProviderInput,
  IdentityProviderTestReply,
  IdentityProviderUpdateBody,
  InstallationRecords,
  MemberPage,
  MetricSeries,
  Node,
  Org,
  OrgLimitsChange,
  OrgPage,
  PkiInfo,
  ResolvedSetting,
  ReleaseCheck,
  Runtime,
  SemanticSearchOrg,
  SemanticSearchOverview,
  StorageStats,
  SystemInfo,
  UnitKind,
} from '@/generated/api-types';

/** How many rows of a list one request brings: enough to fill a screen, few enough to answer quickly. */
export const ADMIN_PAGE_SIZE = 50;

const compact = <Query extends Record<string, unknown>>(query: Query): Query => {
  const kept: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(query)) {
    if (value !== undefined && value !== null && value !== '') kept[name] = value;
  }
  return kept as Query;
};

// ---------------------------------------------------------------- the state of the system

export const systemKey = ['admin', 'system'] as const;

/** Version and build, the parts with their heartbeats, the queues, the machines online and the threads running. */
export const systemQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: systemKey,
  queryFn: ({ signal }): Promise<SystemInfo> => ok(api.GET('/admin/system', { signal })),
});

export const installationRecordsKey = ['admin', 'installation-records'] as const;
export const backupRestoreListQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: ['admin', 'backup-restore-list'] as const,
  queryFn: ({ signal }): Promise<BackupRestoreList | null> => ok(api.GET('/admin/backup-restore-list', { signal })),
  staleTime: 0,
  refetchOnMount: 'always',
});

/** Protected installation records, including backups completed while the API was stopped. */
export const installationRecordsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: installationRecordsKey,
  queryFn: ({ signal }): Promise<InstallationRecords> => ok(api.GET('/admin/installation-records', { signal })),
});

/** An explicit read of the configured release source; the returned command is only text. */
export function checkRelease(): Promise<ReleaseCheck> {
  return ok(api.POST('/admin/releases/check'));
}

export interface MetricRequest {
  /** Names of the system's measurements, without their prefix. */
  names: readonly string[];
  /** The span, as moments. */
  from: string;
  to: string;
}

export const metricsKey = (request: MetricRequest) => ['admin', 'metrics', request] as const;

/**
 * What the system stored about itself over a span. The step is the server's to choose: it gives at
 * most as many points for a series as a chart can tell apart.
 */
export const metricsQuery = (request: MetricRequest) =>
  queryOptions({
    ...ADMIN_QUERY,
    queryKey: metricsKey(request),
    queryFn: async ({ signal }): Promise<MetricSeries[]> =>
      // The generated reply types a moment and its value as a list of two; they are the pair the API documents.
      (await ok(api.GET('/admin/metrics', { params: { query: { name: [...request.names], from: request.from, to: request.to } }, signal }))) as MetricSeries[],
  });

// ---------------------------------------------------------------- search by meaning

export const semanticSearchKey = ['admin', 'semantic-search'] as const;

/** Where search by meaning stands for each root org where it is on or that has an index. */
export const semanticSearchQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: semanticSearchKey,
  queryFn: ({ signal }): Promise<SemanticSearchOverview> => ok(api.GET('/admin/semantic-search', { signal })),
});

/** Rebuilds a root org's index in place: its vectors are made anew with the model named. */
export function rebuildSemanticSearch(orgId: string): Promise<SemanticSearchOrg> {
  return ok(api.POST('/admin/semantic-search/{id}/rebuild', { params: { path: { id: orgId } } }));
}

/** Keeps the generation shown to the admin, including its model, prompts and chunk size. */
export function keepSemanticSearch(orgId: string, generationId: string): Promise<SemanticSearchOrg> {
  return ok(api.POST('/admin/semantic-search/{id}/keep', { params: { path: { id: orgId } }, body: { generation_id: generationId } }));
}

// ---------------------------------------------------------------- settings of the system

export const settingsKey = ['admin', 'settings'] as const;

/** Every setting at system scope with where its value comes from. */
export const systemSettingsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: settingsKey,
  queryFn: ({ signal }): Promise<ResolvedSetting[]> => ok(api.GET('/admin/settings', { signal })),
});

/** Sets a setting of the system, and with `locked` holds every org to it. */
export function setSystemSetting(key: string, value: unknown, locked?: boolean): Promise<ResolvedSetting> {
  return ok(api.PUT('/admin/settings/{key}', { params: { path: { key } }, body: { value, ...(locked === undefined ? {} : { locked }) } }));
}

/** Puts a setting of the system back to what Orch ships. */
export function clearSystemSetting(key: string): Promise<ResolvedSetting> {
  return ok(api.DELETE('/admin/settings/{key}', { params: { path: { key } } }));
}

// ---------------------------------------------------------------- people

export interface UserFilter {
  q: string;
  status: AdminUserStatus | '';
  admins: boolean;
}

export const usersKey = (filter: UserFilter) => ['admin', 'users', filter] as const;

/** The people of the system by name, a page at a time. */
export const usersQuery = (filter: UserFilter) =>
  infiniteQueryOptions({
    ...ADMIN_QUERY,
    queryKey: usersKey(filter),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<AdminUserPage> =>
      ok(
        api.GET('/admin/users', {
          params: {
            query: compact({
              q: filter.q.trim(),
              status: filter.status === '' ? undefined : filter.status,
              system_admin: filter.admins ? true : undefined,
              limit: ADMIN_PAGE_SIZE,
              cursor: pageParam,
            }),
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** The first people whose name, username or address holds `q`, for a field that picks one of them. */
export const userSearchQuery = (q: string) =>
  queryOptions({
    ...ADMIN_QUERY,
    queryKey: ['admin', 'user-search', q] as const,
    queryFn: ({ signal }): Promise<AdminUserPage> => ok(api.GET('/admin/users', { params: { query: compact({ q: q.trim(), limit: ADMIN_PAGE_SIZE }) }, signal })),
  });

/** Makes a person, with their own org; the answer holds the link by which they set a password. */
export function createUser(body: AdminUserCreateBody, key: string): Promise<AdminUserCreateReply> {
  return ok(api.POST('/admin/users', { body, ...repeatable(key) }));
}

/** Suspends or resumes a person, or makes them a system admin or not. */
export function updateUser(id: string, body: AdminUserUpdateBody): Promise<AdminUser> {
  return ok(api.PATCH('/admin/users/{id}', { params: { path: { id } }, body }));
}

/** Makes the address at which a person sets a new password. */
export function makeResetLink(id: string, body: AdminUserResetLinkCreateBody): Promise<AdminUserResetLinkCreateReply> {
  return ok(api.POST('/admin/users/{id}/reset-link', { params: { path: { id } }, body }));
}

/** Gives a person one invitation to make a team org; one they hold unused stays as it was. */
export function giveOrgInvitation(id: string): Promise<void> {
  return done(api.POST('/admin/users/{id}/org-invitation', { params: { path: { id } } }));
}

/** Takes back the invitation to make a team org that a person has not used. */
export function takeBackOrgInvitation(id: string): Promise<void> {
  return done(api.DELETE('/admin/users/{id}/org-invitation', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- root orgs

export interface OrgFilter {
  q: string;
  deleted: boolean;
  /** Only team orgs, or only people's own; both, teams first, when absent. */
  orgKind?: 'team' | 'personal';
}

export const adminOrgsKey = (filter: OrgFilter) => ['admin', 'orgs', filter] as const;

/** Every org of the system by name, or those deleted and not yet purged, a page at a time.
 * A form's optional scope isolates its reader/query session without changing any native argument. */
export const adminOrgsQuery = (filter: OrgFilter, choice?: { reader: number; field: string }) =>
  infiniteQueryOptions({
    ...ADMIN_QUERY,
    queryKey: choice === undefined ? adminOrgsKey(filter) : [...adminOrgsKey(filter), choice] as const,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<OrgPage> =>
      ok(
        api.GET('/admin/orgs', {
          params: { query: compact({ q: filter.q.trim(), deleted: filter.deleted ? ('only' as const) : undefined, org_kind: filter.orgKind, limit: ADMIN_PAGE_SIZE, cursor: pageParam }) },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** Sets the two limits of a root org; setting one removes nobody and no org. */
export function setOrgLimits(id: string, change: OrgLimitsChange): Promise<Org> {
  return ok(api.PATCH('/admin/orgs/{id}', { params: { path: { id } }, body: change }));
}

/** Brings a deleted org back, archived, with everything under it. */
export function restoreOrg(id: string): Promise<Node> {
  return ok(api.POST('/nodes/{id}/restore', { params: { path: { id } }, body: {} }));
}

/** The people of an org, to name one of them its owner. */
export const orgPeopleQuery = (orgId: string, q: string) =>
  queryOptions({
    ...ADMIN_QUERY,
    queryKey: ['admin', 'org-people', orgId, q] as const,
    queryFn: ({ signal }): Promise<MemberPage> =>
      ok(api.GET('/orgs/{id}/members', { params: { path: { id: orgId }, query: compact({ q: q.trim(), limit: ADMIN_PAGE_SIZE }) }, signal })),
  });

/** Names a person an owner of an org, and its first. */
export function nameOwner(orgId: string, userId: string): Promise<Node> {
  return ok(api.POST('/nodes/{id}/transfer', { params: { path: { id: orgId } }, body: { user_id: userId, keep_previous: true } }));
}

// ---------------------------------------------------------------- hosts and runtimes

export const hostsKey = ['admin', 'hosts'] as const;

/** Every host on the system, the system's own and those members added for their orgs. */
export const hostsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: hostsKey,
  queryFn: ({ signal }): Promise<Host[]> => ok(api.GET('/admin/hosts', { signal })),
});

export interface RuntimeFilter {
  host: string;
  kind: UnitKind | '';
}

export const runtimesKey = (filter: RuntimeFilter) => ['admin', 'runtimes', filter] as const;

/** Both pools of units on every host, or on one. */
export const runtimesQuery = (filter: RuntimeFilter) =>
  queryOptions({
    ...ADMIN_QUERY,
    queryKey: runtimesKey(filter),
    queryFn: ({ signal }): Promise<Runtime[]> =>
      ok(api.GET('/admin/runtimes', { params: { query: compact({ host_id: filter.host, kind: filter.kind === '' ? undefined : filter.kind }) }, signal })),
  });

// ---------------------------------------------------------------- capabilities, harnesses, agents, plugins

export const capabilitiesKey = ['admin', 'capabilities'] as const;

export const capabilitiesQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: capabilitiesKey,
  queryFn: ({ signal }): Promise<CapabilityDef[]> => ok(api.GET('/admin/capabilities', { signal })),
});

/** Makes or changes a definition. A definition that exists is changed against the revision the screen read. */
export function saveCapability(def: CapabilityDef): Promise<CapabilityDef> {
  return ok(api.PUT('/admin/capabilities/{key}', { params: { path: { key: def.key } }, body: def }));
}

export function deleteCapability(key: string): Promise<CapabilityDef> {
  return ok(api.DELETE('/admin/capabilities/{key}', { params: { path: { key } } }));
}

export const harnessBuildsKey = ['admin', 'harnesses'] as const;
export const harnessChannelsKey = [...harnessBuildsKey, 'channels'] as const;

/** Local channel read metadata and the reason for its current schedule; this contacts no vendor. */
export const harnessChannelsQuery = queryOptions({
  ...ADMIN_QUERY,
  staleTime: 0,
  queryKey: harnessChannelsKey,
  queryFn: ({ signal }): Promise<HarnessChannelsStatus> => ok(api.GET('/admin/harnesses/channels', { signal })),
});

/** The builds of every adapter, by kind and version. */
export const harnessBuildsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: harnessBuildsKey,
  queryFn: ({ signal }): Promise<HarnessBuild[]> => ok(api.GET('/admin/harnesses', { signal })),
});

export const harnessKindsKey = ['harness-kinds'] as const;

/** The adapters this system has, built in and external, each with the descriptor it declares. */
export const harnessKindsQuery = queryOptions({
  queryKey: harnessKindsKey,
  queryFn: ({ signal }): Promise<HarnessKindInfo[]> => ok(api.GET('/harness-kinds', { signal })),
});

/** Adds a build or changes one, and says which is the default of its kind on its system and processor. */
export function saveHarnessBuild(build: HarnessBuild): Promise<HarnessBuild[]> {
  return ok(api.PUT('/admin/harnesses/{kind}/{version}', { params: { path: { kind: build.kind, version: build.version } }, body: build }));
}

/** Reads the agent programs' release channels now, as the schedule does, and answers every build. */
export function readHarnessChannels(): Promise<HarnessBuild[]> {
  return ok(api.POST('/admin/harnesses/channels'));
}

/** Checks a kept build again, in the background, and answers every build, that one marked as being checked. */
export function checkHarnessBuild(build: HarnessBuild): Promise<HarnessBuild[]> {
  return ok(api.POST('/admin/harnesses/{kind}/{version}/check', { params: { path: { kind: build.kind, version: build.version } } }));
}

export const systemAgentsKey = ['admin', 'agents'] as const;

/** The agent profiles that belong to the system itself: those no org is the home of. */
export const systemAgentsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: systemAgentsKey,
  queryFn: async ({ signal }): Promise<Agent[]> => {
    // The list carries the minimal rows of private profiles apart; the system's own are readable ones.
    const all = resourceRows<Agent>(await ok(api.GET('/agents', { signal })));
    return all.filter((agent) => agent.home === null);
  },
});

export function createSystemAgent(body: { name: string; harness_kind: string; profile: unknown }): Promise<Agent> {
  return ok(api.POST('/admin/agents', { body }));
}

export const pluginsKey = ['admin', 'plugins'] as const;

export const pluginsQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: pluginsKey,
  queryFn: ({ signal }): Promise<AdminPlugin[]> => ok(api.GET('/admin/plugins', { signal })),
});

// ---------------------------------------------------------------- storage and authority

export const storageKey = ['admin', 'storage'] as const;

export const storageQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: storageKey,
  queryFn: ({ signal }): Promise<StorageStats> => ok(api.GET('/admin/storage', { signal })),
});

export const pkiKey = ['admin', 'pki'] as const;

export const pkiQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: pkiKey,
  queryFn: ({ signal }): Promise<PkiInfo> => ok(api.GET('/admin/pki', { signal })),
});

// ---------------------------------------------------------------- the audit trail

export interface AuditFilter {
  actor: string;
  action: string;
  node: string;
  from: string;
  to: string;
}

export const auditKey = (filter: AuditFilter) => ['admin', 'audit', filter] as const;

/** The first moment of a day (UTC), as the API takes a moment. */
export const startOfDay = (day: string): string => `${day}T00:00:00.000Z`;

/** The last moment of a day (UTC). */
export const endOfDay = (day: string): string => `${day}T23:59:59.999Z`;

/** The audit trail of the whole system, newest first, a page at a time. */
export const auditQuery = (filter: AuditFilter) =>
  infiniteQueryOptions({
    ...ADMIN_QUERY,
    queryKey: auditKey(filter),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<AuditEntryPage> =>
      ok(
        api.GET('/admin/audit', {
          params: {
            query: compact({
              actor: filter.actor,
              action: filter.action,
              node_id: filter.node,
              from: filter.from === '' ? undefined : startOfDay(filter.from),
              to: filter.to === '' ? undefined : endOfDay(filter.to),
              limit: ADMIN_PAGE_SIZE,
              cursor: pageParam,
            }),
          },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** Starts the file of the audit trail for a span of days and answers its stream; the server writes `audit.exported`. */
export function startAuditExport(format: AuditExportFormat, from: string, to: string, signal?: AbortSignal): Promise<ReadableStream<Uint8Array>> {
  return ok(
    api.GET('/admin/audit/export', {
      params: { query: { format, from: startOfDay(from), to: endOfDay(to) } },
      parseAs: 'stream',
      ...(signal === undefined ? {} : { signal }),
    }),
  ) as Promise<ReadableStream<Uint8Array>>;
}

// ---------------------------------------------------------------- identity providers

export const identityProvidersKey = ['admin', 'identity-providers'] as const;

export const identityProvidersQuery = queryOptions({
  ...ADMIN_QUERY,
  queryKey: identityProvidersKey,
  queryFn: ({ signal }): Promise<IdentityProvider[]> => ok(api.GET('/admin/identity-providers', { signal })),
});

export function createIdentityProvider(input: IdentityProviderInput): Promise<IdentityProvider> {
  return ok(api.POST('/admin/identity-providers', { body: input }));
}

export function updateIdentityProvider(id: string, change: IdentityProviderUpdateBody): Promise<IdentityProvider> {
  return ok(api.PATCH('/admin/identity-providers/{id}', { params: { path: { id } }, body: change }));
}

export function deleteIdentityProvider(id: string): Promise<void> {
  return done(api.DELETE('/admin/identity-providers/{id}', { params: { path: { id } } }));
}

/** Looks at a provider itself: its description of itself and its signing keys, or what went wrong reading them. Signs nobody in. */
export function testIdentityProvider(id: string): Promise<IdentityProviderTestReply> {
  return ok(api.POST('/admin/identity-providers/{id}/test', { params: { path: { id } } }));
}
