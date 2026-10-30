/** How the rows of a permission page are reached from the finder: the page in view says so, and the finder lists them only then.
 *
 * "Who can archive" is a question about a page of permissions, and what answers it is the row of that
 * page which gives the right to archive. The finder knows the catalogue of permissions and can find the
 * row by what it allows; opening it is the page's own act (it scrolls to the row and lights it). A page
 * that can do so registers how while it is in view, and without one the finder lists no rows, since a
 * result that opens nothing would be a result for nothing.
 */
import { useSyncExternalStore } from 'react';

export type RowOpener = ((permission: string) => void) & { readonly permissions: ReadonlySet<string> | null };

let opener: RowOpener | null = null;
const listeners = new Set<() => void>();

/** Says how the page in view opens one of its rows. What it returns takes that back, unless another page said so meanwhile. */
export function registerRowOpener(open: (permission: string) => void, permissions?: readonly string[]): () => void {
  const registered: RowOpener = Object.assign((permission: string) => open(permission), {permissions:permissions === undefined ? null : new Set(permissions)});
  opener = registered;
  for (const listener of listeners) listener();
  return () => {
    if (opener === registered) {
      opener = null;
      for (const listener of listeners) listener();
    }
  };
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** How the page in view opens a row of itself; `null` when it has none. */
export function useRowOpener(): RowOpener | null {
  return useSyncExternalStore(subscribe, () => opener);
}
