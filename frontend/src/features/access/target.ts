/** Permission-page targets use the same public addresses as their menus, including chat and document folders. */
import type { AccessTarget } from '@/api/access';
import { linkTo } from '@/lib/links';

export function pathFor(target: Pick<AccessTarget, 'kind' | 'id'>): string {
  const url = new URL(linkTo(target));
  return `${url.pathname}${url.search}`;
}

/** Compatibility entrance for permission-page callers, backed by the shared link helper. */
export function linkOf(target: Pick<AccessTarget, 'kind' | 'id'>): string {
  return linkTo(target);
}
