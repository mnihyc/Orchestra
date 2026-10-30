import type { QueryClient } from '@tanstack/react-query';

/** A system query requires both the human session's administrator mode and current system-admin standing.
 * Authority resets retire these projections when that reach is off, even before their view unmounts.
 * The server remains responsible for the caller's actual permissions.
 */
export const ADMIN_QUERY = { meta: { requiresAdminMode: true, requiresSystemAdmin: true } } as const;

/** A query's declared supplier, not its UI mount or key prefix, owns its authority.
 * Unknown standing retains the existing bootstrap behavior; a fresh negative
 * capsule is definitive even when the session keeps its org-admin mode on.
 */
export function queryAuthorityAllows(cache: QueryClient, meta: Record<string, unknown> | undefined): boolean {
  return !(meta?.requiresAdminMode === true && cache.getQueryData<{ as_admin: boolean }>(['admin-mode'])?.as_admin !== true)
    && !(meta?.requiresSystemAdmin === true && cache.getQueryData<{ is_system_admin: boolean }>(['me'])?.is_system_admin === false);
}
