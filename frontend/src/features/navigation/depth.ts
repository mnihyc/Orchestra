/** What a refusal for the depth of folders says: the sentence of its code and the number the system gave.
 *
 * A path holds only so many projects one inside another, and a folder is one of them. The refusal comes
 * from making a folder and from moving a project, a folder or a topic's place; both say the same thing, and
 * where the system names the number it is said with it, so that the person knows how deep they may go.
 */
import type { TFunction } from 'i18next';

import type { ApiError } from '@/api/client';

/** The words for a refusal because projects would lie too deep; `undefined` for any other refusal. */
export function depthWords(error: ApiError, t: TFunction): string | undefined {
  if (!error.is('project.too_deep')) return undefined;
  const limit = error.extra('project.too_deep')?.limit;
  return limit === undefined ? undefined : `${t('errors.project.too_deep')} ${t('folders.depth', { count: limit })}`;
}
