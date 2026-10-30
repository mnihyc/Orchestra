/** API client for Settings and parameters.
 *
 * The registry of every key there is, the settings in effect at a place (a node, an environment, a
 * provider's runtime, or the system) with where each value comes from, a value written there and
 * taken away again, and the secrets of a place, whose names are listed and whose values are only ever
 * written. A screen that reads the few settings it explains itself by (the rules an org gives its
 * invitations) uses the same list as the editor, under the same key.
 */
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { queryOptions } from '@tanstack/react-query';

import { ApiError, api, done, ok, repeatable } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import { queryClient } from '@/api/query';
import { hostKey, providerQuery } from '@/api/resources';
import { sameValue } from '@/features/settings/settingsModel';
import type { Destination, NotificationChannel, NotificationTestReply, Notifier, ResolvedSetting, SecretInfo, SettingDef } from '@/generated/api-types';

/** Where settings are read and written: a node (an org, a project, a topic), an environment, a managed provider's runtime, or the system. */
export type SettingsTarget =
  | { kind: 'node'; id: string }
  | { kind: 'environment'; id: string }
  | { kind: 'provider'; id: string }
  | { kind: 'host'; id: string }
  | { kind: 'system' };

/** The key a node's settings are kept under: what other screens that read a few of them use. */
export const settingsKey = (nodeId: string) => ['settings', nodeId] as const;

/** The single read of a host's effective maintenance settings. */
export const hostSettingsKey = (hostId: string) => ['host-settings', hostId] as const;

/** The key of the settings of any target. */
export function targetSettingsKey(target: SettingsTarget): readonly string[] {
  switch (target.kind) {
    case 'node':
      return settingsKey(target.id);
    case 'system':
      return ['admin', 'settings'];
    case 'host':
      return hostSettingsKey(target.id);
    default:
      return [`${target.kind}-settings`, target.id];
  }
}

/** Every setting in effect at a node, each with where its value comes from and whether the caller may change it. */
export const settingsQuery = (nodeId: string) =>
  queryOptions({
    queryKey: settingsKey(nodeId),
    queryFn: (): Promise<ResolvedSetting[]> =>
      ok(api.GET('/nodes/{id}/settings', { params: { path: { id: nodeId } } })),
    staleTime: Infinity,
  });

/** Every key there is, with its words, the schema of its value, its class, where it can be set and how scopes combine. It changes with a release. */
export const registryQuery = queryOptions({
  queryKey: ['settings-registry'] as const,
  queryFn: ({ signal }): Promise<SettingDef[]> => ok(api.GET('/settings/registry', { signal })),
  staleTime: Number.POSITIVE_INFINITY,
});

/** The settings in effect at any target. */
export const targetSettingsQuery = (target: SettingsTarget) =>
  queryOptions({
    ...(target.kind === 'system' ? ADMIN_QUERY : {}),
    queryKey: targetSettingsKey(target),
    queryFn: (): Promise<ResolvedSetting[]> => {
      switch (target.kind) {
        case 'node':
          return ok(api.GET('/nodes/{id}/settings', { params: { path: { id: target.id } } }));
        case 'environment':
          return ok(api.GET('/environments/{id}/settings', { params: { path: { id: target.id } } }));
        case 'provider':
          return ok(api.GET('/providers/{id}/settings', { params: { path: { id: target.id } } }));
        case 'host':
          return ok(api.GET('/hosts/{id}/settings', { params: { path: { id: target.id } } }));
        case 'system':
          return ok(api.GET('/admin/settings'));
      }
    },
  });

/** What is written with a value: whether it is locked against the places below, and the revision of the row it replaces. */
export interface SettingWrite {
  value: unknown;
  locked?: boolean;
  expectedRevision?: number | null;
}

