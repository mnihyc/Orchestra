/** What a reader is told when they may not do something here: what it takes, on what, and whom to ask.
 *
 * Settings use the same ordinary act-and-place sentence as the rest of the application,
 * with the current owner as the way to ask when the server names one.
 */
import type { TFunction } from 'i18next';

import type { UserRef } from '@/generated/api-types';
import { denial } from '@/lib/permissions';

/** Says what `permission` takes on a place, in words; whom to ask is the owner, where the API names one. */
export function takes(t: TFunction, permission: string, place: string, owner: Pick<UserRef, 'display_name'> | null | undefined): string {
  return denial(t, permission, place, owner);
}
