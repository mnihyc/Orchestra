/** How the authority's figures are read: a fingerprint in groups a person can compare by eye, and how many whole days a certificate has left.
 *
 * Pure functions, so that what the PKI section says can be tested without drawing it.
 */

/** A SHA-256 fingerprint of 64 hexadecimal characters as pairs separated by colons, upper-case: the form a machine shows when it enrolls. */
export function groupFingerprint(hex: string): string {
  const pairs = hex.toUpperCase().match(/.{1,2}/g);
  return pairs === null ? '' : pairs.join(':');
}

const DAY_MS = 86_400_000;

/** How many whole days remain until a moment; zero once it has passed. */
export function daysLeft(until: string, now: number): number {
  return Math.max(0, Math.floor((Date.parse(until) - now) / DAY_MS));
}