/** Changes only one own key, preserving the other confirmed config members under the resource's revision guard. */
async function changeResourceSetting(target: Extract<SettingsTarget, { kind: 'environment' | 'provider' }>, key: string, value: unknown, remove: boolean, expectedRevision?: number | null): Promise<ResolvedSetting> {
  const stale = () => new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'The setting changed', detail: 'Read the current own value before saving again.', status: 409, request_id: '' }, null);
  if (target.kind === 'environment') {
    const [current, rows] = await Promise.all([
      queryClient.query({ ...environmentQuery(target.id), staleTime: Infinity }),
      queryClient.query({ ...targetSettingsQuery(target), staleTime: Infinity }),
    ]);
    const config = typeof current.config === 'object' && current.config !== null && !Array.isArray(current.config) ? { ...current.config } as Record<string, unknown> : {};
    const own = rows.find((row) => row.key === key)?.own ?? null;
    if (!sameValue(config[key], own === null ? undefined : own.value) || (expectedRevision !== undefined && (own?.revision ?? null) !== expectedRevision)) throw stale();
    const next = remove ? Object.fromEntries(Object.entries(config).filter(([name]) => name !== key)) : { ...config, [key]: value };
    await ok(api.PATCH('/environments/{id}', { params: { path: { id: target.id } }, body: { config: next, expected_revision: current.revision } }));
  } else {
    const [current, rows] = await Promise.all([
      queryClient.query({ ...providerQuery(target.id), staleTime: Infinity }),
      queryClient.query({ ...targetSettingsQuery(target), staleTime: Infinity }),
    ]);
    const config = typeof current.runtime_config === 'object' && current.runtime_config !== null && !Array.isArray(current.runtime_config) ? { ...current.runtime_config } as Record<string, unknown> : {};
    const own = rows.find((row) => row.key === key)?.own ?? null;
    if (!sameValue(config[key], own === null ? undefined : own.value) || (expectedRevision !== undefined && (own?.revision ?? null) !== expectedRevision)) throw stale();
    const next = remove ? Object.fromEntries(Object.entries(config).filter(([name]) => name !== key)) : { ...config, [key]: value };
    await ok(api.PATCH('/providers/{id}', { params: { path: { id: target.id } }, body: { runtime_config: next, expected_revision: current.revision } }));
  }
  // A resource PATCH answers with the resource, not inherited setting provenance. Repair only
  // this missing projection; the editor and every other settings consumer share that answer.
  const list = target.kind === 'environment'
    ? await ok(api.GET('/environments/{id}/settings', { params: { path: { id: target.id } } }))
    : await ok(api.GET('/providers/{id}/settings', { params: { path: { id: target.id } } }));
  queryClient.setQueryData(targetSettingsKey(target), list);
  const next = list.find((row) => row.key === key);
  if (next === undefined) throw new Error('the changed setting was not returned');
  return next;
}

/** Sets the value of a key at a place, and answers how the key stands there now. */
export function putSetting(target: SettingsTarget, key: string, write: SettingWrite): Promise<ResolvedSetting> {
  const body = {
    value: write.value,
    ...(write.locked === undefined ? {} : { locked: write.locked }),
    ...(write.expectedRevision === undefined ? {} : { expected_revision: write.expectedRevision ?? 0 }),
  };
  switch (target.kind) {
    case 'node':
      return ok(api.PUT('/nodes/{id}/settings/{key}', { params: { path: { id: target.id, key } }, body }));
    case 'system':
      return ok(api.PUT('/admin/settings/{key}', { params: { path: { key } }, body }));
    case 'host':
      return ok(api.PUT('/hosts/{id}/settings/{key}', { params: { path: { id: target.id, key } }, body })).then((next) => { void queryClient.invalidateQueries({queryKey:hostKey(target.id),exact:true}); return next; });
    default:
      return changeResourceSetting(target, key, write.value, false, write.expectedRevision);
  }
}

/** Takes the value a place set away: the key falls back to what the places above say. */
export function clearSetting(target: SettingsTarget, key: string, expectedRevision?: number | null): Promise<ResolvedSetting> {
  const query = expectedRevision === undefined ? {} : { expected_revision: expectedRevision ?? 0 };
  switch (target.kind) {
    case 'node':
      return ok(api.DELETE('/nodes/{id}/settings/{key}', { params: { path: { id: target.id, key }, query } }));
    case 'system':
      return ok(api.DELETE('/admin/settings/{key}', { params: { path: { key }, query } }));
    case 'host':
      return ok(api.DELETE('/hosts/{id}/settings/{key}', { params: { path: { id: target.id, key }, query } })).then((next) => { void queryClient.invalidateQueries({queryKey:hostKey(target.id),exact:true}); return next; });
    default:
      return changeResourceSetting(target, key, null, true, expectedRevision);
  }
}

// ---------------------------------------------------------------- secrets

export const secretsKey = (target: SettingsTarget) => ['secrets', ...targetSettingsKey(target).slice(1)] as const;

/** The names of the secrets a place has, those it gets from the places above included. Never their values. */
export const secretsQuery = (target: SettingsTarget) =>
  queryOptions({
    queryKey: secretsKey(target),
    queryFn: ({ signal }): Promise<SecretInfo[]> => {
      switch (target.kind) {
        case 'node':
          return ok(api.GET('/nodes/{id}/secrets', { params: { path: { id: target.id } }, signal }));
        case 'environment':
          return ok(api.GET('/environments/{id}/secrets', { params: { path: { id: target.id } }, signal }));
        default:
          return Promise.resolve([]);
      }
    },
  });

/** Writes the value of a secret. It is sealed on the server and is not given back by any request. */
export function putSecret(target: SettingsTarget, name: string, value: string): Promise<SecretInfo> {
  switch (target.kind) {
    case 'node':
      return ok(api.PUT('/nodes/{id}/secrets/{name}', { params: { path: { id: target.id, name } }, body: { value } }));
    case 'environment':
      return ok(api.PUT('/environments/{id}/secrets/{name}', { params: { path: { id: target.id, name } }, body: { value } }));
    default:
      return Promise.reject(new Error('this kind of thing keeps no secrets here'));
  }
}

export function deleteSecret(target: SettingsTarget, name: string): Promise<void> {
  switch (target.kind) {
    case 'node':
      return done(api.DELETE('/nodes/{id}/secrets/{name}', { params: { path: { id: target.id, name } } }));
    case 'environment':
      return done(api.DELETE('/environments/{id}/secrets/{name}', { params: { path: { id: target.id, name } } }));
    default:
      return Promise.reject(new Error('this kind of thing keeps no secrets here'));
  }
}

