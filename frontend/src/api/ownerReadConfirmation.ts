/** A sharing refusal can ask the person to acknowledge the current retained result count.
 * The server binds its acknowledgement to the exact write and count; this module grants no access.
 */
import { ApiError } from '@/api/client';
import type { OwnerReadDisclosure, ServerHostUseDisclosure } from '@/generated/api-types';

export type SharingDisclosure = OwnerReadDisclosure | ServerHostUseDisclosure;
type Review = (disclosure: SharingDisclosure) => Promise<boolean>;
let review: Review | null = null;
let cancel: (() => void) | null = null;

export function installOwnerReadReview(next: Review, stop: () => void): () => void {
  review = next;
  cancel = stop;
  return () => { if (review === next) { review = null; cancel = null; } };
}

export function cancelOwnerReadConfirmations(): void { cancel?.(); }

export function ownerReadDisclosure(error: unknown): SharingDisclosure | null {
  if (!(error instanceof ApiError) || !error.is('conflict.confirmation_required')) return null;
  const host = error.problem.server_host_use;
  if (typeof host === 'object' && host !== null) {
    const row = host as Partial<ServerHostUseDisclosure>;
    if (typeof row.hosts === 'number' && Number.isSafeInteger(row.hosts) && row.hosts > 0 && typeof row.confirmation === 'string' && row.confirmation.startsWith('server-host:')) return row as ServerHostUseDisclosure;
  }
  const value = error.problem.owner_reads;
  if (typeof value !== 'object' || value === null) return null;
  const row = value as Partial<OwnerReadDisclosure>;
  return typeof row.count === 'number' && Number.isSafeInteger(row.count) && row.count > 0
    && typeof row.threads === 'number' && Number.isSafeInteger(row.threads) && row.threads > 0
    && typeof row.confirmation === 'string' && row.confirmation !== '' ? row as OwnerReadDisclosure : null;
}

export function ownerReadHeaders(confirmation?: string): Record<string, string> {
  return confirmation === undefined ? {} : { [confirmation.startsWith('server-host:') ? 'orch-server-host-use-confirmed' : 'orch-owner-reads-confirmed']: confirmation };
}

/** UI-owned progress of one request; neither its proof nor its refusal grants access. */
export interface OwnerReadAttempt {
  confirmed: string | undefined;
  refusal: unknown;
  onConfirm: (confirmation: string) => void;
}

/** A changed count asks again. Cancel, navigation or an absent dialog never acknowledges it. */
export async function withOwnerReadConfirmation<T>(write: (confirmation?: string) => Promise<T>, confirmed?: string, attempt?: OwnerReadAttempt): Promise<T> {
  let confirmation = attempt?.confirmed ?? confirmed;
  let refusal = attempt?.refusal;
  for (;;) {
    if (refusal !== undefined) {
      const disclosure = ownerReadDisclosure(refusal);
      // eslint-disable-next-line @typescript-eslint/only-throw-error -- Preserve the caller's original rejection unchanged, including non-Error values.
      if (disclosure === null || review === null || !(await review(disclosure))) throw refusal;
      confirmation = disclosure.confirmation;
      if (attempt !== undefined) {
        attempt.onConfirm(confirmation);
        attempt.confirmed = confirmation;
        attempt.refusal = undefined;
      }
    }
    try { return await write(confirmation); }
    catch (error) {
      if (ownerReadDisclosure(error) === null) throw error;
      refusal = error;
      if (attempt !== undefined) attempt.refusal = error;
    }
  }
}
