/** Administrator reach belongs to the authenticated human session, never portable preferences or an agent. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { AdminMode } from '@/generated/api-types';
import { applyChanges } from '@/api/acts';
import { queryClient } from '@/api/query';
import { NetworkError } from '@/lib/requests';

export const adminModeKey = ['admin-mode'] as const;
export const adminModeQuery = queryOptions({ queryKey: adminModeKey, queryFn: ({ signal }): Promise<AdminMode> => ok(api.GET('/me/admin-mode', { signal })) });
export async function setAdminMode(enabled: boolean): Promise<AdminMode> {
  const mode = queryClient.getQueryCache().find({ queryKey: adminModeKey, exact: true });
  const reads = enabled ? [] : queryClient.getQueryCache().findAll({ predicate: (query) => query.meta?.requiresAdminMode === true && query.isActive() });
  // Retire in-flight privileged HTTP reads before the server removes their reach.
  // Ordinary reads remain owned by the current reader until the confirmed reset.
  if (!enabled) await queryClient.cancelQueries({ predicate: (query) => query.meta?.requiresAdminMode === true });
  try { return await ok(api.PUT('/me/admin-mode', { body: { as_admin: enabled } })); }
  catch (failure) {
    if (!(failure instanceof NetworkError)) {
      // A refused write kept the old mode. Resume only the same still-visible
      // suppliers; a new reader or a later mode change must not revive them.
      if (mode !== undefined && queryClient.getQueryCache().find({ queryKey: adminModeKey, exact: true }) === mode && queryClient.getQueryData<AdminMode>(adminModeKey)?.as_admin === true) {
        await Promise.all(reads.filter((query) => query.isActive() && queryClient.getQueryCache().find({ queryKey: query.queryKey, exact: true }) === query)
          .map((query) => queryClient.refetchQueries({ queryKey: query.queryKey, exact: true })));
      }
      throw failure;
    }
    // The write may have reached the session. Read it, rather than sending the write a second time.
    let current: AdminMode;
    try { current = await ok(api.GET('/me/admin-mode', {})); } catch { throw failure; }
    await applyChanges([{ session: true, mode:current }]);
    if (current.as_admin !== enabled) throw failure;
    return current;
  }
}