// ---------------------------------------------------------------- channels and destinations

export const notifiersKey = ['notifiers'] as const;

/** The kinds of channel and of destination this system has: the built-in ones and each that was installed, with the fields each asks for. */
export const notifiersQuery = queryOptions({
  queryKey: notifiersKey,
  queryFn: ({ signal }): Promise<Notifier[]> => ok(api.GET('/notifiers', { signal })),
  staleTime: Number.POSITIVE_INFINITY,
});

export const channelsKey = (orgId: string) => ['channels', orgId] as const;

/** The ways out of Orch that a root org gives its members, to look after: for whoever makes the rules there. */
export const channelsQuery = (orgId: string) =>
  queryOptions({
    queryKey: channelsKey(orgId),
    queryFn: async ({ signal }): Promise<NotificationChannel[]> => {
      const channels = await ok(api.GET('/notification-channels', { params: { query: orgId === 'system' ? {} : { org_id: orgId } }, signal }));
      return orgId === 'system' ? channels.filter((channel) => channel.owner === 'system') : channels;
    },
  });

/** What a channel or a destination is made or changed with: its name, the values of its fields, the secrets that were typed, and its texts. */
export interface NotifierWrite {
  name?: string;
  config?: Record<string, unknown>;
  secrets?: Record<string, string>;
  enabled?: boolean;
}

/** Makes a system channel, using the same notifier fields and write-only secrets as an org channel. */
export function createSystemChannel(kind: string, write: NotifierWrite & { name: string; config: Record<string, unknown> }, key: string): Promise<NotificationChannel> {
  return ok(api.POST('/admin/notification-channels', { body: { kind, name: write.name, config: write.config,
    ...(write.secrets === undefined ? {} : { secrets: write.secrets }), ...(write.enabled === undefined ? {} : { enabled: write.enabled }) }, ...repeatable(key) }));
}

/** Makes a channel of a root org, for its members. Nothing is sent to anybody until each member switches their own route on. */
export function createOrgChannel(orgId: string, kind: string, write: NotifierWrite & { name: string; config: Record<string, unknown> }, key: string): Promise<NotificationChannel> {
  return ok(
    api.POST('/orgs/{id}/notification-channels', {
      params: { path: { id: orgId } },
      body: { kind, name: write.name, config: write.config, ...(write.secrets === undefined ? {} : { secrets: write.secrets }), ...(write.enabled === undefined ? {} : { enabled: write.enabled }) },
      ...repeatable(key),
    }),
  );
}

export function updateChannel(id: string, write: NotifierWrite): Promise<NotificationChannel> {
  return ok(api.PATCH('/notification-channels/{id}', { params: { path: { id } }, body: write }));
}

/** Ends a channel and every route through it. */
export function deleteChannel(id: string): Promise<void> {
  return done(api.DELETE('/notification-channels/{id}', { params: { path: { id } } }));
}

/** Sends one sample through a channel now; a mail server needs the address to send it to. The answer holds what the far side said. */
export function testChannel(id: string, to?: string): Promise<NotificationTestReply> {
  return ok(api.POST('/notification-channels/{id}/test', { params: { path: { id } }, body: to === undefined || to === '' ? {} : { to } }));
}

export const destinationsKey = (nodeId: string) => ['destinations', nodeId] as const;

/** Where what happens at a node is sent, for nobody in particular. */
export const destinationsQuery = (nodeId: string) =>
  queryOptions({
    queryKey: destinationsKey(nodeId),
    queryFn: ({ signal }): Promise<Destination[]> => ok(api.GET('/nodes/{id}/destinations', { params: { path: { id: nodeId } }, signal })),
  });

/** What a destination is made with, beside what a channel is. */
export interface DestinationWrite extends NotifierWrite {
  kinds?: string[];
  level?: 'all' | 'important';
}

export function createDestination(nodeId: string, channel: string, write: DestinationWrite & { name: string; config: Record<string, unknown> }, key: string): Promise<Destination> {
  return ok(
    api.POST('/nodes/{id}/destinations', {
      params: { path: { id: nodeId } },
      body: { channel, name: write.name, config: write.config, ...(write.secrets === undefined ? {} : { secrets: write.secrets }), ...(write.kinds === undefined ? {} : { kinds: write.kinds }), ...(write.level === undefined ? {} : { level: write.level }), ...(write.enabled === undefined ? {} : { enabled: write.enabled }) },
      ...repeatable(key),
    }),
  );
}

export function updateDestination(id: string, write: DestinationWrite): Promise<Destination> {
  return ok(api.PATCH('/destinations/{id}', { params: { path: { id } }, body: write }));
}

export function deleteDestination(id: string): Promise<void> {
  return done(api.DELETE('/destinations/{id}', { params: { path: { id } } }));
}

/** Delivers one sample notification through a destination now, with what came back. */
export function testDestination(id: string): Promise<NotificationTestReply> {
  return ok(api.POST('/destinations/{id}/test', { params: { path: { id } } }));
}
