/** Generated: the articles of the user guide with their titles, headings, keywords and the screens each belongs to.
 *
 * Read from the files of `docs/user/` when the application is built (`library.ts`), and asked for
 * here, the first time a hint, the help view or the finder wants an article. It is a part of the build of
 * its own, so that nobody pays for the guide until the help is opened; nothing is fetched from any
 * other site.
 */
import { useQuery } from '@tanstack/react-query';

import type { Guide } from '@/help/guide';

export { belongsTo } from '@/help/guide';
export type { Article, Guide, Heading } from '@/help/guide';

let loading: Promise<Guide> | null = null;

/** The guide, read once. */
export function loadGuide(): Promise<Guide> {
  loading ??= import('@/help/library').then((module) => module.guide);
  return loading;
}

/** The guide as far as it has arrived: `undefined` until the part of the build that holds it is here. */
export function useGuide(enabled = true): Guide | undefined {
  return useQuery({ queryKey: ['help-guide'], queryFn: loadGuide, enabled, staleTime: Number.POSITIVE_INFINITY, retry: false }).data;
}
