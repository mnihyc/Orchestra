/** What a host's figures say: how much of what it has the environments there declare they need, how much of a storage is used, and why a new environment cannot go there.
 */
import type { TFunction } from 'i18next';

import type { HostStorage, PlaceHost } from '@/generated/api-types';

/** The host reports memory in MB; people read it in GB. */
const MB_PER_GB = 1_024;

/** How much of a capacity is declared, in percent, or nothing where the host reports no capacity. A declaration reserves nothing: it can pass the whole. */
export function declaredShare(declared: number, capacity: number): number | null {
  if (!Number.isFinite(capacity) || capacity <= 0) return null;
  return (declared / capacity) * 100;
}

/** How much of a storage is used, in percent, or nothing where it has no size. */
export function usedShare(storage: Pick<HostStorage, 'capacity_gb' | 'free_gb'>): number | null {
  if (storage.capacity_gb <= 0) return null;
  return ((storage.capacity_gb - storage.free_gb) / storage.capacity_gb) * 100;
}

/** What is wrong with a host for a new environment, in words: why it cannot be chosen, or what to know before it is. Both dialogs that place work say the same. */
export function hostNotes(place: PlaceHost, t: TFunction): { off: string | null; warn: string | null } {
  if (place.policy_refusal !== undefined && place.policy_refusal !== null) return { off: place.policy_refusal, warn: null };
  const declared = Math.round(place.declared.memory_mb / MB_PER_GB);
  const capacity = Math.round(place.capacity.memory_mb / MB_PER_GB);
  const reason = place.reasons[0];
  if (reason !== undefined) return { off: t(`change.where.hostReason.${reason}`, { declared, capacity, name: place.host.name }), warn: null };
  const warning = place.warnings[0];
  return { off: null, warn: warning === undefined ? null : t(`change.where.hostWarning.${warning}`, { declared, capacity, name: place.host.name }) };
}
