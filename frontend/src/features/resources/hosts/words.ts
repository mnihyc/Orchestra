/** What a host's figures say: how much of what it has the environments there declare they need, how much of a storage is used, and why a new environment cannot go there.
 */
import type { TFunction } from 'i18next';

import { formatNumber } from '@/lib/format';
import type { Host, HostStorage, PlaceHost } from '@/generated/api-types';

/** Measured processor room in cores, rounded to the nearest tenth on both the host and the placement picker. */
export function freeCpuWords(millis: number | null, t: TFunction, locale: string): string {
  if (millis === null) return t('placeRoom.cpuUnknown');
  const cores = Math.round(Math.max(0, millis) / 100) / 10;
  return t('placeRoom.cpu', { count: cores, n: formatNumber(cores, locale) });
}

/** A busy core reports 100 percent, or 100,000 thousandths of a percent. */
export function hostFreeCpuMillis(host: Pick<Host, 'capacity' | 'usage'>): number | null {
  const used = host.usage?.cpu_percent_milli;
  if (used === null || used === undefined || host.capacity.cpu_millis <= 0) return null;
  return Math.max(0, host.capacity.cpu_millis - Math.round(used / 100));
}

/** How much of a capacity is declared, in percent, or nothing where the host reports no capacity. A declaration reserves nothing: it can pass the whole. */
export function declaredShare(declared: number, capacity: number): number | null {
  if (!Number.isFinite(capacity) || capacity <= 0) return null;
  return (declared / capacity) * 100;
}

/**
 * The storage a new environment's volumes go to: the host's default, or its only one. Where an environment fits is read
 * from it (the host page, the pickers of where an environment runs), never from the disk the node keeps its own data on.
 */
export function placementStorage<Row extends Pick<HostStorage, 'default'>>(storages: readonly Row[]): Row | undefined {
  return storages.find((row) => row.default) ?? (storages.length === 1 ? storages[0] : undefined);
}

/** How much of a storage is used, in percent, or nothing where it has no size. */
export function usedShare(storage: Pick<HostStorage, 'capacity_gb' | 'free_gb'>): number | null {
  if (storage.capacity_gb <= 0) return null;
  return ((storage.capacity_gb - storage.free_gb) / storage.capacity_gb) * 100;
}

/** What is wrong with a host for a new environment, in words: why it cannot be chosen, or what to know before it is. Both dialogs that place work say the same. */
export function hostNotes(place: PlaceHost, t: TFunction): { off: string | null; warn: string | null } {
  if (place.policy_refusal !== undefined && place.policy_refusal !== null) return { off: place.policy_refusal, warn: null };
  const reason = place.reasons[0];
  if (reason !== undefined) return { off: t(`change.where.hostReason.${reason}`, { name: place.host.name }), warn: null };
  const warning = place.warnings[0];
  return { off: null, warn: warning === undefined ? null : t(`change.where.hostWarning.${warning}`, { name: place.host.name }) };
}
