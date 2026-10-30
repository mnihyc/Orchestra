/** A fresh proof of a person without a password: their identity provider proves them again for one act, and comes back with a one-use proof.
 *
 * A person who signs in only through an identity provider has no password to give for a sensitive act
 * (making a token, setting up a second step). Their provider proves them again instead: the browser
 * goes to the provider's sign-in for this one purpose and comes back to the same page with a one-use
 * proof in the address's fragment, which never reaches a server's log. The page takes it at once and
 * clears it from the address, and the act sends it with its other fields within five minutes. What the
 * person had chosen before leaving is kept in this tab while they are away, so that they come back to
 * the form as they left it.
 */
import { BASE_PATH } from '@/api/client';

/** What a proof is asked for: one act, which another cannot spend. */
export type FreshPurpose = 'token_create' | 'second_step_setup' | 'recovery_codes';

/** The member of the fragment that carries the proof back. */
export const PROOF_FRAGMENT = 'identity_proof';

/** Where this tab keeps a draft while its person proves who they are elsewhere. */
const DRAFT_PREFIX = 'orch.freshProof.';

/** The address that starts proving a person again with the identity provider `key`, for `purpose`, back to `back`. */
export function freshProofAddress(key: string, purpose: FreshPurpose, back: string, serviceAccountId?: string): string {
  const query = new URLSearchParams({ redirect: back, sensitive_purpose: purpose });
  if (serviceAccountId !== undefined) query.set('service_account_id', serviceAccountId);
  return `${BASE_PATH}/auth/oidc/${encodeURIComponent(key)}/start?${query.toString()}`;
}

/** Keeps `draft` under `name` in this tab and goes to the identity provider to prove the person again. */
export function proveAgain(
  key: string,
  purpose: FreshPurpose,
  name: string,
  draft: unknown,
  serviceAccountId?: string,
): void {
  try {
    sessionStorage.setItem(`${DRAFT_PREFIX}${name}`, JSON.stringify(draft));
  } catch {
    // A tab that keeps nothing still proves the person; the form is empty when they come back.
  }
  const back = `${window.location.pathname}${window.location.search}`;
  window.location.assign(freshProofAddress(key, purpose, back, serviceAccountId));
}

/** Drafts taken and not handed on twice yet, as for the proof below. */
const lastDrafts = new Map<string, string>();

/** The draft kept under `name` before the person left to prove who they are, taken once; `null` when there is none. What it is, the form that kept it says. */
export function takeDraft(name: string): unknown {
  try {
    const key = `${DRAFT_PREFIX}${name}`;
    const stored = sessionStorage.getItem(key);
    sessionStorage.removeItem(key);
    const text = stored ?? lastDrafts.get(key) ?? null;
    if (stored === null) lastDrafts.delete(key);
    else lastDrafts.set(key, stored);
    if (text === null) return null;
    const draft: unknown = JSON.parse(text);
    return draft;
  } catch {
    return null;
  }
}

/** The proof in the fragment `hash` (`#identity_proof=…`), if it carries one. */
export function proofIn(hash: string): string | null {
  const fragment = new URLSearchParams(hash.startsWith('#') ? hash.slice(1) : hash);
  const proof = fragment.get(PROOF_FRAGMENT);
  return proof === null || proof.trim() === '' ? null : proof;
}

/** The member of the fragment that says why no proof came back. */
export const PROOF_ERROR_FRAGMENT = 'identity_proof_error';

/** Why the identity provider came back without a proof, taken from the address and cleared from it; `null` when it did not. */
export function takeFreshProofError(): string | null {
  const hash = window.location.hash;
  const fragment = new URLSearchParams(hash.startsWith('#') ? hash.slice(1) : hash);
  const why = fragment.get(PROOF_ERROR_FRAGMENT);
  if (why === null) return null;
  window.history.replaceState(window.history.state, '', `${window.location.pathname}${window.location.search}`);
  return why;
}

/** A proof taken from the address and not handed on twice yet: a screen asked again in the same moment (as a strict render does) gets it once more. */
let lastTaken: string | null = null;

/** The proof the identity provider came back with, taken from the address and cleared from it at once. */
export function takeFreshProof(): string | null {
  const proof = proofIn(window.location.hash);
  if (proof !== null) {
    window.history.replaceState(window.history.state, '', `${window.location.pathname}${window.location.search}`);
    lastTaken = proof;
    return proof;
  }
  const again = lastTaken;
  lastTaken = null;
  return again;
}
