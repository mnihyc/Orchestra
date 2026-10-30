/** How a host's room is read: the memory its units declare they need against the memory it reports, when that counts as full, and what being full does there.
 *
 * Nothing is reserved for a unit. The figures are sums of what environments say they need, so a host
 * that grows on demand can take more than it shows; "full" is counted from memory alone, because it is
 * the one thing that cannot be shared out twice.
 */
import type { Amounts, Host, WhenFull } from '@/generated/api-types';

/** The fullness at which a host counts as full while nothing says otherwise, in percent. */
export const FULL_PERCENT_SHIPPED = 90;

const MEBIBYTE = 1024 * 1024;

/** Memory, in bytes, from the megabytes a host reports. */
export const megabytes = (mb: number): number => mb * MEBIBYTE;

/** How much of its memory a host's units declare they need, in percent; 0 for a host that has reported none. */
export function declaredShare(capacity: Amounts, declared: Amounts): number {
  return capacity.memory_mb > 0 ? (declared.memory_mb / capacity.memory_mb) * 100 : 0;
}

/** Whether a host is full: it reports some memory, and what its units declare reaches `percent` of it. */
export function isFull(capacity: Amounts, declared: Amounts, percent: number): boolean {
  return capacity.memory_mb > 0 && declared.memory_mb * 100 >= capacity.memory_mb * percent;
}

/** What being full does at a host, and whether that is the host's own word or the system's. */
export interface FullEffect {
  full: boolean;
  /** `warn`: the request goes on, and the host answers; `block`: no environment is placed there. */
  does: WhenFull;
  own: boolean;
}

export function fullEffect(host: Pick<Host, 'capacity' | 'declared' | 'when_full'>, percent: number): FullEffect {
  return { full: isFull(host.capacity, host.declared, percent), does: host.when_full.value, own: host.when_full.own };
}
