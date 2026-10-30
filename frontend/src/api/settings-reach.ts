/** A setting's inherited reach is a native, visibility-filtered read, opened on its row. */
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { PAGE_SIZE } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import type { SettingReach } from '@/generated/api-types';

export type SettingReachTarget = Extract<SettingsTarget, { kind: 'node' | 'system' }>;
export const settingReachKey = (target: SettingReachTarget, key: string, snapshot: string) =>
  ['setting-reach', target.kind, target.kind === 'node' ? target.id : 'system', key, snapshot] as const;

/** Explicitly opening a changed native value gets its new read; no automatic repair or inferred counts. */
export const settingReachQuery = (target: SettingReachTarget, key: string, snapshot: string) => infiniteQueryOptions({
  ...(target.kind === 'system' ? ADMIN_QUERY : {}),
  queryKey: settingReachKey(target, key, snapshot),
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<SettingReach> => {
    const query = { limit: PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) };
    return target.kind === 'node'
      ? ok(api.GET('/nodes/{id}/settings/{key}/reach', { params: { path: { id: target.id, key }, query }, signal }))
      : ok(api.GET('/admin/settings/{key}/reach', { params: { path: { key }, query }, signal }));
  },
  getNextPageParam: (page) => page.overrides.next_cursor ?? undefined,
  // The snapshot identifies the displayed value. Explicit Refresh is one first-page read, not polling.
  staleTime: Infinity,
});
